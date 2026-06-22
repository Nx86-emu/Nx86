//! NxIR evaluator (Phase 14).
//!
//! A reference interpreter over verified NxIR that produces a [`CpuState`]. It
//! is the differential partner of the AArch64 [`TinyInterpreter`](crate::TinyInterpreter):
//! the two engines must agree on the final guest state and
//! memory for every synthetic program.

use nx86_core::guest::{CpuState, Nzcv};
use nx86_ir::{BinaryOp, DeoptId, FlagOp, Function, Inst, Op, Reg, Terminator, Type, Value};
use nx86_vmm::{GuestAddress, GuestMemory, VmmFault};
use thiserror::Error;

/// A lazily-recorded NZCV flag source: the operation and its operands.
type FlagSource = (FlagOp, u64, u64);

/// Maximum number of block executions before evaluation is abandoned.
const STEP_LIMIT: usize = 100_000;

#[derive(Debug, Error)]
pub enum EvalError {
    #[error("nxir evaluation exceeded {max_steps} block steps")]
    StepLimit { max_steps: usize },
    #[error("nxir referenced block{block} which does not exist")]
    BlockOutOfRange { block: usize },
    #[error("nxir referenced undefined value {value}")]
    ValueOutOfRange { value: Value },
    #[error("nxir memory op used unsupported type {ty}")]
    UnsupportedMemoryType { ty: Type },
    #[error("memory fault: {0}")]
    Memory(#[from] VmmFault),
    #[error("guard failed but deopt{} does not exist", deopt.0)]
    DeoptFailure { deopt: DeoptId },
}

/// How NxIR evaluation finished. Deopt is a translator-internal recovery exit
/// (a failed speculative guard), not guest-visible architectural state, so it is
/// reported here rather than on [`CpuState`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvalOutcome {
    /// The program exited normally — a guest `SVC` halt or a function return.
    Exit(CpuState),
    /// A guard failed and control routed to its deopt point. `state` is the
    /// reconstructed guest-visible state at the deopt point's `resume_pc`.
    Deopt {
        state: CpuState,
        deopt: DeoptId,
        reason: String,
    },
}

impl EvalOutcome {
    /// The reconstructed guest state, regardless of how evaluation finished.
    #[must_use]
    pub const fn state(&self) -> &CpuState {
        match self {
            Self::Exit(state) | Self::Deopt { state, .. } => state,
        }
    }

    /// Consume the outcome and take the reconstructed guest state.
    #[must_use]
    pub fn into_state(self) -> CpuState {
        match self {
            Self::Exit(state) | Self::Deopt { state, .. } => state,
        }
    }

    /// Whether evaluation finished by routing a failed guard to its deopt point.
    #[must_use]
    pub const fn is_deopt(&self) -> bool {
        matches!(self, Self::Deopt { .. })
    }
}

/// Evaluate a verified NxIR function against `memory`, returning how it finished
/// (a normal exit or a guard-driven deopt) and the reconstructed guest state.
pub fn evaluate(function: &Function, memory: &mut GuestMemory) -> Result<EvalOutcome, EvalError> {
    let mut cpu = CpuState::new();
    cpu.set_pc(function.entry_address);
    let mut values = vec![0u64; function.value_count as usize];
    let mut pending_flags: Option<FlagSource> = None;
    let mut block_index = 0usize;

    for _ in 0..STEP_LIMIT {
        let block = function
            .blocks
            .get(block_index)
            .ok_or(EvalError::BlockOutOfRange { block: block_index })?;
        for inst in &block.instructions {
            execute_op(inst, &mut cpu, &mut values, memory, &mut pending_flags)?;
        }
        match &block.terminator {
            Terminator::Branch { target } => block_index = target.0 as usize,
            Terminator::CondBranch {
                cond,
                if_true,
                if_false,
            } => {
                // Materialize NZCV from the lazy source only when a branch reads
                // it, then record it so the architectural flags stay observable.
                let nzcv = materialize(pending_flags);
                cpu.set_nzcv(nzcv);
                let taken = if nzcv.satisfies(*cond) {
                    *if_true
                } else {
                    *if_false
                };
                block_index = taken.0 as usize;
            }
            Terminator::Guard {
                cond,
                if_pass,
                deopt,
            } => {
                // Same lazy-flag materialization as a conditional branch. If the
                // guard holds, fall through to `if_pass`; otherwise it failed and
                // control side-exits to the deopt handler.
                let nzcv = materialize(pending_flags);
                cpu.set_nzcv(nzcv);
                if nzcv.satisfies(*cond) {
                    block_index = if_pass.0 as usize;
                } else {
                    // Route to the deopt point, reconstructing the resume PC. A
                    // missing point is a deopt failure: crash loudly rather than
                    // continue with unrecovered state (SPEC §20.4). A verified
                    // function can never reach this — the verifier range-checks
                    // every guard's `DeoptId`.
                    let point = function
                        .deopt_points
                        .get(deopt.0 as usize)
                        .ok_or(EvalError::DeoptFailure { deopt: *deopt })?;
                    cpu.set_pc(point.resume_pc);
                    return Ok(EvalOutcome::Deopt {
                        state: cpu,
                        deopt: *deopt,
                        reason: point.reason.clone(),
                    });
                }
            }
            Terminator::Halt { reason } => {
                cpu.set_nzcv(materialize(pending_flags));
                cpu.set_pc(block.terminator_address + 4);
                cpu.halt(reason.clone());
                return Ok(EvalOutcome::Exit(cpu));
            }
            Terminator::Return => {
                cpu.set_nzcv(materialize(pending_flags));
                return Ok(EvalOutcome::Exit(cpu));
            }
        }
    }

    Err(EvalError::StepLimit {
        max_steps: STEP_LIMIT,
    })
}

/// Materialize NZCV from a lazily-recorded flag source.
fn materialize(pending: Option<FlagSource>) -> Nzcv {
    match pending {
        Some((FlagOp::Add, lhs, rhs)) => Nzcv::from_add(lhs, rhs),
        Some((FlagOp::Sub, lhs, rhs)) => Nzcv::from_sub(lhs, rhs),
        None => Nzcv::default(),
    }
}

fn execute_op(
    inst: &Inst,
    cpu: &mut CpuState,
    values: &mut [u64],
    memory: &mut GuestMemory,
    pending_flags: &mut Option<FlagSource>,
) -> Result<(), EvalError> {
    let computed: Option<u64> = match &inst.op {
        Op::Const { ty, value } => Some(mask(*ty, *value)),
        Op::GetReg { reg } => Some(read_reg(cpu, *reg)),
        Op::SetReg { reg, value } => {
            let resolved = value_at(values, *value)?;
            write_reg(cpu, *reg, resolved);
            None
        }
        Op::Binary { op, ty, lhs, rhs } => {
            let a = value_at(values, *lhs)?;
            let b = value_at(values, *rhs)?;
            Some(mask(*ty, binary(*op, *ty, a, b)))
        }
        Op::Trunc { value } => Some(value_at(values, *value)? & 0xFFFF_FFFF),
        Op::ZeroExtend { value } => Some(value_at(values, *value)? & 0xFFFF_FFFF),
        Op::Load { ty, address } => {
            let resolved = value_at(values, *address)?;
            Some(load_mem(memory, *ty, resolved)?)
        }
        Op::Store { ty, address, value } => {
            let resolved_address = value_at(values, *address)?;
            let resolved_value = value_at(values, *value)?;
            store_mem(memory, *ty, resolved_address, resolved_value)?;
            None
        }
        Op::SetFlags { op, lhs, rhs } => {
            // Lazy: record the source; NZCV is materialized only when consumed.
            let a = value_at(values, *lhs)?;
            let b = value_at(values, *rhs)?;
            *pending_flags = Some((*op, a, b));
            None
        }
    };

    if let (Some(value), Some(result)) = (inst.result, computed) {
        let slot = values
            .get_mut(value.0 as usize)
            .ok_or(EvalError::ValueOutOfRange { value })?;
        *slot = result;
    }
    Ok(())
}

fn value_at(values: &[u64], value: Value) -> Result<u64, EvalError> {
    values
        .get(value.0 as usize)
        .copied()
        .ok_or(EvalError::ValueOutOfRange { value })
}

const fn mask(ty: Type, value: u64) -> u64 {
    match ty {
        Type::I64 | Type::Flags => value,
        Type::I32 => value & 0xFFFF_FFFF,
        Type::I1 => value & 1,
    }
}

fn read_reg(cpu: &CpuState, reg: Reg) -> u64 {
    match reg {
        Reg::X(index) => cpu.x(index),
        Reg::Sp => cpu.sp(),
    }
}

fn write_reg(cpu: &mut CpuState, reg: Reg, value: u64) {
    match reg {
        Reg::X(index) => cpu.set_x(index, value),
        Reg::Sp => cpu.set_sp(value),
    }
}

fn binary(op: BinaryOp, ty: Type, lhs: u64, rhs: u64) -> u64 {
    if matches!(ty, Type::I32) {
        let a = lhs as u32;
        let b = rhs as u32;
        let result = match op {
            BinaryOp::Add => a.wrapping_add(b),
            BinaryOp::Sub => a.wrapping_sub(b),
            BinaryOp::And => a & b,
            BinaryOp::Or => a | b,
            BinaryOp::Xor => a ^ b,
        };
        return u64::from(result);
    }

    match op {
        BinaryOp::Add => lhs.wrapping_add(rhs),
        BinaryOp::Sub => lhs.wrapping_sub(rhs),
        BinaryOp::And => lhs & rhs,
        BinaryOp::Or => lhs | rhs,
        BinaryOp::Xor => lhs ^ rhs,
    }
}

fn load_mem(memory: &GuestMemory, ty: Type, address: u64) -> Result<u64, EvalError> {
    match ty {
        Type::I32 => {
            let bytes = memory.read(GuestAddress(address), 4)?;
            let mut word = [0u8; 4];
            word.copy_from_slice(&bytes);
            Ok(u64::from(u32::from_le_bytes(word)))
        }
        Type::I64 => {
            let bytes = memory.read(GuestAddress(address), 8)?;
            let mut word = [0u8; 8];
            word.copy_from_slice(&bytes);
            Ok(u64::from_le_bytes(word))
        }
        other => Err(EvalError::UnsupportedMemoryType { ty: other }),
    }
}

fn store_mem(
    memory: &mut GuestMemory,
    ty: Type,
    address: u64,
    value: u64,
) -> Result<(), EvalError> {
    match ty {
        Type::I32 => memory.write(GuestAddress(address), &(value as u32).to_le_bytes())?,
        Type::I64 => memory.write(GuestAddress(address), &value.to_le_bytes())?,
        other => return Err(EvalError::UnsupportedMemoryType { ty: other }),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use nx86_ir::{
        Block, BlockId, Cond, DeoptId, DeoptPoint, FlagOp, Function, Inst, Op, Terminator, Type,
        Value,
    };
    use nx86_vmm::GuestMemory;

    use super::{EvalError, EvalOutcome, evaluate};

    const DEOPT_RESUME_PC: u64 = 0x2000;

    /// A function that compares two constants and guards on equality:
    ///
    /// ```text
    /// block0: v0 = const a; v1 = const b; setflags.subs v0, v1;
    ///         guard.eq block1 else deopt0
    /// block1: halt "passed"
    /// ```
    ///
    /// When `with_point` is false the deopt table is empty, so a failing guard
    /// has nowhere to recover (a deopt failure).
    fn guard_eq_function(a: u64, b: u64, deopt: DeoptId, with_point: bool) -> Function {
        let deopt_points = if with_point {
            vec![DeoptPoint {
                resume_pc: DEOPT_RESUME_PC,
                reason: "guard:eq".to_owned(),
            }]
        } else {
            Vec::new()
        };
        Function {
            name: "guarded".to_owned(),
            entry_address: 0,
            value_count: 2,
            deopt_points,
            blocks: vec![
                Block {
                    instructions: vec![
                        Inst {
                            result: Some(Value(0)),
                            op: Op::Const {
                                ty: Type::I64,
                                value: a,
                            },
                            guest_address: 0,
                        },
                        Inst {
                            result: Some(Value(1)),
                            op: Op::Const {
                                ty: Type::I64,
                                value: b,
                            },
                            guest_address: 0,
                        },
                        Inst {
                            result: None,
                            op: Op::SetFlags {
                                op: FlagOp::Sub,
                                lhs: Value(0),
                                rhs: Value(1),
                            },
                            guest_address: 0,
                        },
                    ],
                    terminator: Terminator::Guard {
                        cond: Cond::Eq,
                        if_pass: BlockId(1),
                        deopt,
                    },
                    terminator_address: 0,
                },
                Block {
                    instructions: Vec::new(),
                    terminator: Terminator::Halt {
                        reason: "passed".to_owned(),
                    },
                    terminator_address: 0x4,
                },
            ],
        }
    }

    #[test]
    fn guard_that_holds_continues_to_pass_block() {
        let function = guard_eq_function(5, 5, DeoptId(0), true);
        let mut memory = GuestMemory::new_logical();

        let outcome = evaluate(&function, &mut memory).expect("evaluation should succeed");

        assert!(!outcome.is_deopt());
        assert!(outcome.state().halted());
        assert_eq!(outcome.state().halt_reason(), Some("passed"));
    }

    #[test]
    fn failed_guard_routes_to_deopt_handler() {
        let function = guard_eq_function(5, 3, DeoptId(0), true);
        let mut memory = GuestMemory::new_logical();

        let outcome = evaluate(&function, &mut memory).expect("evaluation should succeed");

        let EvalOutcome::Deopt {
            state,
            deopt,
            reason,
        } = outcome
        else {
            panic!("expected a deopt, got {outcome:?}");
        };
        assert_eq!(deopt, DeoptId(0));
        assert_eq!(reason, "guard:eq");
        assert_eq!(state.pc(), DEOPT_RESUME_PC);
        assert!(!state.halted());
    }

    #[test]
    fn missing_deopt_point_is_a_deopt_failure() {
        // The guard fails (5 != 3) and references deopt0, but the table is empty:
        // there is no recovery metadata, so evaluation crashes loudly instead of
        // continuing with unrecovered state.
        let function = guard_eq_function(5, 3, DeoptId(0), false);
        let mut memory = GuestMemory::new_logical();

        let error = evaluate(&function, &mut memory).expect_err("deopt failure expected");

        assert!(matches!(
            error,
            EvalError::DeoptFailure { deopt: DeoptId(0) }
        ));
    }
}
