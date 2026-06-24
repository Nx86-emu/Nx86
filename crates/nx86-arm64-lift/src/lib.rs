//! AArch64 → NxIR lifter (Phase 14).
//!
//! Lifts the narrow decoded instruction set into NxIR: MOV, ADD/SUB immediate,
//! logical register ops, unconditional branches, loads/stores, and the
//! synthetic `SVC` exit. It builds a basic-block CFG from branch targets and
//! verifies the result before returning it.
//!
//! Guest register state crosses blocks via `GetReg`/`SetReg`; SSA values stay
//! block-local, which keeps the v0 IR phi-free. Register 31 means the stack
//! pointer for ADD/SUB-immediate and load/store base operands, and the zero
//! register everywhere else — matching the decoder's operand conventions.

use std::collections::{BTreeSet, HashMap};

use nx86_arm64_decode::{DecodedInstruction, InstructionKind, LogicalOp, MemSize};
use nx86_ir::verify::{self, VerifyError};
use nx86_ir::{BinaryOp, Block, BlockId, FlagOp, Function, Inst, Op, Reg, Terminator, Type, Value};
use thiserror::Error;

pub mod recover;

pub use recover::{
    CodeView, EdgeKind, RecoverError, RecoveredBlock, RecoveredCfg, RecoveredFunction, recover_cfg,
};

#[derive(Debug, Error)]
pub enum LiftError {
    #[error("branch at {address:#x} targets {target:#x}, which is not an instruction boundary")]
    UnknownBranchTarget { address: u64, target: u64 },
    #[error("conditional branch at {address:#x} has no fall-through instruction")]
    DanglingConditionalBranch { address: u64 },
    #[error("lifted IR failed verification: {0}")]
    Invalid(#[from] VerifyError),
}

/// Resolve a branch target address to the block that begins there.
fn block_for_target(
    index_of: &HashMap<u64, usize>,
    block_id_of_start: &HashMap<usize, u32>,
    address: u64,
    target: u64,
) -> Result<BlockId, LiftError> {
    let target_index = index_of
        .get(&target)
        .ok_or(LiftError::UnknownBranchTarget { address, target })?;
    block_id_of_start
        .get(target_index)
        .copied()
        .map(BlockId)
        .ok_or(LiftError::UnknownBranchTarget { address, target })
}

/// Lift a decoded program into a verified NxIR function.
pub fn lift_program(
    name: &str,
    instructions: &[DecodedInstruction],
    entry_address: u64,
) -> Result<Function, LiftError> {
    let index_of: HashMap<u64, usize> = instructions
        .iter()
        .enumerate()
        .map(|(index, inst)| (inst.address, index))
        .collect();

    let block_starts = compute_block_starts(instructions, &index_of);
    let block_id_of_start: HashMap<usize, u32> = block_starts
        .iter()
        .enumerate()
        .map(|(block_index, &start)| (start, block_index as u32))
        .collect();

    let mut value_counter: u32 = 0;
    let mut blocks: Vec<Block> = Vec::new();

    for (block_index, &start) in block_starts.iter().enumerate() {
        let end = block_starts
            .get(block_index + 1)
            .copied()
            .unwrap_or(instructions.len());

        let mut block_insts: Vec<Inst> = Vec::new();
        let mut terminator: Option<(Terminator, u64)> = None;

        for (offset, inst) in instructions[start..end].iter().enumerate() {
            let index = start + offset;
            match &inst.kind {
                InstructionKind::Branch { target, .. } => {
                    let target_block =
                        block_for_target(&index_of, &block_id_of_start, inst.address, *target)?;
                    terminator = Some((
                        Terminator::Branch {
                            target: target_block,
                        },
                        inst.address,
                    ));
                    break;
                }
                InstructionKind::CondBranch { cond, target, .. } => {
                    let if_true =
                        block_for_target(&index_of, &block_id_of_start, inst.address, *target)?;
                    // The not-taken edge falls through to the next instruction,
                    // which compute_block_starts marked as a block leader.
                    let if_false = block_id_of_start
                        .get(&(index + 1))
                        .copied()
                        .map(BlockId)
                        .ok_or(LiftError::DanglingConditionalBranch {
                            address: inst.address,
                        })?;
                    terminator = Some((
                        Terminator::CondBranch {
                            cond: *cond,
                            if_true,
                            if_false,
                        },
                        inst.address,
                    ));
                    break;
                }
                InstructionKind::Svc { imm } => {
                    terminator = Some((
                        Terminator::Halt {
                            reason: format!("svc #{imm:#x}"),
                        },
                        inst.address,
                    ));
                    break;
                }
                kind => lift_data(kind, inst.address, &mut block_insts, &mut value_counter),
            }
        }

        let (terminator, terminator_address) = terminator.unwrap_or_else(|| {
            let last_address = block_insts
                .last()
                .map_or(entry_address, |inst| inst.guest_address);
            if block_index + 1 < block_starts.len() {
                (
                    Terminator::Branch {
                        target: BlockId((block_index + 1) as u32),
                    },
                    last_address,
                )
            } else {
                (Terminator::Return, last_address)
            }
        });

        blocks.push(Block {
            instructions: block_insts,
            terminator,
            terminator_address,
        });
    }

    if blocks.is_empty() {
        blocks.push(Block {
            instructions: Vec::new(),
            terminator: Terminator::Return,
            terminator_address: entry_address,
        });
    }

    let function = Function {
        name: name.to_owned(),
        entry_address,
        blocks,
        value_count: value_counter,
        deopt_points: Vec::new(),
    };
    verify::verify(&function)?;
    Ok(function)
}

/// Compute the sorted set of instruction indices that begin a basic block.
fn compute_block_starts(
    instructions: &[DecodedInstruction],
    index_of: &HashMap<u64, usize>,
) -> Vec<usize> {
    let mut leaders: BTreeSet<usize> = BTreeSet::new();
    if !instructions.is_empty() {
        leaders.insert(0);
    }
    for (index, inst) in instructions.iter().enumerate() {
        let is_terminator = match &inst.kind {
            InstructionKind::Branch { target, .. } | InstructionKind::CondBranch { target, .. } => {
                if let Some(&target_index) = index_of.get(target) {
                    leaders.insert(target_index);
                }
                true
            }
            InstructionKind::Svc { .. } => true,
            _ => false,
        };
        // The instruction after a terminator begins a new block.
        if is_terminator && index + 1 < instructions.len() {
            leaders.insert(index + 1);
        }
    }
    leaders.into_iter().collect()
}

fn lift_data(kind: &InstructionKind, address: u64, out: &mut Vec<Inst>, counter: &mut u32) {
    match kind {
        InstructionKind::MovZ { rd, imm, .. } => {
            let value = alloc(counter);
            push(
                out,
                Some(value),
                Op::Const {
                    ty: Type::I64,
                    value: *imm,
                },
                address,
            );
            push(
                out,
                None,
                Op::SetReg {
                    reg: reg_zr(*rd),
                    value,
                },
                address,
            );
        }
        InstructionKind::AddImmediate { rd, rn, imm } => {
            lift_add_sub(BinaryOp::Add, *rd, *rn, *imm, address, out, counter);
        }
        InstructionKind::SubImmediate { rd, rn, imm } => {
            lift_add_sub(BinaryOp::Sub, *rd, *rn, *imm, address, out, counter);
        }
        InstructionKind::AddsImmediate { rd, rn, imm } => {
            lift_flag_setting(FlagOp::Add, *rd, *rn, *imm, address, out, counter);
        }
        InstructionKind::SubsImmediate { rd, rn, imm } => {
            lift_flag_setting(FlagOp::Sub, *rd, *rn, *imm, address, out, counter);
        }
        InstructionKind::LogicalReg { op, rd, rn, rm } => {
            let lhs = alloc(counter);
            push(out, Some(lhs), Op::GetReg { reg: reg_zr(*rn) }, address);
            let rhs = alloc(counter);
            push(out, Some(rhs), Op::GetReg { reg: reg_zr(*rm) }, address);
            let result = alloc(counter);
            push(
                out,
                Some(result),
                Op::Binary {
                    op: logical_to_binary(*op),
                    ty: Type::I64,
                    lhs,
                    rhs,
                },
                address,
            );
            push(
                out,
                None,
                Op::SetReg {
                    reg: reg_zr(*rd),
                    value: result,
                },
                address,
            );
        }
        InstructionKind::Store {
            rt,
            rn,
            offset,
            size,
        } => {
            let addr = lift_address(*rn, *offset, address, out, counter);
            let data = alloc(counter);
            push(out, Some(data), Op::GetReg { reg: reg_zr(*rt) }, address);
            match size {
                MemSize::Word => {
                    let trunc = alloc(counter);
                    push(out, Some(trunc), Op::Trunc { value: data }, address);
                    push(
                        out,
                        None,
                        Op::Store {
                            ty: Type::I32,
                            address: addr,
                            value: trunc,
                        },
                        address,
                    );
                }
                MemSize::Double => {
                    push(
                        out,
                        None,
                        Op::Store {
                            ty: Type::I64,
                            address: addr,
                            value: data,
                        },
                        address,
                    );
                }
            }
        }
        InstructionKind::Load {
            rt,
            rn,
            offset,
            size,
        } => {
            let addr = lift_address(*rn, *offset, address, out, counter);
            match size {
                MemSize::Word => {
                    let loaded = alloc(counter);
                    push(
                        out,
                        Some(loaded),
                        Op::Load {
                            ty: Type::I32,
                            address: addr,
                        },
                        address,
                    );
                    let extended = alloc(counter);
                    push(
                        out,
                        Some(extended),
                        Op::ZeroExtend { value: loaded },
                        address,
                    );
                    push(
                        out,
                        None,
                        Op::SetReg {
                            reg: reg_zr(*rt),
                            value: extended,
                        },
                        address,
                    );
                }
                MemSize::Double => {
                    let loaded = alloc(counter);
                    push(
                        out,
                        Some(loaded),
                        Op::Load {
                            ty: Type::I64,
                            address: addr,
                        },
                        address,
                    );
                    push(
                        out,
                        None,
                        Op::SetReg {
                            reg: reg_zr(*rt),
                            value: loaded,
                        },
                        address,
                    );
                }
            }
        }
        InstructionKind::LoadExclusive { rt, rn, size } => {
            let addr = alloc(counter);
            push(out, Some(addr), Op::GetReg { reg: reg_sp(*rn) }, address);
            let (ty, loaded) = match size {
                MemSize::Word => {
                    let v = alloc(counter);
                    push(
                        out,
                        Some(v),
                        Op::LoadExclusive {
                            ty: Type::I32,
                            address: addr,
                        },
                        address,
                    );
                    let ext = alloc(counter);
                    push(out, Some(ext), Op::ZeroExtend { value: v }, address);
                    (Type::I32, ext)
                }
                MemSize::Double => {
                    let v = alloc(counter);
                    push(
                        out,
                        Some(v),
                        Op::LoadExclusive {
                            ty: Type::I64,
                            address: addr,
                        },
                        address,
                    );
                    (Type::I64, v)
                }
            };
            let _ = ty;
            push(
                out,
                None,
                Op::SetReg {
                    reg: reg_zr(*rt),
                    value: loaded,
                },
                address,
            );
        }
        InstructionKind::StoreExclusive { rs, rt, rn, size } => {
            let addr = alloc(counter);
            push(out, Some(addr), Op::GetReg { reg: reg_sp(*rn) }, address);
            let data = alloc(counter);
            push(out, Some(data), Op::GetReg { reg: reg_zr(*rt) }, address);
            let status = match size {
                MemSize::Word => {
                    let trunc = alloc(counter);
                    push(out, Some(trunc), Op::Trunc { value: data }, address);
                    let s = alloc(counter);
                    push(
                        out,
                        Some(s),
                        Op::StoreExclusive {
                            ty: Type::I32,
                            address: addr,
                            value: trunc,
                        },
                        address,
                    );
                    s
                }
                MemSize::Double => {
                    let s = alloc(counter);
                    push(
                        out,
                        Some(s),
                        Op::StoreExclusive {
                            ty: Type::I64,
                            address: addr,
                            value: data,
                        },
                        address,
                    );
                    s
                }
            };
            let ext = alloc(counter);
            push(out, Some(ext), Op::ZeroExtend { value: status }, address);
            push(
                out,
                None,
                Op::SetReg {
                    reg: reg_zr(*rs),
                    value: ext,
                },
                address,
            );
        }
        InstructionKind::LoadAcquire { rt, rn, size } => {
            let addr = alloc(counter);
            push(out, Some(addr), Op::GetReg { reg: reg_sp(*rn) }, address);
            let loaded = match size {
                MemSize::Word => {
                    let v = alloc(counter);
                    push(
                        out,
                        Some(v),
                        Op::LoadAcquire {
                            ty: Type::I32,
                            address: addr,
                        },
                        address,
                    );
                    let ext = alloc(counter);
                    push(out, Some(ext), Op::ZeroExtend { value: v }, address);
                    ext
                }
                MemSize::Double => {
                    let v = alloc(counter);
                    push(
                        out,
                        Some(v),
                        Op::LoadAcquire {
                            ty: Type::I64,
                            address: addr,
                        },
                        address,
                    );
                    v
                }
            };
            push(
                out,
                None,
                Op::SetReg {
                    reg: reg_zr(*rt),
                    value: loaded,
                },
                address,
            );
        }
        InstructionKind::StoreRelease { rt, rn, size } => {
            let addr = alloc(counter);
            push(out, Some(addr), Op::GetReg { reg: reg_sp(*rn) }, address);
            let data = alloc(counter);
            push(out, Some(data), Op::GetReg { reg: reg_zr(*rt) }, address);
            match size {
                MemSize::Word => {
                    let trunc = alloc(counter);
                    push(out, Some(trunc), Op::Trunc { value: data }, address);
                    push(
                        out,
                        None,
                        Op::StoreRelease {
                            ty: Type::I32,
                            address: addr,
                            value: trunc,
                        },
                        address,
                    );
                }
                MemSize::Double => {
                    push(
                        out,
                        None,
                        Op::StoreRelease {
                            ty: Type::I64,
                            address: addr,
                            value: data,
                        },
                        address,
                    );
                }
            }
        }
        // Terminators are handled by the caller.
        InstructionKind::Branch { .. }
        | InstructionKind::CondBranch { .. }
        | InstructionKind::Svc { .. } => {}
    }
}

/// Lift a flag-setting `ADDS`/`SUBS` (and the `CMP`/`CMN` aliases). Emits a lazy
/// `SetFlags` recording the operands, then the result write (`rd` = 31 discards,
/// matching the zero-register semantics of the S-form).
fn lift_flag_setting(
    flag: FlagOp,
    rd: u8,
    rn: u8,
    imm: u64,
    address: u64,
    out: &mut Vec<Inst>,
    counter: &mut u32,
) {
    let binary = match flag {
        FlagOp::Add => BinaryOp::Add,
        FlagOp::Sub => BinaryOp::Sub,
    };
    let lhs = alloc(counter);
    push(out, Some(lhs), Op::GetReg { reg: reg_sp(rn) }, address);
    let rhs = alloc(counter);
    push(
        out,
        Some(rhs),
        Op::Const {
            ty: Type::I64,
            value: imm,
        },
        address,
    );
    push(out, None, Op::SetFlags { op: flag, lhs, rhs }, address);
    let result = alloc(counter);
    push(
        out,
        Some(result),
        Op::Binary {
            op: binary,
            ty: Type::I64,
            lhs,
            rhs,
        },
        address,
    );
    push(
        out,
        None,
        Op::SetReg {
            reg: reg_zr(rd),
            value: result,
        },
        address,
    );
}

fn lift_add_sub(
    op: BinaryOp,
    rd: u8,
    rn: u8,
    imm: u64,
    address: u64,
    out: &mut Vec<Inst>,
    counter: &mut u32,
) {
    let lhs = alloc(counter);
    push(out, Some(lhs), Op::GetReg { reg: reg_sp(rn) }, address);
    let rhs = alloc(counter);
    push(
        out,
        Some(rhs),
        Op::Const {
            ty: Type::I64,
            value: imm,
        },
        address,
    );
    let result = alloc(counter);
    push(
        out,
        Some(result),
        Op::Binary {
            op,
            ty: Type::I64,
            lhs,
            rhs,
        },
        address,
    );
    push(
        out,
        None,
        Op::SetReg {
            reg: reg_sp(rd),
            value: result,
        },
        address,
    );
}

/// Emit `base + offset` for a load/store and return the address value.
fn lift_address(
    rn: u8,
    offset: u64,
    address: u64,
    out: &mut Vec<Inst>,
    counter: &mut u32,
) -> Value {
    let base = alloc(counter);
    push(out, Some(base), Op::GetReg { reg: reg_sp(rn) }, address);
    let off = alloc(counter);
    push(
        out,
        Some(off),
        Op::Const {
            ty: Type::I64,
            value: offset,
        },
        address,
    );
    let addr = alloc(counter);
    push(
        out,
        Some(addr),
        Op::Binary {
            op: BinaryOp::Add,
            ty: Type::I64,
            lhs: base,
            rhs: off,
        },
        address,
    );
    addr
}

fn alloc(counter: &mut u32) -> Value {
    let value = Value(*counter);
    *counter += 1;
    value
}

fn push(out: &mut Vec<Inst>, result: Option<Value>, op: Op, guest_address: u64) {
    out.push(Inst {
        result,
        op,
        guest_address,
    });
}

/// Register operand where 31 is the stack pointer (ADD/SUB immediate, memory
/// base).
const fn reg_sp(register: u8) -> Reg {
    if register == 31 {
        Reg::Sp
    } else {
        Reg::X(register)
    }
}

/// Register operand where 31 is the zero register (the evaluator treats `x31`
/// as zr).
const fn reg_zr(register: u8) -> Reg {
    Reg::X(register)
}

const fn logical_to_binary(op: LogicalOp) -> BinaryOp {
    match op {
        LogicalOp::And => BinaryOp::And,
        LogicalOp::Or => BinaryOp::Or,
        LogicalOp::Xor => BinaryOp::Xor,
    }
}

#[cfg(test)]
mod tests {
    use nx86_arm64_decode::decode_program;

    use super::lift_program;

    #[test]
    fn lifts_add_program_and_verifies() {
        // mov x0, #1 ; add x1, x0, #2 ; svc #0
        let bytes = [
            0x20, 0x00, 0x80, 0xD2, 0x01, 0x08, 0x00, 0x91, 0x01, 0x00, 0x00, 0xD4,
        ];
        let instructions = decode_program(&bytes, 0).expect("program should decode");

        let function = lift_program("test", &instructions, 0).expect("program should lift");

        assert_eq!(function.blocks.len(), 1);
        let dump = function.dump();
        assert!(dump.contains("setreg x0"));
        assert!(dump.contains("add.i64"));
        assert!(dump.contains("halt \"svc #0x0\""));
    }

    #[test]
    fn lifts_branch_into_multiple_blocks() {
        // mov x0, #1 ; b +8 ; mov x0, #2 ; svc #0
        let bytes = [
            0x20, 0x00, 0x80, 0xD2, // mov x0, #1
            0x02, 0x00, 0x00, 0x14, // b +8
            0x40, 0x00, 0x80, 0xD2, // mov x0, #2
            0x01, 0x00, 0x00, 0xD4, // svc #0
        ];
        let instructions = decode_program(&bytes, 0).expect("program should decode");

        let function = lift_program("branch", &instructions, 0).expect("program should lift");

        assert!(function.blocks.len() >= 2);
        assert!(function.dump().contains("br block"));
    }

    #[test]
    fn unknown_branch_target_is_rejected() {
        // b -4 (target outside the program)
        let bytes = [0xFF, 0xFF, 0xFF, 0x17];
        let instructions = decode_program(&bytes, 0).expect("program should decode");

        let error = lift_program("bad", &instructions, 0).expect_err("branch should be rejected");

        assert!(matches!(
            error,
            super::LiftError::UnknownBranchTarget { .. }
        ));
    }
}
