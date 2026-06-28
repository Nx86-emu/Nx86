use std::{collections::BTreeMap, fs, path::Path};

use nx86_core::guest::{CpuState, CpuStateDiff, RegisterName, RegisterParseError, RegisterValue};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct SyntheticArm64Test {
    pub metadata: SyntheticMetadata,
    pub program: SyntheticProgram,
    #[serde(default)]
    pub expected: ExpectedState,
    #[serde(default)]
    pub framebuffer: Option<FramebufferSpec>,
}

impl SyntheticArm64Test {
    pub fn parse(source: &str) -> Result<Self, SyntheticTestError> {
        let mut test: Self = toml::from_str(source).map_err(SyntheticTestError::Toml)?;
        test.program.bytes = decode_hex(&test.program.arm64_hex)?;
        for range in &mut test.expected.memory {
            range.bytes = decode_hex(&range.bytes_hex)?;
        }
        Ok(test)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, SyntheticTestError> {
        let path = path.as_ref();
        let source = fs::read_to_string(path).map_err(|source| SyntheticTestError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&source)
    }

    pub fn entry_point(&self) -> Result<u64, SyntheticTestError> {
        if self.metadata.entry_point.trim().is_empty() {
            return Ok(0);
        }

        parse_u64(&self.metadata.entry_point)
            .map_err(|value| SyntheticTestError::InvalidEntryPoint { value })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct SyntheticMetadata {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub entry_point: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct SyntheticProgram {
    pub arm64_hex: String,
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "kebab-case")]
pub struct ExpectedState {
    pub registers: BTreeMap<String, String>,
    pub memory: Vec<ExpectedMemoryRange>,
}

impl ExpectedState {
    pub fn typed_registers(&self) -> Result<Vec<ExpectedRegister>, SyntheticTestError> {
        self.registers
            .iter()
            .map(|(name, value)| {
                let register = name.parse::<RegisterName>()?;
                let value = RegisterValue::parse_for_register(register, value)?;
                Ok(ExpectedRegister { register, value })
            })
            .collect()
    }

    pub fn compare_cpu_state(
        &self,
        cpu_state: &CpuState,
    ) -> Result<Vec<CpuStateDiff>, SyntheticTestError> {
        let expected = self
            .typed_registers()?
            .into_iter()
            .map(|expected| (expected.register, expected.value));
        Ok(cpu_state.compare_expected_registers(expected))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ExpectedRegister {
    pub register: RegisterName,
    pub value: RegisterValue,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ExpectedMemoryRange {
    pub address: String,
    pub bytes_hex: String,
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

impl ExpectedMemoryRange {
    pub fn address_u64(&self) -> Result<u64, SyntheticTestError> {
        parse_u64(&self.address).map_err(|value| SyntheticTestError::InvalidMemoryAddress { value })
    }
}

/// A guest framebuffer region the synthetic program is expected to draw into.
///
/// Pixels are 32-bit little-endian RGBA words: storing `0xAABBGGRR` writes the
/// bytes `RR GG BB AA`, which display as red `RR`, green `GG`, blue `BB`, alpha
/// `AA`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct FramebufferSpec {
    pub base: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub format: FramebufferFormat,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FramebufferFormat {
    #[default]
    Rgba8,
}

impl FramebufferSpec {
    pub fn base_u64(&self) -> Result<u64, SyntheticTestError> {
        parse_u64(&self.base).map_err(|value| SyntheticTestError::InvalidMemoryAddress { value })
    }

    /// Length of the framebuffer in bytes (4 bytes per RGBA8 pixel).
    #[must_use]
    pub const fn byte_len(&self) -> usize {
        (self.width as usize) * (self.height as usize) * 4
    }
}

/// A rendered framebuffer produced by running a synthetic test.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

/// A mismatch between an expected memory range and what a run produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryDiff {
    pub address: u64,
    pub expected: Vec<u8>,
    pub actual: Vec<u8>,
}

/// A synthetic, clean-room shader used to exercise the Phase 49 shader
/// translation/cache path. The `stage` is a free-form string (parsed into
/// `nx86-shader`'s `ShaderStage` at the boundary, exactly as `entry_point` is
/// parsed for synthetic ARM64 tests), so this crate stays free of a shader-model
/// dependency. The source is opaque hex bytes; the legal boundary forbids real
/// game shaders, so these are placeholder inputs only.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct SyntheticShader {
    pub metadata: SyntheticShaderMetadata,
    pub source: SyntheticShaderSource,
}

impl SyntheticShader {
    pub fn parse(source: &str) -> Result<Self, SyntheticTestError> {
        let mut shader: Self = toml::from_str(source).map_err(SyntheticTestError::Toml)?;
        shader.source.bytes = decode_hex(&shader.source.source_hex)?;
        Ok(shader)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, SyntheticTestError> {
        let path = path.as_ref();
        let source = fs::read_to_string(path).map_err(|source| SyntheticTestError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&source)
    }

    /// A built-in sample shader for the worker/GUI demonstration paths.
    #[must_use]
    pub fn sample() -> Self {
        Self::synthetic(
            "sample triangle vertex",
            "synthetic placeholder vertex shader",
            "vertex",
            "main",
            b"void main() {}",
        )
    }

    /// A small clean-room set of placeholder shaders spanning every stage, for
    /// the Phase 50 batch shader-AOT demonstration. The bytes are opaque
    /// stand-ins (no real game shaders); each has a distinct source so they hash
    /// and cache to distinct `.nxshader` objects.
    #[must_use]
    pub fn sample_set() -> Vec<Self> {
        vec![
            Self::synthetic(
                "sample triangle vertex",
                "synthetic placeholder vertex shader",
                "vertex",
                "main",
                b"void vert() {}",
            ),
            Self::synthetic(
                "sample solid fragment",
                "synthetic placeholder fragment shader",
                "fragment",
                "main",
                b"void frag() {}",
            ),
            Self::synthetic(
                "sample reduce compute",
                "synthetic placeholder compute shader",
                "compute",
                "main",
                b"void comp() {}",
            ),
        ]
    }

    /// Build a synthetic shader from raw placeholder bytes, keeping `bytes` and
    /// `source-hex` consistent by construction (no fallible hex decode needed).
    #[must_use]
    fn synthetic(name: &str, description: &str, stage: &str, entry: &str, source: &[u8]) -> Self {
        Self {
            metadata: SyntheticShaderMetadata {
                name: name.to_owned(),
                description: description.to_owned(),
                stage: stage.to_owned(),
                entry: entry.to_owned(),
            },
            source: SyntheticShaderSource {
                source_hex: encode_hex(source),
                bytes: source.to_vec(),
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct SyntheticShaderMetadata {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub stage: String,
    #[serde(default)]
    pub entry: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct SyntheticShaderSource {
    pub source_hex: String,
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum SyntheticTestError {
    #[error("failed to read synthetic test {path}: {source}")]
    Read {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse synthetic test TOML: {0}")]
    Toml(toml::de::Error),
    #[error("hex data must contain an even number of digits")]
    OddHexLength,
    #[error("invalid hex byte `{byte}`")]
    InvalidHexByte { byte: String },
    #[error("invalid expected register: {0}")]
    Register(#[from] RegisterParseError),
    #[error("invalid entry point `{value}`")]
    InvalidEntryPoint { value: String },
    #[error("invalid memory address `{value}`")]
    InvalidMemoryAddress { value: String },
}

fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing to a String is infallible; the result is discarded.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn decode_hex(source: &str) -> Result<Vec<u8>, SyntheticTestError> {
    let compact: String = source
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '_' && *ch != '-')
        .collect();

    if !compact.len().is_multiple_of(2) {
        return Err(SyntheticTestError::OddHexLength);
    }

    let mut bytes = Vec::with_capacity(compact.len() / 2);
    for index in (0..compact.len()).step_by(2) {
        let byte = &compact[index..index + 2];
        let value =
            u8::from_str_radix(byte, 16).map_err(|_| SyntheticTestError::InvalidHexByte {
                byte: byte.to_owned(),
            })?;
        bytes.push(value);
    }
    Ok(bytes)
}

fn parse_u64(source: &str) -> Result<u64, String> {
    let trimmed = source.trim().replace('_', "");
    let parsed = if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16)
    } else {
        trimmed.parse()
    };

    parsed.map_err(|_| source.to_owned())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use nx86_core::guest::CpuState;

    use super::SyntheticArm64Test;

    #[test]
    fn synthetic_test_parses_expected_registers_and_memory() {
        let source = r#"
            [metadata]
            name = "mov x0 immediate"
            description = "tiny synthetic program"
            entry-point = "0x00000000"

            [program]
            arm64-hex = "20 00 80 D2"

            [expected.registers]
            x0 = "0x1"
            pc = "0x4"

            [[expected.memory]]
            address = "0x1000"
            bytes-hex = "01 02 03 04"
        "#;

        let test = SyntheticArm64Test::parse(source).expect("test should parse");

        assert_eq!(test.metadata.name, "mov x0 immediate");
        assert_eq!(test.entry_point().expect("entry should parse"), 0);
        assert_eq!(test.program.bytes, vec![0x20, 0x00, 0x80, 0xD2]);
        assert_eq!(test.expected.registers.get("x0"), Some(&"0x1".to_owned()));
        assert_eq!(test.expected.memory[0].bytes, vec![1, 2, 3, 4]);
        assert_eq!(
            test.expected.memory[0]
                .address_u64()
                .expect("address should parse"),
            0x1000
        );
    }

    #[test]
    fn synthetic_test_parses_framebuffer_section() {
        let source = r#"
            [metadata]
            name = "draw"

            [program]
            arm64-hex = ""

            [framebuffer]
            base = "0x10000"
            width = 4
            height = 2
        "#;

        let test = SyntheticArm64Test::parse(source).expect("test should parse");
        let framebuffer = test.framebuffer.expect("framebuffer should be present");

        assert_eq!(framebuffer.base_u64().expect("base should parse"), 0x10000);
        assert_eq!(framebuffer.width, 4);
        assert_eq!(framebuffer.height, 2);
        assert_eq!(framebuffer.byte_len(), 4 * 2 * 4);
        assert_eq!(framebuffer.format, super::FramebufferFormat::Rgba8);
    }

    #[test]
    fn synthetic_shader_parses_stage_and_decodes_hex_source() {
        let source = r#"
            [metadata]
            name = "demo fragment"
            stage = "fragment"
            entry = "main"

            [source]
            source-hex = "00 11 22 33"
        "#;

        let shader = super::SyntheticShader::parse(source).expect("shader should parse");
        assert_eq!(shader.metadata.stage, "fragment");
        assert_eq!(shader.metadata.entry, "main");
        assert_eq!(shader.source.bytes, vec![0x00, 0x11, 0x22, 0x33]);
    }

    #[test]
    fn synthetic_shader_sample_decodes_its_source() {
        let shader = super::SyntheticShader::sample();
        assert_eq!(shader.metadata.stage, "vertex");
        assert_eq!(shader.source.bytes, b"void main() {}");
        // The encoded hex must round-trip back to the same opaque bytes.
        assert_eq!(
            super::decode_hex(&shader.source.source_hex).expect("hex"),
            shader.source.bytes
        );
    }

    #[test]
    fn synthetic_shader_sample_set_spans_distinct_stages() {
        let set = super::SyntheticShader::sample_set();
        assert_eq!(set.len(), 3);
        let stages: Vec<&str> = set.iter().map(|s| s.metadata.stage.as_str()).collect();
        assert_eq!(stages, ["vertex", "fragment", "compute"]);
        // Each shader's source-hex round-trips to its opaque bytes, and the
        // bytes are all distinct so they hash/cache to distinct objects.
        for shader in &set {
            assert!(!shader.source.bytes.is_empty());
            assert_eq!(
                super::decode_hex(&shader.source.source_hex).expect("hex"),
                shader.source.bytes
            );
        }
        let unique: std::collections::BTreeSet<&[u8]> =
            set.iter().map(|s| s.source.bytes.as_slice()).collect();
        assert_eq!(unique.len(), set.len());
    }

    #[test]
    fn synthetic_test_without_framebuffer_defaults_to_none() {
        let source = r#"
            [metadata]
            name = "no-fb"

            [program]
            arm64-hex = ""
        "#;

        let test = SyntheticArm64Test::parse(source).expect("test should parse");

        assert!(test.framebuffer.is_none());
    }

    #[test]
    fn synthetic_test_loads_from_file() {
        let dir = tempdir().expect("temp dir should be created");
        let path = dir.path().join("add.nxarm64.toml");
        std::fs::write(
            &path,
            r#"
                [metadata]
                name = "empty"

                [program]
                arm64-hex = ""
            "#,
        )
        .expect("test file should be writable");

        let test = SyntheticArm64Test::load(&path).expect("test should load");

        assert_eq!(test.metadata.name, "empty");
        assert!(test.program.bytes.is_empty());
    }

    #[test]
    fn odd_hex_is_rejected() {
        let source = r#"
            [metadata]
            name = "bad"

            [program]
            arm64-hex = "0"
        "#;

        assert!(SyntheticArm64Test::parse(source).is_err());
    }

    #[test]
    fn expected_registers_compare_against_cpu_state() {
        let source = r#"
            [metadata]
            name = "compare"

            [program]
            arm64-hex = ""

            [expected.registers]
            x0 = "0x2"
            pc = "0x4"
            halted = "true"
        "#;
        let test = SyntheticArm64Test::parse(source).expect("test should parse");
        let mut state = CpuState::new();
        state.set_x(0, 2);
        state.set_pc(4);
        state.halt("svc #0");

        let diffs = test
            .expected
            .compare_cpu_state(&state)
            .expect("registers should parse");

        assert!(diffs.is_empty());
    }

    #[test]
    fn expected_register_mismatch_is_reported() {
        let source = r#"
            [metadata]
            name = "mismatch"

            [program]
            arm64-hex = ""

            [expected.registers]
            x0 = "0x2"
        "#;
        let test = SyntheticArm64Test::parse(source).expect("test should parse");
        let state = CpuState::new();

        let diffs = test
            .expected
            .compare_cpu_state(&state)
            .expect("registers should parse");

        assert_eq!(diffs.len(), 1);
    }
}
