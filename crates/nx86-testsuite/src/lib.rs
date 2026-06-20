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
