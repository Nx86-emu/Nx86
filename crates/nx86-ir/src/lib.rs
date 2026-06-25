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

pub mod verify;

/// AArch64 condition codes, re-exported so NxIR consumers can name conditions
/// without depending on `nx86-core` directly.
pub use nx86_core::guest::Cond;

/// An SSA value: defined exactly once, referenced by later instructions.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct Value(pub u32);

/// Index of a [`Block`] within a [`Function`].
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct BlockId(pub u32);

/// Index of a [`DeoptPoint`] within a [`Function`]'s deopt table.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct DeoptId(pub u32);

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

/// The flag-producing operation recorded by a lazy [`Op::SetFlags`].
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FlagOp {
    Add,
    Sub,
}

/// AArch64 barrier instruction kind preserved in NxIR.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BarrierKind {
    Dmb,
    Dsb,
    Isb,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FpPrecision {
    F64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FpBinaryOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum VectorArrangement {
    TwoD,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum VectorBinaryOp {
    AddI64,
    AddF64,
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
    /// Lazily record the NZCV flag source for `lhs <op> rhs` (Phase 15). NZCV is
    /// not computed here; a later flag consumer materializes it. Side effect.
    SetFlags { op: FlagOp, lhs: Value, rhs: Value },
    /// Load exclusive: read memory and set the exclusive monitor. Defines a
    /// value of `ty`. Side effect (sets monitor).
    LoadExclusive { ty: Type, address: Value },
    /// Store exclusive: check monitor, write if valid. Defines an `I32` status
    /// (0 = success, 1 = failure). Side effect (clears monitor, may write).
    StoreExclusive {
        ty: Type,
        address: Value,
        value: Value,
    },
    /// Load-acquire: read memory with acquire ordering (v0: plain read).
    /// Defines a value of `ty`. Side effect.
    LoadAcquire { ty: Type, address: Value },
    /// Store-release: write memory with release ordering (v0: plain write).
    /// Side effect.
    StoreRelease {
        ty: Type,
        address: Value,
        value: Value,
    },
    /// Barrier ordering marker. Phase 35 preserves kind+option as an explicit
    /// side effect; runtime execution is a no-op until threading/MMIO/native
    /// semantics arrive.
    Barrier { kind: BarrierKind, option: u8 },
    /// Scalar floating-point immediate move into a SIMD/FP register.
    FpMoveImmediate {
        precision: FpPrecision,
        rd: u8,
        bits: u64,
    },
    /// Scalar floating-point binary op over SIMD/FP register low lanes.
    FpScalarBinary {
        op: FpBinaryOp,
        precision: FpPrecision,
        rd: u8,
        rn: u8,
        rm: u8,
    },
    /// Scalar floating-point compare. Side effect: materializes NZCV.
    FpCompare {
        precision: FpPrecision,
        rn: u8,
        rm: u8,
    },
    /// Basic vector operation over SIMD/FP registers.
    VectorBinary {
        op: VectorBinaryOp,
        arrangement: VectorArrangement,
        rd: u8,
        rn: u8,
        rm: u8,
    },
}

impl Op {
    /// The type of the value this op defines, or `None` for pure side effects.
    #[must_use]
    pub const fn result_type(&self) -> Option<Type> {
        match self {
            Self::Const { ty, .. }
            | Self::Binary { ty, .. }
            | Self::Load { ty, .. }
            | Self::LoadExclusive { ty, .. }
            | Self::LoadAcquire { ty, .. } => Some(*ty),
            Self::GetReg { .. } | Self::ZeroExtend { .. } => Some(Type::I64),
            Self::Trunc { .. } | Self::StoreExclusive { .. } => Some(Type::I32),
            Self::SetReg { .. }
            | Self::Store { .. }
            | Self::SetFlags { .. }
            | Self::StoreRelease { .. }
            | Self::Barrier { .. }
            | Self::FpMoveImmediate { .. }
            | Self::FpScalarBinary { .. }
            | Self::FpCompare { .. }
            | Self::VectorBinary { .. } => None,
        }
    }

    /// Whether this op has a side effect beyond defining its value.
    #[must_use]
    pub const fn is_side_effect(&self) -> bool {
        matches!(
            self,
            Self::SetReg { .. }
                | Self::Store { .. }
                | Self::SetFlags { .. }
                | Self::LoadExclusive { .. }
                | Self::StoreExclusive { .. }
                | Self::LoadAcquire { .. }
                | Self::StoreRelease { .. }
                | Self::Barrier { .. }
                | Self::FpMoveImmediate { .. }
                | Self::FpScalarBinary { .. }
                | Self::FpCompare { .. }
                | Self::VectorBinary { .. }
        )
    }

    /// The operands this op consumes, with the type each operand must have.
    #[must_use]
    pub fn operand_constraints(&self) -> Vec<(Value, Type)> {
        match self {
            Self::Const { .. }
            | Self::GetReg { .. }
            | Self::Barrier { .. }
            | Self::FpMoveImmediate { .. }
            | Self::FpScalarBinary { .. }
            | Self::FpCompare { .. }
            | Self::VectorBinary { .. } => Vec::new(),
            Self::SetReg { value, .. } => vec![(*value, Type::I64)],
            Self::Binary { ty, lhs, rhs, .. } => vec![(*lhs, *ty), (*rhs, *ty)],
            Self::Trunc { value } => vec![(*value, Type::I64)],
            Self::ZeroExtend { value } => vec![(*value, Type::I32)],
            Self::Load { address, .. } => vec![(*address, Type::I64)],
            Self::Store { ty, address, value } => vec![(*address, Type::I64), (*value, *ty)],
            Self::SetFlags { lhs, rhs, .. } => vec![(*lhs, Type::I64), (*rhs, Type::I64)],
            Self::LoadExclusive { address, .. } | Self::LoadAcquire { address, .. } => {
                vec![(*address, Type::I64)]
            }
            Self::StoreExclusive { ty, address, value } => {
                vec![(*address, Type::I64), (*value, *ty)]
            }
            Self::StoreRelease { ty, address, value } => {
                vec![(*address, Type::I64), (*value, *ty)]
            }
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
    /// Conditional branch that materializes NZCV from the current lazy flag
    /// source and tests `cond` (Phase 15).
    CondBranch {
        cond: Cond,
        if_true: BlockId,
        if_false: BlockId,
    },
    /// Synthetic program exit (guest `SVC`). `pc` becomes the address after the
    /// halting instruction.
    Halt { reason: String },
    /// Speculative guard (Phase 28). Materializes NZCV from the current lazy flag
    /// source and tests `cond`. If it holds, control continues to `if_pass`;
    /// otherwise the guard fails and control side-exits to the deopt point
    /// `deopt`, which reconstructs guest-visible state and resumes. A guard is a
    /// terminator because a failed guard transfers control, and NxIR blocks carry
    /// exactly one terminator.
    Guard {
        cond: Cond,
        if_pass: BlockId,
        deopt: DeoptId,
    },
    /// Return from the function.
    Return,
}

impl Terminator {
    /// The block targets this terminator may branch to. The deopt side-exit of a
    /// [`Self::Guard`] is **not** a block successor — it leaves the function for
    /// the deopt handler, like [`Self::Halt`].
    #[must_use]
    pub fn successors(&self) -> Vec<BlockId> {
        match self {
            Self::Branch { target } => vec![*target],
            Self::CondBranch {
                if_true, if_false, ..
            } => vec![*if_true, *if_false],
            Self::Guard { if_pass, .. } => vec![*if_pass],
            Self::Halt { .. } | Self::Return => Vec::new(),
        }
    }
}

/// A deopt point: where speculative execution recovers when a [`Terminator::Guard`]
/// fails. v0 records the guest PC to resume at plus a human-readable reason; the
/// live evaluator already holds the rest of the guest-visible state. Full
/// register/slot reconstruction metadata and object/sidecar persistence arrive
/// with native guard emission in a later phase.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct DeoptPoint {
    /// Guest PC the deopt handler resumes execution at.
    pub resume_pc: u64,
    /// Why this deopt point exists (for diagnostics and the Inspector).
    pub reason: String,
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

impl Block {
    /// Guest PC used to enter this block.
    #[must_use]
    pub fn entry_address(&self) -> u64 {
        self.instructions
            .first()
            .map_or(self.terminator_address, |inst| inst.guest_address)
    }
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
    /// Deopt table: side-exit recovery points referenced by [`Terminator::Guard`]
    /// via [`DeoptId`]. Empty for functions without guards (lifted code never
    /// emits guards; they come from later speculative optimization).
    #[serde(default)]
    pub deopt_points: Vec<DeoptPoint>,
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
        for (index, point) in self.deopt_points.iter().enumerate() {
            let _ = writeln!(
                output,
                "  deopt{index}: resume @{:#x} {:?}",
                point.resume_pc, point.reason
            );
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

impl fmt::Display for BarrierKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Dmb => "dmb",
            Self::Dsb => "dsb",
            Self::Isb => "isb",
        };
        formatter.write_str(text)
    }
}

impl fmt::Display for FpPrecision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::F64 => formatter.write_str("f64"),
        }
    }
}

impl fmt::Display for FpBinaryOp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Add => "fadd",
            Self::Sub => "fsub",
            Self::Mul => "fmul",
            Self::Div => "fdiv",
        };
        formatter.write_str(text)
    }
}

impl fmt::Display for VectorArrangement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TwoD => formatter.write_str("2d"),
        }
    }
}

impl fmt::Display for VectorBinaryOp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::AddI64 => "add.i64",
            Self::AddF64 => "fadd.f64",
        };
        formatter.write_str(text)
    }
}

#[must_use]
pub const fn barrier_option_name(option: u8) -> Option<&'static str> {
    match option & 0x0F {
        0x1 => Some("oshld"),
        0x2 => Some("oshst"),
        0x3 => Some("osh"),
        0x5 => Some("nshld"),
        0x6 => Some("nshst"),
        0x7 => Some("nsh"),
        0x9 => Some("ishld"),
        0xA => Some("ishst"),
        0xB => Some("ish"),
        0xD => Some("ld"),
        0xE => Some("st"),
        0xF => Some("sy"),
        _ => None,
    }
}

fn format_barrier_option(option: u8) -> String {
    barrier_option_name(option)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("#0x{:x}", option & 0x0F))
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
        Op::SetFlags { op, lhs, rhs } => {
            let mnemonic = match op {
                FlagOp::Add => "adds",
                FlagOp::Sub => "subs",
            };
            format!("setflags.{mnemonic} {lhs}, {rhs}")
        }
        Op::LoadExclusive { ty, address } => format!("ldxr.{ty} [{address}]"),
        Op::StoreExclusive { ty, address, value } => {
            format!("stxr.{ty} [{address}], {value}")
        }
        Op::LoadAcquire { ty, address } => format!("ldar.{ty} [{address}]"),
        Op::StoreRelease { ty, address, value } => {
            format!("stlr.{ty} [{address}], {value}")
        }
        Op::Barrier { kind, option } => {
            format!("barrier.{kind} {}", format_barrier_option(*option))
        }
        Op::FpMoveImmediate {
            precision,
            rd,
            bits,
        } => {
            format!("fmov.{precision} v{rd}, bits {bits:#x}")
        }
        Op::FpScalarBinary {
            op,
            precision,
            rd,
            rn,
            rm,
        } => {
            format!("{op}.{precision} v{rd}, v{rn}, v{rm}")
        }
        Op::FpCompare { precision, rn, rm } => format!("fcmp.{precision} v{rn}, v{rm}"),
        Op::VectorBinary {
            op,
            arrangement,
            rd,
            rn,
            rm,
        } => {
            format!("vec.{op}.{arrangement} v{rd}, v{rn}, v{rm}")
        }
    }
}

fn format_terminator(terminator: &Terminator) -> String {
    match terminator {
        Terminator::Branch { target } => format!("br block{}", target.0),
        Terminator::CondBranch {
            cond,
            if_true,
            if_false,
        } => format!(
            "br.{} block{} else block{}",
            cond.suffix(),
            if_true.0,
            if_false.0
        ),
        Terminator::Halt { reason } => format!("halt {reason:?}"),
        Terminator::Guard {
            cond,
            if_pass,
            deopt,
        } => format!(
            "guard.{} block{} else deopt{}",
            cond.suffix(),
            if_pass.0,
            deopt.0
        ),
        Terminator::Return => "ret".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BarrierKind, BinaryOp, Block, FpBinaryOp, FpPrecision, Function, Inst, Module, Op, Reg,
        Terminator, Type, Value, VectorArrangement, VectorBinaryOp, barrier_option_name,
    };

    fn add_function() -> Function {
        // v0 = const.i64 1 ; setreg x0, v0
        // v1 = getreg x0   ; v2 = const.i64 2 ; v3 = add.i64 v1, v2 ; setreg x1, v3
        // halt "svc #0x0"
        Function {
            name: "add".to_owned(),
            entry_address: 0,
            value_count: 4,
            deopt_points: Vec::new(),
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
    fn guard_and_deopt_table_render_and_round_trip() {
        use super::{BlockId, Cond, DeoptId, DeoptPoint};

        let function = Function {
            name: "guarded".to_owned(),
            entry_address: 0,
            value_count: 0,
            deopt_points: vec![DeoptPoint {
                resume_pc: 0x1234,
                reason: "guard:eq".to_owned(),
            }],
            blocks: vec![
                Block {
                    instructions: Vec::new(),
                    terminator: Terminator::Guard {
                        cond: Cond::Eq,
                        if_pass: BlockId(1),
                        deopt: DeoptId(0),
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

        let dump = function.dump();
        assert!(dump.contains("guard.eq block1 else deopt0"), "{dump}");
        assert!(
            dump.contains("deopt0: resume @0x1234 \"guard:eq\""),
            "{dump}"
        );
        // The guard's pass edge is its only block successor; the deopt is a side-exit.
        assert_eq!(function.blocks[0].terminator.successors(), vec![BlockId(1)]);

        let json = serde_json::to_string(&function).expect("function should serialize");
        let decoded: Function = serde_json::from_str(&json).expect("function should deserialize");
        assert_eq!(decoded, function);
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
    fn barrier_op_is_side_effecting_resultless_and_serializable() {
        let op = Op::Barrier {
            kind: BarrierKind::Dmb,
            option: 0xA,
        };

        assert_eq!(op.result_type(), None);
        assert!(op.is_side_effect());
        assert!(op.operand_constraints().is_empty());
        assert_eq!(barrier_option_name(0xA), Some("ishst"));

        let inst = Inst {
            result: None,
            op: op.clone(),
            guest_address: 0x40,
        };
        assert_eq!(super::format_inst(&inst), "barrier.dmb ishst");

        let json = serde_json::to_string(&op).expect("barrier op should serialize");
        let decoded: Op = serde_json::from_str(&json).expect("barrier op should deserialize");
        assert_eq!(decoded, op);
    }

    #[test]
    fn fp_and_vector_ops_are_side_effecting_and_serializable() {
        let ops = [
            Op::FpMoveImmediate {
                precision: FpPrecision::F64,
                rd: 0,
                bits: 1.0f64.to_bits(),
            },
            Op::FpScalarBinary {
                op: FpBinaryOp::Add,
                precision: FpPrecision::F64,
                rd: 2,
                rn: 0,
                rm: 1,
            },
            Op::FpCompare {
                precision: FpPrecision::F64,
                rn: 0,
                rm: 1,
            },
            Op::VectorBinary {
                op: VectorBinaryOp::AddF64,
                arrangement: VectorArrangement::TwoD,
                rd: 3,
                rn: 0,
                rm: 1,
            },
        ];

        for op in ops {
            assert_eq!(op.result_type(), None);
            assert!(op.is_side_effect());
            assert!(op.operand_constraints().is_empty());
            let json = serde_json::to_string(&op).expect("op should serialize");
            let decoded: Op = serde_json::from_str(&json).expect("op should deserialize");
            assert_eq!(decoded, op);
        }
        assert_eq!(
            super::format_op(&Op::VectorBinary {
                op: VectorBinaryOp::AddI64,
                arrangement: VectorArrangement::TwoD,
                rd: 2,
                rn: 0,
                rm: 1,
            }),
            "vec.add.i64.2d v2, v0, v1"
        );
    }

    #[test]
    fn set_flags_and_cond_branch_dump_and_successors() {
        use super::{Cond, FlagOp};

        let function = Function {
            name: "cmp".to_owned(),
            entry_address: 0,
            value_count: 2,
            deopt_points: Vec::new(),
            blocks: vec![
                Block {
                    instructions: vec![
                        Inst {
                            result: Some(Value(0)),
                            op: Op::GetReg { reg: Reg::X(0) },
                            guest_address: 0x0,
                        },
                        Inst {
                            result: Some(Value(1)),
                            op: Op::Const {
                                ty: Type::I64,
                                value: 1,
                            },
                            guest_address: 0x0,
                        },
                        Inst {
                            result: None,
                            op: Op::SetFlags {
                                op: FlagOp::Sub,
                                lhs: Value(0),
                                rhs: Value(1),
                            },
                            guest_address: 0x0,
                        },
                    ],
                    terminator: Terminator::CondBranch {
                        cond: Cond::Eq,
                        if_true: super::BlockId(1),
                        if_false: super::BlockId(0),
                    },
                    terminator_address: 0x4,
                },
                Block {
                    instructions: Vec::new(),
                    terminator: Terminator::Return,
                    terminator_address: 0x8,
                },
            ],
        };

        let dump = function.dump();
        assert!(dump.contains("setflags.subs v0, v1"));
        assert!(dump.contains("br.eq block1 else block0"));
        assert_eq!(
            function.blocks[0].terminator.successors(),
            vec![super::BlockId(1), super::BlockId(0)]
        );

        let json = serde_json::to_string(&function).expect("function should serialize");
        let decoded: Function = serde_json::from_str(&json).expect("function should deserialize");
        assert_eq!(decoded, function);
    }

    #[test]
    fn two_block_function_lists_successors() {
        let function = Function {
            name: "branch".to_owned(),
            entry_address: 0,
            value_count: 0,
            deopt_points: Vec::new(),
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
