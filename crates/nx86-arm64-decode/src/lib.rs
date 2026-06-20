use serde::{Deserialize, Serialize};
use thiserror::Error;

pub fn decode_program(
    bytes: &[u8],
    base_address: u64,
) -> Result<Vec<DecodedInstruction>, DecodeError> {
    if !bytes.len().is_multiple_of(4) {
        return Err(DecodeError::InvalidLength { len: bytes.len() });
    }

    bytes
        .chunks_exact(4)
        .enumerate()
        .map(|(index, raw)| {
            let byte_offset = u64::try_from(index)
                .ok()
                .and_then(|index| index.checked_mul(4))
                .ok_or(DecodeError::AddressOverflow {
                    base_address,
                    instruction_index: index,
                })?;
            let address =
                base_address
                    .checked_add(byte_offset)
                    .ok_or(DecodeError::AddressOverflow {
                        base_address,
                        instruction_index: index,
                    })?;
            decode_instruction([raw[0], raw[1], raw[2], raw[3]], address)
        })
        .collect()
}

pub fn decode_instruction(
    raw_bytes: [u8; 4],
    address: u64,
) -> Result<DecodedInstruction, DecodeError> {
    let word = u32::from_le_bytes(raw_bytes);
    let kind = decode_kind(word, address)?;
    let class = kind.class();
    let disassembly = kind.disassembly();

    Ok(DecodedInstruction {
        address,
        word,
        raw_bytes,
        class,
        kind,
        disassembly,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct DecodedInstruction {
    pub address: u64,
    pub word: u32,
    pub raw_bytes: [u8; 4],
    pub class: InstructionClass,
    pub kind: InstructionKind,
    pub disassembly: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum InstructionClass {
    DataProcessingImmediate,
    Branch,
    Exception,
    LoadStore,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum InstructionKind {
    MovZ {
        rd: u8,
        imm: u64,
        shift: u8,
    },
    AddImmediate {
        rd: u8,
        rn: u8,
        imm: u64,
    },
    SubImmediate {
        rd: u8,
        rn: u8,
        imm: u64,
    },
    Branch {
        offset: i64,
        target: u64,
    },
    Svc {
        imm: u16,
    },
    /// `STR Wt, [Xn, #offset]` — store the low 32 bits of `rt` at `rn + offset`.
    StoreWord {
        rt: u8,
        rn: u8,
        offset: u64,
    },
}

impl InstructionKind {
    #[must_use]
    pub const fn class(&self) -> InstructionClass {
        match self {
            Self::MovZ { .. } | Self::AddImmediate { .. } | Self::SubImmediate { .. } => {
                InstructionClass::DataProcessingImmediate
            }
            Self::Branch { .. } => InstructionClass::Branch,
            Self::Svc { .. } => InstructionClass::Exception,
            Self::StoreWord { .. } => InstructionClass::LoadStore,
        }
    }

    #[must_use]
    pub fn disassembly(&self) -> String {
        match self {
            Self::MovZ { rd, imm, shift } if *shift == 0 => {
                format!("mov {}, #{imm:#x}", gp_or_zr(*rd))
            }
            Self::MovZ { rd, imm, shift } => {
                format!("movz {}, #{imm:#x}, lsl #{shift}", gp_or_zr(*rd))
            }
            Self::AddImmediate { rd, rn, imm } => {
                format!("add {}, {}, #{imm:#x}", gp_or_sp(*rd), gp_or_sp(*rn))
            }
            Self::SubImmediate { rd, rn, imm } => {
                format!("sub {}, {}, #{imm:#x}", gp_or_sp(*rd), gp_or_sp(*rn))
            }
            Self::Branch { target, .. } => format!("b {target:#x}"),
            Self::Svc { imm } => format!("svc #{imm:#x}"),
            Self::StoreWord { rt, rn, offset } => {
                format!("str w{rt}, [{}, #{offset:#x}]", gp_or_sp(*rn))
            }
        }
    }
}

fn decode_kind(word: u32, address: u64) -> Result<InstructionKind, DecodeError> {
    if (word & 0x7F80_0000) == 0x5280_0000 && bit(word, 31) && bits(word, 29, 2) == 0b10 {
        let rd = bits(word, 0, 5) as u8;
        let imm16 = u64::from(bits(word, 5, 16));
        let shift = (bits(word, 21, 2) as u8) * 16;
        return Ok(InstructionKind::MovZ {
            rd,
            imm: imm16 << shift,
            shift,
        });
    }

    if (word & 0xFF00_0000) == 0x9100_0000 {
        let rd = bits(word, 0, 5) as u8;
        let rn = bits(word, 5, 5) as u8;
        let shift = if bit(word, 22) { 12 } else { 0 };
        let imm = u64::from(bits(word, 10, 12)) << shift;
        return Ok(InstructionKind::AddImmediate { rd, rn, imm });
    }

    if (word & 0xFF00_0000) == 0xD100_0000 {
        let rd = bits(word, 0, 5) as u8;
        let rn = bits(word, 5, 5) as u8;
        let shift = if bit(word, 22) { 12 } else { 0 };
        let imm = u64::from(bits(word, 10, 12)) << shift;
        return Ok(InstructionKind::SubImmediate { rd, rn, imm });
    }

    if (word & 0xFC00_0000) == 0x1400_0000 {
        let imm26 = u64::from(word & 0x03FF_FFFF);
        let offset = sign_extend(imm26 << 2, 28);
        let target = address.wrapping_add_signed(offset);
        return Ok(InstructionKind::Branch { offset, target });
    }

    if (word & 0xFFE0_001F) == 0xD400_0001 {
        return Ok(InstructionKind::Svc {
            imm: bits(word, 5, 16) as u16,
        });
    }

    // STR (immediate, unsigned offset), 32-bit variant: size=10, V=0, opc=00.
    if (word & 0xFFC0_0000) == 0xB900_0000 {
        let rt = bits(word, 0, 5) as u8;
        let rn = bits(word, 5, 5) as u8;
        let offset = u64::from(bits(word, 10, 12)) << 2;
        return Ok(InstructionKind::StoreWord { rt, rn, offset });
    }

    Err(DecodeError::UnsupportedInstruction { address, word })
}

fn gp_or_sp(register: u8) -> String {
    if register == 31 {
        "sp".to_owned()
    } else {
        format!("x{register}")
    }
}

fn gp_or_zr(register: u8) -> String {
    if register == 31 {
        "xzr".to_owned()
    } else {
        format!("x{register}")
    }
}

fn bit(word: u32, index: u8) -> bool {
    ((word >> index) & 1) != 0
}

fn bits(word: u32, offset: u8, width: u8) -> u32 {
    (word >> offset) & ((1 << width) - 1)
}

fn sign_extend(value: u64, width: u8) -> i64 {
    let shift = 64 - width;
    ((value << shift) as i64) >> shift
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DecodeError {
    #[error("AArch64 byte stream length {len} is not a multiple of 4")]
    InvalidLength { len: usize },
    #[error(
        "AArch64 instruction address overflow from base {base_address:#x} at instruction index {instruction_index}"
    )]
    AddressOverflow {
        base_address: u64,
        instruction_index: usize,
    },
    #[error("unsupported AArch64 instruction {word:#010x} at {address:#x}")]
    UnsupportedInstruction { address: u64, word: u32 },
}

#[cfg(test)]
mod tests {
    use super::{DecodeError, InstructionClass, InstructionKind, decode_program};

    #[test]
    fn decodes_mov_add_sub_b_svc() {
        let bytes = [
            0x20, 0x00, 0x80, 0xD2, // mov x0, #1
            0x01, 0x08, 0x00, 0x91, // add x1, x0, #2
            0x22, 0x04, 0x00, 0xD1, // sub x2, x1, #1
            0x01, 0x00, 0x00, 0x14, // b +4
            0x01, 0x00, 0x00, 0xD4, // svc #0
        ];

        let decoded = decode_program(&bytes, 0x1000).expect("program should decode");

        assert_eq!(
            decoded[0].kind,
            InstructionKind::MovZ {
                rd: 0,
                imm: 1,
                shift: 0
            }
        );
        assert_eq!(
            decoded[1].kind,
            InstructionKind::AddImmediate {
                rd: 1,
                rn: 0,
                imm: 2
            }
        );
        assert_eq!(
            decoded[2].kind,
            InstructionKind::SubImmediate {
                rd: 2,
                rn: 1,
                imm: 1
            }
        );
        assert_eq!(
            decoded[3].kind,
            InstructionKind::Branch {
                offset: 4,
                target: 0x1010
            }
        );
        assert_eq!(decoded[4].kind, InstructionKind::Svc { imm: 0 });
        assert_eq!(decoded[4].class, InstructionClass::Exception);
        assert_eq!(decoded[0].raw_bytes, [0x20, 0x00, 0x80, 0xD2]);
        assert_eq!(decoded[0].disassembly, "mov x0, #0x1");
    }

    #[test]
    fn decodes_str_word_unsigned_offset() {
        // str w0, [x1, #0]
        let decoded = decode_program(&[0x20, 0x00, 0x00, 0xB9], 0x2000).expect("str should decode");
        assert_eq!(
            decoded[0].kind,
            InstructionKind::StoreWord {
                rt: 0,
                rn: 1,
                offset: 0
            }
        );
        assert_eq!(decoded[0].class, InstructionClass::LoadStore);
        assert_eq!(decoded[0].disassembly, "str w0, [x1, #0x0]");

        // str w2, [x3, #4]
        let decoded = decode_program(&[0x62, 0x04, 0x00, 0xB9], 0).expect("str should decode");
        assert_eq!(
            decoded[0].kind,
            InstructionKind::StoreWord {
                rt: 2,
                rn: 3,
                offset: 4
            }
        );
    }

    #[test]
    fn rejects_invalid_length() {
        let error = decode_program(&[0, 1], 0).expect_err("length should fail");

        assert_eq!(error, DecodeError::InvalidLength { len: 2 });
    }

    #[test]
    fn rejects_address_overflow() {
        let error = decode_program(
            &[
                0x01, 0x00, 0x00, 0xD4, // svc #0
                0x01, 0x00, 0x00, 0xD4, // svc #0
            ],
            u64::MAX - 3,
        )
        .expect_err("second instruction address should overflow");

        assert_eq!(
            error,
            DecodeError::AddressOverflow {
                base_address: u64::MAX - 3,
                instruction_index: 1
            }
        );
    }

    #[test]
    fn rejects_unsupported_instruction() {
        let error = decode_program(&[0, 0, 0, 0], 0x44).expect_err("instruction should fail");

        assert_eq!(
            error,
            DecodeError::UnsupportedInstruction {
                address: 0x44,
                word: 0
            }
        );
    }

    #[test]
    fn rejects_bl_and_disassembles_movz_xzr() {
        let bl = decode_program(&[0x00, 0x00, 0x00, 0x94], 0x1000)
            .expect_err("BL is out of scope for phase 8");
        assert_eq!(
            bl,
            DecodeError::UnsupportedInstruction {
                address: 0x1000,
                word: 0x9400_0000,
            }
        );

        let decoded = decode_program(&[0x3F, 0x00, 0x80, 0xD2], 0).expect("mov xzr should decode");
        assert_eq!(decoded[0].disassembly, "mov xzr, #0x1");
    }
}
