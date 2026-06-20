//! NxIR — Nx86's intermediate representation.
//!
//! This is the Phase 12 core: a small, typed, SSA-style IR that an AArch64
//! program lifts into. Computed values are SSA temporaries ([`Value`]); guest
//! register and memory state is modelled with explicit side-effecting
//! operations ([`Op::SetReg`], [`Op::Store`]) so the v0 IR stays verifiable
//! without cross-block phi nodes. Every instruction records the guest
//! instruction boundary it came from.

use std::fmt::{self, Write as _};

use serde::{Deserialize, Serialize};

/// An SSA value: defined exactly once, referenced by later instructions.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct Value(pub u32);

/// Index of a [`Block`] within a [`Function`].
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct BlockId(pub u32);

/// NxIR value types.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Type {
    I1,
    I32,
    I64,
    /// Lazy NZCV flag source (Phase 15).
    Flags,
}

/// A guest register operand. Guest GPRs and SP are 64-bit.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum Reg {
    /// General-purpose register `x0`..`x30`.
    X(u8),
    /// Stack pointer.
    Sp,
}

/// Integer binary operators.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BinaryOp {
    Add,
    Sub,
    And,
    Or,
    Xor,
}

/// An NxIR operation. Operations either define a value (e.g. [`Op::Binary`]) or
/// produce a side effect (e.g. [`Op::SetReg`]).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Op {
    /// A typed integer constant.
    Const { ty: Type, value: u64 },
    /// Read a guest register (64-bit).
    GetReg { reg: Reg },
    /// Write a guest register (64-bit). Side effect.
    SetReg { reg: Reg, value: Value },
    /// Integer binary operation; operands and result share `ty`.
    Binary {
        op: BinaryOp,
        ty: Type,
        lhs: Value,
        rhs: Value,
    },
    /// Truncate an `i64` to `i32`.
    Trunc { value: Value },
    /// Zero-extend an `i32` to `i64`.
    ZeroExtend { value: Value },
    /// Read `ty` bytes from a guest address. Memory read side effect.
    Load { ty: Type, address: Value },
    /// Write `ty` bytes to a guest address. Side effect.
    Store {
        ty: Type,
        address: Value,
        value: Value,
    },
}

impl Op {
    /// The type of the value this op defines, or `None` for pure side effects.
    #[must_use]
    pub const fn result_type(&self) -> Option<Type> {
        match self {
            Self::Const { ty, .. } | Self::Binary { ty, .. } | Self::Load { ty, .. } => Some(*ty),
            Self::GetReg { .. } | Self::ZeroExtend { .. } => Some(Type::I64),
            Self::Trunc { .. } => Some(Type::I32),
            Self::SetReg { .. } | Self::Store { .. } => None,
        }
    }

    /// Whether this op has a side effect beyond defining its value.
    #[must_use]
    pub const fn is_side_effect(&self) -> bool {
        matches!(self, Self::SetReg { .. } | Self::Store { .. })
    }

    /// The operands this op consumes, with the type each operand must have.
    #[must_use]
    pub fn operand_constraints(&self) -> Vec<(Value, Type)> {
        match self {
            Self::Const { .. } | Self::GetReg { .. } => Vec::new(),
            Self::SetReg { value, .. } => vec![(*value, Type::I64)],
            Self::Binary { ty, lhs, rhs, .. } => vec![(*lhs, *ty), (*rhs, *ty)],
            Self::Trunc { value } => vec![(*value, Type::I64)],
            Self::ZeroExtend { value } => vec![(*value, Type::I32)],
            Self::Load { address, .. } => vec![(*address, Type::I64)],
            Self::Store { ty, address, value } => vec![(*address, Type::I64), (*value, *ty)],
        }
    }

    /// The values this op reads.
    #[must_use]
    pub fn operands(&self) -> Vec<Value> {
        self.operand_constraints()
            .into_iter()
            .map(|(value, _)| value)
            .collect()
    }
}

/// One NxIR instruction: an optional SSA result, an op, and the guest address
/// it was lifted from.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct Inst {
    pub result: Option<Value>,
    pub op: Op,
    pub guest_address: u64,
}

/// How a block transfers control.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Terminator {
    /// Unconditional branch to another block.
    Branch { target: BlockId },
    /// Synthetic program exit (guest `SVC`). `pc` becomes the address after the
    /// halting instruction.
    Halt { reason: String },
    /// Return from the function.
    Return,
}

impl Terminator {
    /// The block targets this terminator may branch to.
    #[must_use]
    pub fn successors(&self) -> Vec<BlockId> {
        match self {
            Self::Branch { target } => vec![*target],
            Self::Halt { .. } | Self::Return => Vec::new(),
        }
    }
}

/// A basic block: straight-line instructions ending in one terminator.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct Block {
    pub instructions: Vec<Inst>,
    pub terminator: Terminator,
    /// Guest address of the instruction that produced the terminator.
    pub terminator_address: u64,
}

/// A lifted function: a list of blocks (block 0 is the entry) plus the number
/// of SSA values allocated.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct Function {
    pub name: String,
    pub entry_address: u64,
    pub blocks: Vec<Block>,
    pub value_count: u32,
}

impl Function {
    /// Render the function as human-readable NxIR text.
    #[must_use]
    pub fn dump(&self) -> String {
        let mut output = String::new();
        let _ = writeln!(output, "fn {} @{:#x}:", self.name, self.entry_address);
        for (index, block) in self.blocks.iter().enumerate() {
            let _ = writeln!(output, "  block{index}:");
            for inst in &block.instructions {
                let _ = writeln!(output, "    {}", format_inst(inst));
            }
            let _ = writeln!(output, "    {}", format_terminator(&block.terminator));
        }
        output
    }
}

/// A complete NxIR module.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct Module {
    pub functions: Vec<Function>,
}

impl Module {
    #[must_use]
    pub fn dump(&self) -> String {
        self.functions
            .iter()
            .map(Function::dump)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl fmt::Display for Reg {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::X(index) => write!(formatter, "x{index}"),
            Self::Sp => formatter.write_str("sp"),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "v{}", self.0)
    }
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::I1 => "i1",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::Flags => "flags",
        };
        formatter.write_str(text)
    }
}

impl fmt::Display for BinaryOp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Add => "add",
            Self::Sub => "sub",
            Self::And => "and",
            Self::Or => "or",
            Self::Xor => "xor",
        };
        formatter.write_str(text)
    }
}

fn format_inst(inst: &Inst) -> String {
    let body = format_op(&inst.op);
    match inst.result {
        Some(value) => format!("{value} = {body}"),
        None => body,
    }
}

fn format_op(op: &Op) -> String {
    match op {
        Op::Const { ty, value } => format!("const.{ty} {value:#x}"),
        Op::GetReg { reg } => format!("getreg {reg}"),
        Op::SetReg { reg, value } => format!("setreg {reg}, {value}"),
        Op::Binary { op, ty, lhs, rhs } => format!("{op}.{ty} {lhs}, {rhs}"),
        Op::Trunc { value } => format!("trunc.i32 {value}"),
        Op::ZeroExtend { value } => format!("zext.i64 {value}"),
        Op::Load { ty, address } => format!("load.{ty} [{address}]"),
        Op::Store { ty, address, value } => format!("store.{ty} [{address}], {value}"),
    }
}

fn format_terminator(terminator: &Terminator) -> String {
    match terminator {
        Terminator::Branch { target } => format!("br block{}", target.0),
        Terminator::Halt { reason } => format!("halt {reason:?}"),
        Terminator::Return => "ret".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{BinaryOp, Block, Function, Inst, Module, Op, Reg, Terminator, Type, Value};

    fn add_function() -> Function {
        // v0 = const.i64 1 ; setreg x0, v0
        // v1 = getreg x0   ; v2 = const.i64 2 ; v3 = add.i64 v1, v2 ; setreg x1, v3
        // halt "svc #0x0"
        Function {
            name: "add".to_owned(),
            entry_address: 0,
            value_count: 4,
            blocks: vec![Block {
                instructions: vec![
                    Inst {
                        result: Some(Value(0)),
                        op: Op::Const {
                            ty: Type::I64,
                            value: 1,
                        },
                        guest_address: 0x0,
                    },
                    Inst {
                        result: None,
                        op: Op::SetReg {
                            reg: Reg::X(0),
                            value: Value(0),
                        },
                        guest_address: 0x0,
                    },
                    Inst {
                        result: Some(Value(1)),
                        op: Op::GetReg { reg: Reg::X(0) },
                        guest_address: 0x4,
                    },
                    Inst {
                        result: Some(Value(2)),
                        op: Op::Const {
                            ty: Type::I64,
                            value: 2,
                        },
                        guest_address: 0x4,
                    },
                    Inst {
                        result: Some(Value(3)),
                        op: Op::Binary {
                            op: BinaryOp::Add,
                            ty: Type::I64,
                            lhs: Value(1),
                            rhs: Value(2),
                        },
                        guest_address: 0x4,
                    },
                    Inst {
                        result: None,
                        op: Op::SetReg {
                            reg: Reg::X(1),
                            value: Value(3),
                        },
                        guest_address: 0x4,
                    },
                ],
                terminator: Terminator::Halt {
                    reason: "svc #0x0".to_owned(),
                },
                terminator_address: 0x8,
            }],
        }
    }

    #[test]
    fn module_round_trips_through_serde() {
        let module = Module {
            functions: vec![add_function()],
        };

        let json = serde_json::to_string(&module).expect("module should serialize");
        let decoded: Module = serde_json::from_str(&json).expect("module should deserialize");

        assert_eq!(decoded, module);
    }

    #[test]
    fn dump_renders_expected_text() {
        let dump = add_function().dump();

        assert!(dump.contains("fn add @0x0:"));
        assert!(dump.contains("v0 = const.i64 0x1"));
        assert!(dump.contains("setreg x0, v0"));
        assert!(dump.contains("v3 = add.i64 v1, v2"));
        assert!(dump.contains("halt \"svc #0x0\""));
    }

    #[test]
    fn op_metadata_is_consistent() {
        let binary = Op::Binary {
            op: BinaryOp::Add,
            ty: Type::I64,
            lhs: Value(1),
            rhs: Value(2),
        };
        assert_eq!(binary.result_type(), Some(Type::I64));
        assert_eq!(binary.operands(), vec![Value(1), Value(2)]);
        assert!(!binary.is_side_effect());

        let store = Op::Store {
            ty: Type::I32,
            address: Value(5),
            value: Value(6),
        };
        assert_eq!(store.result_type(), None);
        assert!(store.is_side_effect());
        assert_eq!(
            store.operand_constraints(),
            vec![(Value(5), Type::I64), (Value(6), Type::I32)]
        );
    }

    #[test]
    fn two_block_function_lists_successors() {
        let function = Function {
            name: "branch".to_owned(),
            entry_address: 0,
            value_count: 0,
            blocks: vec![
                Block {
                    instructions: Vec::new(),
                    terminator: Terminator::Branch {
                        target: super::BlockId(1),
                    },
                    terminator_address: 0x0,
                },
                Block {
                    instructions: Vec::new(),
                    terminator: Terminator::Return,
                    terminator_address: 0x4,
                },
            ],
        };

        assert_eq!(
            function.blocks[0].terminator.successors(),
            vec![super::BlockId(1)]
        );
        assert!(function.blocks[1].terminator.successors().is_empty());
        assert!(function.dump().contains("br block1"));
    }
}
