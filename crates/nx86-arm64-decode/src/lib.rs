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
    DataProcessingRegister,
    Branch,
    Exception,
    LoadStore,
}

/// Access width of a load/store.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MemSize {
    /// 32-bit `Wt` access.
    Word,
    /// 64-bit `Xt` access.
    Double,
}

impl MemSize {
    /// Access width in bytes.
    #[must_use]
    pub const fn bytes(self) -> u64 {
        match self {
            Self::Word => 4,
            Self::Double => 8,
        }
    }
}

/// Logical (register) operation.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LogicalOp {
    And,
    Or,
    Xor,
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
    /// `AND`/`ORR`/`EOR` (shifted register, `LSL #0`), 64-bit.
    LogicalReg {
        op: LogicalOp,
        rd: u8,
        rn: u8,
        rm: u8,
    },
    Branch {
        offset: i64,
        target: u64,
    },
    Svc {
        imm: u16,
    },
    /// `STR Wt|Xt, [Xn|SP, #offset]` (unsigned offset).
    Store {
        rt: u8,
        rn: u8,
        offset: u64,
        size: MemSize,
    },
    /// `LDR Wt|Xt, [Xn|SP, #offset]` (unsigned offset).
    Load {
        rt: u8,
        rn: u8,
        offset: u64,
        size: MemSize,
    },
}

impl InstructionKind {
    #[must_use]
    pub const fn class(&self) -> InstructionClass {
        match self {
            Self::MovZ { .. } | Self::AddImmediate { .. } | Self::SubImmediate { .. } => {
                InstructionClass::DataProcessingImmediate
            }
            Self::LogicalReg { .. } => InstructionClass::DataProcessingRegister,
            Self::Branch { .. } => InstructionClass::Branch,
            Self::Svc { .. } => InstructionClass::Exception,
            Self::Store { .. } | Self::Load { .. } => InstructionClass::LoadStore,
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
            Self::LogicalReg { op, rd, rn, rm } => {
                let mnemonic = match op {
                    LogicalOp::And => "and",
                    LogicalOp::Or => "orr",
                    LogicalOp::Xor => "eor",
                };
                format!(
                    "{mnemonic} {}, {}, {}",
                    gp_or_zr(*rd),
                    gp_or_zr(*rn),
                    gp_or_zr(*rm)
                )
            }
            Self::Branch { target, .. } => format!("b {target:#x}"),
            Self::Svc { imm } => format!("svc #{imm:#x}"),
            Self::Store {
                rt,
                rn,
                offset,
                size,
            } => {
                format!(
                    "str {}, [{}, #{offset:#x}]",
                    reg_for_size(*rt, *size),
                    gp_or_sp(*rn)
                )
            }
            Self::Load {
                rt,
                rn,
                offset,
                size,
            } => {
                format!(
                    "ldr {}, [{}, #{offset:#x}]",
                    reg_for_size(*rt, *size),
                    gp_or_sp(*rn)
                )
            }
        }
    }
}

fn reg_for_size(register: u8, size: MemSize) -> String {
    let prefix = match size {
        MemSize::Word => 'w',
        MemSize::Double => 'x',
    };
    format!("{prefix}{register}")
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

    // Logical (shifted register), 64-bit, LSL #0, N=0: AND/ORR/EOR.
    if (word & 0xFFE0_FC00) == 0x8A00_0000 {
        return Ok(logical_reg(LogicalOp::And, word));
    }
    if (word & 0xFFE0_FC00) == 0xAA00_0000 {
        return Ok(logical_reg(LogicalOp::Or, word));
    }
    if (word & 0xFFE0_FC00) == 0xCA00_0000 {
        return Ok(logical_reg(LogicalOp::Xor, word));
    }

    // STR/LDR (immediate, unsigned offset). size 10=32-bit, 11=64-bit;
    // opc 00=STR, 01=LDR. Offset is scaled by the access size.
    if (word & 0xFFC0_0000) == 0xB900_0000 {
        return Ok(load_store(false, MemSize::Word, word));
    }
    if (word & 0xFFC0_0000) == 0xF900_0000 {
        return Ok(load_store(false, MemSize::Double, word));
    }
    if (word & 0xFFC0_0000) == 0xB940_0000 {
        return Ok(load_store(true, MemSize::Word, word));
    }
    if (word & 0xFFC0_0000) == 0xF940_0000 {
        return Ok(load_store(true, MemSize::Double, word));
    }

    Err(DecodeError::UnsupportedInstruction { address, word })
}

fn logical_reg(op: LogicalOp, word: u32) -> InstructionKind {
    InstructionKind::LogicalReg {
        op,
        rd: bits(word, 0, 5) as u8,
        rn: bits(word, 5, 5) as u8,
        rm: bits(word, 16, 5) as u8,
    }
}

fn load_store(is_load: bool, size: MemSize, word: u32) -> InstructionKind {
    let rt = bits(word, 0, 5) as u8;
    let rn = bits(word, 5, 5) as u8;
    let scale = size.bytes().trailing_zeros();
    let offset = u64::from(bits(word, 10, 12)) << scale;
    if is_load {
        InstructionKind::Load {
            rt,
            rn,
            offset,
            size,
        }
    } else {
        InstructionKind::Store {
            rt,
            rn,
            offset,
            size,
        }
    }
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
    use super::{
        DecodeError, InstructionClass, InstructionKind, LogicalOp, MemSize, decode_program,
    };

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
            InstructionKind::Store {
                rt: 0,
                rn: 1,
                offset: 0,
                size: MemSize::Word,
            }
        );
        assert_eq!(decoded[0].class, InstructionClass::LoadStore);
        assert_eq!(decoded[0].disassembly, "str w0, [x1, #0x0]");

        // str w2, [x3, #4] (imm12 = 1, scaled by 4)
        let decoded = decode_program(&[0x62, 0x04, 0x00, 0xB9], 0).expect("str should decode");
        assert_eq!(
            decoded[0].kind,
            InstructionKind::Store {
                rt: 2,
                rn: 3,
                offset: 4,
                size: MemSize::Word,
            }
        );
    }

    #[test]
    fn decodes_ldr_str_double_and_ldr_word() {
        // ldr x0, [x1, #8] (size=11, opc=01, imm12=1 scaled by 8)
        let decoded = decode_program(&[0x20, 0x04, 0x40, 0xF9], 0).expect("ldr should decode");
        assert_eq!(
            decoded[0].kind,
            InstructionKind::Load {
                rt: 0,
                rn: 1,
                offset: 8,
                size: MemSize::Double,
            }
        );
        assert_eq!(decoded[0].disassembly, "ldr x0, [x1, #0x8]");

        // str x2, [x3, #0]
        let decoded = decode_program(&[0x62, 0x00, 0x00, 0xF9], 0).expect("str should decode");
        assert_eq!(
            decoded[0].kind,
            InstructionKind::Store {
                rt: 2,
                rn: 3,
                offset: 0,
                size: MemSize::Double,
            }
        );

        // ldr w4, [x5, #4]
        let decoded = decode_program(&[0xA4, 0x04, 0x40, 0xB9], 0).expect("ldr should decode");
        assert_eq!(
            decoded[0].kind,
            InstructionKind::Load {
                rt: 4,
                rn: 5,
                offset: 4,
                size: MemSize::Word,
            }
        );
    }

    #[test]
    fn decodes_logical_register_ops() {
        // and x0, x1, x2 ; orr x3, x4, x5 ; eor x6, x7, x8
        let bytes = [
            0x20, 0x00, 0x02, 0x8A, // and x0, x1, x2
            0x83, 0x00, 0x05, 0xAA, // orr x3, x4, x5
            0xE6, 0x00, 0x08, 0xCA, // eor x6, x7, x8
        ];
        let decoded = decode_program(&bytes, 0).expect("logical ops should decode");

        assert_eq!(
            decoded[0].kind,
            InstructionKind::LogicalReg {
                op: LogicalOp::And,
                rd: 0,
                rn: 1,
                rm: 2,
            }
        );
        assert_eq!(decoded[0].class, InstructionClass::DataProcessingRegister);
        assert_eq!(decoded[0].disassembly, "and x0, x1, x2");
        assert_eq!(
            decoded[1].kind,
            InstructionKind::LogicalReg {
                op: LogicalOp::Or,
                rd: 3,
                rn: 4,
                rm: 5,
            }
        );
        assert_eq!(
            decoded[2].kind,
            InstructionKind::LogicalReg {
                op: LogicalOp::Xor,
                rd: 6,
                rn: 7,
                rm: 8,
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
