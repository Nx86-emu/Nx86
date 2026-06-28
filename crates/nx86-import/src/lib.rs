use std::{
    fs,
    path::{Path, PathBuf},
};

use nx86_vmm::{GuestAddress, GuestMemory, PAGE_SIZE, PagePermissions, VmmFault};
use serde::Deserialize;
use thiserror::Error;

pub const CRATE_NAME: &str = "nx86-import";
pub const HOMEBREW_FORMAT_VERSION: u32 = 1;
pub const DEFAULT_STACK_SIZE: u64 = 64 * 1024;

const MAX_STACK_SIZE: u64 = 8 * 1024 * 1024;
const MAX_HEX_DECODE_BYTES: usize = 64 * 1024 * 1024;
const MAX_SEGMENTS: usize = 256;
const MAX_TOTAL_MAPPED_BYTES: u64 = 256 * 1024 * 1024;

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomebrewModule {
    metadata: HomebrewMetadata,
    program: HomebrewProgram,
    segments: Vec<HomebrewSegment>,
}

impl HomebrewModule {
    pub fn parse(source: &str) -> Result<Self, HomebrewImportError> {
        let parsed: HomebrewToml = toml::from_str(source).map_err(HomebrewImportError::Toml)?;
        if parsed.format_version != HOMEBREW_FORMAT_VERSION {
            return Err(HomebrewImportError::UnsupportedFormatVersion {
                version: parsed.format_version,
            });
        }

        let name = trim_required(parsed.metadata.name, "metadata.name")?;
        let version = trim_optional(parsed.metadata.version);
        let author = trim_optional(parsed.metadata.author);
        let module_id = trim_optional(parsed.metadata.module_id);
        let load_address = parse_u64(&parsed.program.load_address, "program.load-address")?;
        let entry_point = parse_u64(&parsed.program.entry_point, "program.entry-point")?;
        let stack_top = parse_u64(&parsed.program.stack_top, "program.stack-top")?;
        let stack_size = match parsed.program.stack_size {
            Some(value) => parse_u64(&value, "program.stack-size")?,
            None => DEFAULT_STACK_SIZE,
        };
        if stack_size == 0 {
            return Err(HomebrewImportError::EmptyStack);
        }
        if stack_size > MAX_STACK_SIZE {
            return Err(HomebrewImportError::StackTooLarge {
                max: MAX_STACK_SIZE,
                got: stack_size,
            });
        }

        let bytes = decode_hex(&parsed.program.arm64_hex)?;
        if bytes.is_empty() {
            return Err(HomebrewImportError::EmptyProgram);
        }
        if !bytes.len().is_multiple_of(4) {
            return Err(HomebrewImportError::ProgramLength { len: bytes.len() });
        }
        let program_end = checked_range_end(load_address, bytes.len() as u64, "program.arm64-hex")?;
        if !(load_address..program_end).contains(&entry_point)
            || !(entry_point - load_address).is_multiple_of(4)
        {
            return Err(HomebrewImportError::EntryPointOutsideProgram {
                entry_point,
                load_address,
                program_end,
            });
        }

        let program = HomebrewProgram {
            load_address,
            entry_point,
            stack_top,
            stack_size,
            arm64_bytes: bytes,
        };
        let mut segments = Vec::new();
        if parsed.segments.len() > MAX_SEGMENTS {
            return Err(HomebrewImportError::TooManySegments {
                max: MAX_SEGMENTS,
                got: parsed.segments.len(),
            });
        }
        for (index, segment) in parsed.segments.into_iter().enumerate() {
            let address = parse_u64(&segment.address, "segments.address")?;
            let bytes = decode_hex(&segment.bytes_hex)?;
            let name = trim_optional(segment.name).unwrap_or_else(|| format!("segment-{index}"));
            if bytes.is_empty() {
                return Err(HomebrewImportError::EmptySegment { name });
            }
            let permissions = SegmentPermissions::parse(&segment.permissions)?;
            let end = checked_range_end(address, bytes.len() as u64, name.as_str())?;
            segments.push(HomebrewSegment {
                name,
                address,
                end,
                permissions,
                bytes,
            });
        }

        validate_layout(&program, &segments)?;

        Ok(Self {
            metadata: HomebrewMetadata {
                name,
                version,
                author,
                module_id,
            },
            program,
            segments,
        })
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, HomebrewImportError> {
        let path = path.as_ref();
        let source = fs::read_to_string(path).map_err(|source| HomebrewImportError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&source)
    }

    #[must_use]
    pub const fn metadata(&self) -> &HomebrewMetadata {
        &self.metadata
    }

    #[must_use]
    pub const fn program(&self) -> &HomebrewProgram {
        &self.program
    }

    #[must_use]
    pub fn segments(&self) -> &[HomebrewSegment] {
        &self.segments
    }

    #[must_use]
    pub const fn entry_point(&self) -> u64 {
        self.program.entry_point
    }

    #[must_use]
    pub const fn program_load_address(&self) -> u64 {
        self.program.load_address
    }

    #[must_use]
    pub const fn stack_top(&self) -> u64 {
        self.program.stack_top
    }

    #[must_use]
    pub fn program_bytes(&self) -> &[u8] {
        &self.program.arm64_bytes
    }

    pub fn map_into(&self, memory: &mut GuestMemory) -> Result<(), HomebrewImportError> {
        let mut total_bytes: u64 = self.program.arm64_bytes.len() as u64;
        for segment in &self.segments {
            total_bytes = total_bytes.checked_add(segment.bytes.len() as u64).ok_or(
                HomebrewImportError::AddressOverflow {
                    name: "total mapping".to_owned(),
                },
            )?;
        }
        total_bytes = total_bytes.checked_add(self.program.stack_size).ok_or(
            HomebrewImportError::AddressOverflow {
                name: "total mapping".to_owned(),
            },
        )?;
        if total_bytes > MAX_TOTAL_MAPPED_BYTES {
            return Err(HomebrewImportError::TotalMappingTooLarge {
                max: MAX_TOTAL_MAPPED_BYTES,
                got: total_bytes,
            });
        }
        map_bytes(
            memory,
            self.program.load_address,
            &self.program.arm64_bytes,
            PagePermissions::READ_EXECUTE,
        )?;
        for segment in &self.segments {
            map_bytes(
                memory,
                segment.address,
                &segment.bytes,
                segment.permissions.into(),
            )?;
        }
        map_stack(memory, self.program.stack_top, self.program.stack_size)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomebrewMetadata {
    pub name: String,
    pub version: Option<String>,
    pub author: Option<String>,
    pub module_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomebrewProgram {
    pub load_address: u64,
    pub entry_point: u64,
    pub stack_top: u64,
    pub stack_size: u64,
    arm64_bytes: Vec<u8>,
}

impl HomebrewProgram {
    #[must_use]
    pub fn arm64_bytes(&self) -> &[u8] {
        &self.arm64_bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomebrewSegment {
    pub name: String,
    pub address: u64,
    pub end: u64,
    pub permissions: SegmentPermissions,
    bytes: Vec<u8>,
}

impl HomebrewSegment {
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentPermissions {
    Read,
    ReadWrite,
    ReadExecute,
}

impl SegmentPermissions {
    fn parse(source: &str) -> Result<Self, HomebrewImportError> {
        match source.trim().to_ascii_lowercase().as_str() {
            "r" => Ok(Self::Read),
            "rw" => Ok(Self::ReadWrite),
            "rx" => Ok(Self::ReadExecute),
            _ => Err(HomebrewImportError::InvalidPermissions {
                value: source.to_owned(),
            }),
        }
    }
}

impl From<SegmentPermissions> for PagePermissions {
    fn from(value: SegmentPermissions) -> Self {
        match value {
            SegmentPermissions::Read => PagePermissions::READ,
            SegmentPermissions::ReadWrite => PagePermissions::READ_WRITE,
            SegmentPermissions::ReadExecute => PagePermissions::READ_EXECUTE,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct HomebrewToml {
    #[serde(default = "default_format_version")]
    format_version: u32,
    metadata: MetadataToml,
    program: ProgramToml,
    #[serde(default)]
    segments: Vec<SegmentToml>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct MetadataToml {
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    module_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct ProgramToml {
    load_address: String,
    entry_point: String,
    stack_top: String,
    #[serde(default)]
    stack_size: Option<String>,
    arm64_hex: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct SegmentToml {
    #[serde(default)]
    name: Option<String>,
    address: String,
    permissions: String,
    bytes_hex: String,
}

const fn default_format_version() -> u32 {
    HOMEBREW_FORMAT_VERSION
}

fn trim_required(value: String, field: &'static str) -> Result<String, HomebrewImportError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(HomebrewImportError::EmptyField { field });
    }
    Ok(value.to_owned())
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

fn parse_u64(source: &str, field: &'static str) -> Result<u64, HomebrewImportError> {
    let trimmed = source.trim().replace('_', "");
    let parsed = if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16)
    } else {
        trimmed.parse()
    };
    parsed.map_err(|_| HomebrewImportError::InvalidInteger {
        field,
        value: source.to_owned(),
    })
}

fn decode_hex(source: &str) -> Result<Vec<u8>, HomebrewImportError> {
    let compact: String = source
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '_' && *ch != '-')
        .collect();
    if !compact.len().is_multiple_of(2) {
        return Err(HomebrewImportError::OddHexLength);
    }
    let decoded_len = compact.len() / 2;
    if decoded_len > MAX_HEX_DECODE_BYTES {
        return Err(HomebrewImportError::ProgramTooLarge {
            max: MAX_HEX_DECODE_BYTES,
            got: decoded_len,
        });
    }

    let mut bytes = Vec::with_capacity(compact.len() / 2);
    for index in (0..compact.len()).step_by(2) {
        let byte = &compact[index..index + 2];
        let value =
            u8::from_str_radix(byte, 16).map_err(|_| HomebrewImportError::InvalidHexByte {
                byte: byte.to_owned(),
            })?;
        bytes.push(value);
    }
    Ok(bytes)
}

fn checked_range_end(
    base: u64,
    len: u64,
    name: impl Into<String>,
) -> Result<u64, HomebrewImportError> {
    base.checked_add(len)
        .ok_or_else(|| HomebrewImportError::AddressOverflow { name: name.into() })
}

fn validate_layout(
    program: &HomebrewProgram,
    segments: &[HomebrewSegment],
) -> Result<(), HomebrewImportError> {
    let mut ranges = Vec::with_capacity(segments.len() + 2);
    let program_end = checked_range_end(
        program.load_address,
        program.arm64_bytes.len() as u64,
        "program",
    )?;
    ranges.push((
        "program".to_owned(),
        GuestAddress(program.load_address).page_base(),
        page_mapping_end(program_end)?,
    ));
    let stack_base = program
        .stack_top
        .checked_sub(program.stack_size)
        .ok_or(HomebrewImportError::StackUnderflow)?;
    ranges.push((
        "stack".to_owned(),
        GuestAddress(stack_base).page_base(),
        page_mapping_end(program.stack_top)?,
    ));
    for segment in segments {
        ranges.push((
            segment.name.clone(),
            GuestAddress(segment.address).page_base(),
            page_mapping_end(segment.end)?,
        ));
    }

    ranges.sort_by_key(|(_, start, _)| *start);
    for pair in ranges.windows(2) {
        let (first_name, _first_start, first_end) = &pair[0];
        let (second_name, second_start, _second_end) = &pair[1];
        if second_start < first_end {
            return Err(HomebrewImportError::OverlappingRanges {
                first: first_name.clone(),
                second: second_name.clone(),
            });
        }
    }
    Ok(())
}

fn page_mapping_end(end: u64) -> Result<u64, HomebrewImportError> {
    let last_byte = end
        .checked_sub(1)
        .ok_or_else(|| HomebrewImportError::AddressOverflow {
            name: "mapping".to_owned(),
        })?;
    GuestAddress(last_byte)
        .page_base()
        .checked_add(PAGE_SIZE)
        .ok_or_else(|| HomebrewImportError::AddressOverflow {
            name: "mapping".to_owned(),
        })
}

fn map_bytes(
    memory: &mut GuestMemory,
    address: u64,
    bytes: &[u8],
    final_permissions: PagePermissions,
) -> Result<(), HomebrewImportError> {
    if bytes.is_empty() {
        return Ok(());
    }
    map_pages(
        memory,
        address,
        bytes.len() as u64,
        PagePermissions::READ_WRITE,
    )?;
    memory.write(GuestAddress(address), bytes)?;
    set_pages(memory, address, bytes.len() as u64, final_permissions)?;
    Ok(())
}

fn map_stack(
    memory: &mut GuestMemory,
    stack_top: u64,
    stack_size: u64,
) -> Result<(), HomebrewImportError> {
    let stack_base = stack_top
        .checked_sub(stack_size)
        .ok_or(HomebrewImportError::StackUnderflow)?;
    map_pages(memory, stack_base, stack_size, PagePermissions::READ_WRITE)
}

fn map_pages(
    memory: &mut GuestMemory,
    address: u64,
    len: u64,
    permissions: PagePermissions,
) -> Result<(), HomebrewImportError> {
    if len == 0 {
        return Ok(());
    }
    let end = checked_range_end(address, len, "mapping")?;
    let mut page = GuestAddress(address).page_base();
    while page < end {
        memory.map_page(GuestAddress(page), permissions)?;
        page = page
            .checked_add(PAGE_SIZE)
            .ok_or_else(|| HomebrewImportError::AddressOverflow {
                name: "mapping".to_owned(),
            })?;
    }
    Ok(())
}

fn set_pages(
    memory: &mut GuestMemory,
    address: u64,
    len: u64,
    permissions: PagePermissions,
) -> Result<(), HomebrewImportError> {
    if len == 0 {
        return Ok(());
    }
    let end = checked_range_end(address, len, "mapping")?;
    let mut page = GuestAddress(address).page_base();
    while page < end {
        memory.set_page_permissions(GuestAddress(page), permissions)?;
        page = page
            .checked_add(PAGE_SIZE)
            .ok_or_else(|| HomebrewImportError::AddressOverflow {
                name: "mapping".to_owned(),
            })?;
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum HomebrewImportError {
    #[error("failed to read homebrew module {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse homebrew TOML: {0}")]
    Toml(toml::de::Error),
    #[error("unsupported homebrew format version {version}")]
    UnsupportedFormatVersion { version: u32 },
    #[error("required homebrew field `{field}` is empty")]
    EmptyField { field: &'static str },
    #[error("invalid integer in `{field}`: `{value}`")]
    InvalidInteger { field: &'static str, value: String },
    #[error("hex data must contain an even number of digits")]
    OddHexLength,
    #[error("invalid hex byte `{byte}`")]
    InvalidHexByte { byte: String },
    #[error("homebrew program has no instructions")]
    EmptyProgram,
    #[error("homebrew segment `{name}` has no bytes")]
    EmptySegment { name: String },
    #[error("homebrew program length {len} is not a multiple of 4 bytes")]
    ProgramLength { len: usize },
    #[error(
        "entry point {entry_point:#x} is outside program range {load_address:#x}..{program_end:#x}"
    )]
    EntryPointOutsideProgram {
        entry_point: u64,
        load_address: u64,
        program_end: u64,
    },
    #[error("stack size must be non-zero")]
    EmptyStack,
    #[error("stack size {got} exceeds maximum {max}")]
    StackTooLarge { max: u64, got: u64 },
    #[error("decoded program bytes {got} exceed maximum {max}")]
    ProgramTooLarge { max: usize, got: usize },
    #[error("segment count {got} exceeds maximum {max}")]
    TooManySegments { max: usize, got: usize },
    #[error("total mapped bytes {got} exceed maximum {max}")]
    TotalMappingTooLarge { max: u64, got: u64 },
    #[error("stack top is below stack size")]
    StackUnderflow,
    #[error("range `{name}` overflows the guest address space")]
    AddressOverflow { name: String },
    #[error("homebrew page mappings `{first}` and `{second}` overlap")]
    OverlappingRanges { first: String, second: String },
    #[error("invalid segment permissions `{value}`; expected r, rw, or rx")]
    InvalidPermissions { value: String },
    #[error("memory fault while loading homebrew: {0}")]
    Memory(#[from] VmmFault),
}

#[cfg(test)]
mod tests {
    use nx86_vmm::{GuestAddress, GuestMemory, PagePermissions};
    use tempfile::tempdir;

    use super::{HomebrewImportError, HomebrewModule, SegmentPermissions};

    const SIMPLE: &str = r#"
        format-version = 1

        [metadata]
        name = "Exit 42"
        version = "0.1.0"
        author = "Nx86"

        [program]
        load-address = "0x8000"
        entry-point = "0x8000"
        stack-top = "0x9000_0000"
        stack-size = "0x4000"
        arm64-hex = "40 05 80 D2 01 00 00 D4"

        [[segments]]
        name = "data"
        address = "0x10000"
        permissions = "rw"
        bytes-hex = "AA BB CC DD"
    "#;

    #[test]
    fn parses_homebrew_metadata_program_and_segments() {
        let module = HomebrewModule::parse(SIMPLE).expect("module should parse");

        assert_eq!(module.metadata().name, "Exit 42");
        assert_eq!(module.entry_point(), 0x8000);
        assert_eq!(module.stack_top(), 0x9000_0000);
        assert_eq!(
            module.program_bytes(),
            [0x40, 0x05, 0x80, 0xD2, 1, 0, 0, 0xD4]
        );
        assert_eq!(module.segments().len(), 1);
        assert_eq!(
            module.segments()[0].permissions,
            SegmentPermissions::ReadWrite
        );
    }

    #[test]
    fn loads_homebrew_from_file() {
        let root = tempdir().expect("temp dir should be created");
        let path = root.path().join("exit.nxhb.toml");
        std::fs::write(&path, SIMPLE).expect("module should be writable");

        let module = HomebrewModule::load(&path).expect("module should load");

        assert_eq!(module.metadata().name, "Exit 42");
    }

    #[test]
    fn maps_program_data_and_stack_into_guest_memory() {
        let module = HomebrewModule::parse(SIMPLE).expect("module should parse");
        let mut memory = GuestMemory::new_logical();

        module.map_into(&mut memory).expect("module should map");

        assert_eq!(
            memory
                .read(GuestAddress(0x8000), 8)
                .expect("program should read back"),
            module.program_bytes()
        );
        assert_eq!(
            memory.page_permissions(GuestAddress(0x8000)),
            Some(PagePermissions::READ_EXECUTE)
        );
        assert_eq!(
            memory
                .read(GuestAddress(0x10000), 4)
                .expect("data should read back"),
            vec![0xAA, 0xBB, 0xCC, 0xDD]
        );
        assert_eq!(
            memory.page_permissions(GuestAddress(0x10000)),
            Some(PagePermissions::READ_WRITE)
        );
        assert_eq!(
            memory.page_permissions(GuestAddress(0x8fff_c000)),
            Some(PagePermissions::READ_WRITE)
        );
    }

    #[test]
    fn rejects_entry_point_outside_program() {
        let source = SIMPLE.replace("entry-point = \"0x8000\"", "entry-point = \"0x9000\"");

        let error = HomebrewModule::parse(&source).expect_err("entry should be rejected");

        assert!(matches!(
            error,
            HomebrewImportError::EntryPointOutsideProgram { .. }
        ));
    }

    #[test]
    fn rejects_overlapping_ranges() {
        let source = SIMPLE.replace("address = \"0x10000\"", "address = \"0x8004\"");

        let error = HomebrewModule::parse(&source).expect_err("overlap should be rejected");

        assert!(matches!(
            error,
            HomebrewImportError::OverlappingRanges { .. }
        ));
    }

    #[test]
    fn rejects_distinct_ranges_that_share_a_page_mapping() {
        let source = SIMPLE.replace("address = \"0x10000\"", "address = \"0x8008\"");

        let error =
            HomebrewModule::parse(&source).expect_err("shared page mapping should be rejected");

        assert!(matches!(
            error,
            HomebrewImportError::OverlappingRanges { .. }
        ));
    }

    #[test]
    fn rejects_writable_executable_segment_permissions() {
        let source = SIMPLE.replace("permissions = \"rw\"", "permissions = \"rwx\"");

        let error = HomebrewModule::parse(&source).expect_err("permissions should be rejected");

        assert!(matches!(
            error,
            HomebrewImportError::InvalidPermissions { .. }
        ));
    }

    #[test]
    fn rejects_stack_size_above_maximum() {
        let source = SIMPLE.replace("stack-size = \"0x4000\"", "stack-size = \"0x900000\"");

        let error = HomebrewModule::parse(&source).expect_err("stack size should be rejected");

        assert!(matches!(error, HomebrewImportError::StackTooLarge { .. }));
    }
}
