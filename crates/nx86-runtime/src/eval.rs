//! NxIR evaluator (Phase 14).
//!
//! A reference interpreter over verified NxIR that produces a [`CpuState`]. It
//! is the differential partner of the AArch64 [`TinyInterpreter`](crate::TinyInterpreter):
//! the two engines must agree on the final guest state and
//! memory for every synthetic program.

use nx86_core::guest::{CpuState, Nzcv};
use nx86_ir::{
    BinaryOp, DeoptId, FlagOp, FpBinaryOp, Function, Inst, Op, Reg, Terminator, Type, Value,
    VectorBinaryOp,
};
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
                let nzcv = pending_flags.map_or_else(|| cpu.nzcv(), materialize);
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
                let nzcv = pending_flags.map_or_else(|| cpu.nzcv(), materialize);
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
                if let Some(flags) = pending_flags {
                    cpu.set_nzcv(materialize(flags));
                }
                cpu.set_pc(block.terminator_address + 4);
                cpu.halt(reason.clone());
                return Ok(EvalOutcome::Exit(cpu));
            }
            Terminator::Return => {
                if let Some(flags) = pending_flags {
                    cpu.set_nzcv(materialize(flags));
                }
                return Ok(EvalOutcome::Exit(cpu));
            }
        }
    }

    Err(EvalError::StepLimit {
        max_steps: STEP_LIMIT,
    })
}

/// Materialize NZCV from a lazily-recorded flag source.
fn materialize(pending: FlagSource) -> Nzcv {
    match pending {
        (FlagOp::Add, lhs, rhs) => Nzcv::from_add(lhs, rhs),
        (FlagOp::Sub, lhs, rhs) => Nzcv::from_sub(lhs, rhs),
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
        Op::LoadExclusive { ty, address } => {
            let resolved = value_at(values, *address)?;
            let size = match ty {
                Type::I32 => 4u8,
                Type::I64 => 8u8,
                _ => return Err(EvalError::UnsupportedMemoryType { ty: *ty }),
            };
            cpu.set_monitor(resolved, size);
            Some(load_mem(memory, *ty, resolved)?)
        }
        Op::StoreExclusive { ty, address, value } => {
            let resolved_address = value_at(values, *address)?;
            let resolved_value = value_at(values, *value)?;
            let size = match ty {
                Type::I32 => 4u8,
                Type::I64 => 8u8,
                _ => return Err(EvalError::UnsupportedMemoryType { ty: *ty }),
            };
            let monitor = cpu.monitor().clone();
            if monitor.address == Some(resolved_address) && monitor.size == size {
                store_mem(memory, *ty, resolved_address, resolved_value)?;
                cpu.clear_monitor();
                Some(0u64)
            } else {
                cpu.clear_monitor();
                Some(1u64)
            }
        }
        Op::LoadAcquire { ty, address } => {
            let resolved = value_at(values, *address)?;
            Some(load_mem(memory, *ty, resolved)?)
        }
        Op::StoreRelease { ty, address, value } => {
            let resolved_address = value_at(values, *address)?;
            let resolved_value = value_at(values, *value)?;
            store_mem(memory, *ty, resolved_address, resolved_value)?;
            None
        }
        Op::Barrier { .. } => None,
        Op::FpMoveImmediate { rd, bits, .. } => {
            cpu.set_vector(*rd, u128::from(*bits));
            None
        }
        Op::FpScalarBinary { op, rd, rn, rm, .. } => {
            let lhs = cpu.scalar_f64(*rn);
            let rhs = cpu.scalar_f64(*rm);
            let value = match op {
                FpBinaryOp::Add => lhs + rhs,
                FpBinaryOp::Sub => lhs - rhs,
                FpBinaryOp::Mul => lhs * rhs,
                FpBinaryOp::Div => lhs / rhs,
            };
            cpu.set_scalar_f64(*rd, value);
            None
        }
        Op::FpCompare { rn, rm, .. } => {
            cpu.set_nzcv(float_compare(cpu.scalar_f64(*rn), cpu.scalar_f64(*rm)));
            *pending_flags = None;
            None
        }
        Op::VectorBinary { op, rd, rn, rm, .. } => {
            match op {
                VectorBinaryOp::AddI64 => {
                    for lane in 0..2 {
                        let value = cpu
                            .vector_lane64(*rn, lane)
                            .wrapping_add(cpu.vector_lane64(*rm, lane));
                        cpu.set_vector_lane64(*rd, lane, value);
                    }
                }
                VectorBinaryOp::AddF64 => {
                    for lane in 0..2 {
                        let lhs = f64::from_bits(cpu.vector_lane64(*rn, lane));
                        let rhs = f64::from_bits(cpu.vector_lane64(*rm, lane));
                        cpu.set_vector_lane64(*rd, lane, (lhs + rhs).to_bits());
                    }
                }
            }
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

fn float_compare(lhs: f64, rhs: f64) -> Nzcv {
    if lhs.is_nan() || rhs.is_nan() {
        return Nzcv {
            negative: false,
            zero: false,
            carry: true,
            overflow: true,
        };
    }
    if lhs == rhs {
        return Nzcv {
            negative: false,
            zero: true,
            carry: true,
            overflow: false,
        };
    }
    if lhs < rhs {
        return Nzcv {
            negative: true,
            zero: false,
            carry: false,
            overflow: false,
        };
    }
    Nzcv {
        negative: false,
        zero: false,
        carry: true,
        overflow: false,
    }
}

#[cfg(test)]
mod tests {
    use nx86_ir::{
        Block, BlockId, Cond, DeoptId, DeoptPoint, FlagOp, Function, Inst, Op, Reg, Terminator,
        Type, Value,
    };
    use nx86_vmm::{GuestAddress, GuestMemory};

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

    fn atomic_test_function(ops: Vec<(Option<Value>, Op)>) -> Function {
        use nx86_ir::{Block, Terminator};
        let mut instructions = Vec::new();
        for (i, (result, op)) in ops.into_iter().enumerate() {
            instructions.push(Inst {
                result,
                op,
                guest_address: (i as u64) * 4,
            });
        }
        instructions.push(Inst {
            result: None,
            op: Op::SetReg {
                reg: Reg::X(0),
                value: Value(0),
            },
            guest_address: (instructions.len() as u64) * 4,
        });
        Function {
            name: "atomic_test".to_owned(),
            entry_address: 0,
            value_count: 8,
            deopt_points: Vec::new(),
            blocks: vec![Block {
                instructions,
                terminator: Terminator::Halt {
                    reason: "svc #0x0".to_owned(),
                },
                terminator_address: 0,
            }],
        }
    }

    #[test]
    fn eval_exclusive_load_sets_monitor() {
        let function = atomic_test_function(vec![
            (
                Some(Value(0)),
                Op::Const {
                    ty: Type::I64,
                    value: 0x1000,
                },
            ),
            (
                Some(Value(1)),
                Op::LoadExclusive {
                    ty: Type::I64,
                    address: Value(0),
                },
            ),
        ]);
        let mut memory = GuestMemory::new_logical();
        memory
            .map_page(GuestAddress(0x1000), nx86_vmm::PagePermissions::READ_WRITE)
            .expect("page should map");
        memory
            .write(GuestAddress(0x1000), &0x42u64.to_le_bytes())
            .expect("write should succeed");

        let outcome = evaluate(&function, &mut memory).expect("eval should succeed");
        assert_eq!(outcome.state().monitor().address, Some(0x1000));
        assert_eq!(outcome.state().monitor().size, 8);
    }

    #[test]
    fn eval_exclusive_store_succeeds_when_monitored() {
        let function = atomic_test_function(vec![
            (
                Some(Value(0)),
                Op::Const {
                    ty: Type::I64,
                    value: 0x1000,
                },
            ),
            (
                Some(Value(1)),
                Op::LoadExclusive {
                    ty: Type::I64,
                    address: Value(0),
                },
            ),
            (
                Some(Value(2)),
                Op::Const {
                    ty: Type::I64,
                    value: 0xAB,
                },
            ),
            (
                Some(Value(3)),
                Op::StoreExclusive {
                    ty: Type::I64,
                    address: Value(0),
                    value: Value(2),
                },
            ),
        ]);
        let mut memory = GuestMemory::new_logical();
        memory
            .map_page(GuestAddress(0x1000), nx86_vmm::PagePermissions::READ_WRITE)
            .expect("page should map");

        let _outcome = evaluate(&function, &mut memory).expect("eval should succeed");
        let bytes = memory
            .read(GuestAddress(0x1000), 8)
            .expect("read should succeed");
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&bytes);
        assert_eq!(u64::from_le_bytes(buf), 0xAB);
    }

    #[test]
    fn eval_exclusive_store_fails_when_not_monitored() {
        let function = atomic_test_function(vec![
            (
                Some(Value(0)),
                Op::Const {
                    ty: Type::I64,
                    value: 0x1000,
                },
            ),
            (
                Some(Value(2)),
                Op::Const {
                    ty: Type::I64,
                    value: 0xAB,
                },
            ),
            (
                Some(Value(3)),
                Op::StoreExclusive {
                    ty: Type::I64,
                    address: Value(0),
                    value: Value(2),
                },
            ),
        ]);
        let mut memory = GuestMemory::new_logical();
        memory
            .map_page(GuestAddress(0x1000), nx86_vmm::PagePermissions::READ_WRITE)
            .expect("page should map");

        let _outcome = evaluate(&function, &mut memory).expect("eval should succeed");
        let bytes = memory
            .read(GuestAddress(0x1000), 8)
            .expect("read should succeed");
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&bytes);
        assert_eq!(u64::from_le_bytes(buf), 0);
    }

    #[test]
    fn eval_exclusive_store_fails_on_address_mismatch() {
        let function = atomic_test_function(vec![
            (
                Some(Value(0)),
                Op::Const {
                    ty: Type::I64,
                    value: 0x1000,
                },
            ),
            (
                Some(Value(1)),
                Op::LoadExclusive {
                    ty: Type::I64,
                    address: Value(0),
                },
            ),
            (
                Some(Value(4)),
                Op::Const {
                    ty: Type::I64,
                    value: 0x2000,
                },
            ),
            (
                Some(Value(2)),
                Op::Const {
                    ty: Type::I64,
                    value: 0xAB,
                },
            ),
            (
                Some(Value(3)),
                Op::StoreExclusive {
                    ty: Type::I64,
                    address: Value(4),
                    value: Value(2),
                },
            ),
        ]);
        let mut memory = GuestMemory::new_logical();
        memory
            .map_page(GuestAddress(0x1000), nx86_vmm::PagePermissions::READ_WRITE)
            .expect("page 1 should map");
        memory
            .map_page(GuestAddress(0x2000), nx86_vmm::PagePermissions::READ_WRITE)
            .expect("page 2 should map");

        let outcome = evaluate(&function, &mut memory).expect("eval should succeed");
        assert_eq!(outcome.state().monitor().address, None);
    }

    #[test]
    fn eval_acquire_load_reads_value() {
        use nx86_ir::{Block, Terminator};
        let function = Function {
            name: "acquire_test".to_owned(),
            entry_address: 0,
            value_count: 4,
            deopt_points: Vec::new(),
            blocks: vec![Block {
                instructions: vec![
                    Inst {
                        result: Some(Value(0)),
                        op: Op::Const {
                            ty: Type::I64,
                            value: 0x1000,
                        },
                        guest_address: 0,
                    },
                    Inst {
                        result: Some(Value(1)),
                        op: Op::LoadAcquire {
                            ty: Type::I64,
                            address: Value(0),
                        },
                        guest_address: 4,
                    },
                    Inst {
                        result: None,
                        op: Op::SetReg {
                            reg: Reg::X(0),
                            value: Value(1),
                        },
                        guest_address: 8,
                    },
                ],
                terminator: Terminator::Halt {
                    reason: "svc #0x0".to_owned(),
                },
                terminator_address: 12,
            }],
        };
        let mut memory = GuestMemory::new_logical();
        memory
            .map_page(GuestAddress(0x1000), nx86_vmm::PagePermissions::READ_WRITE)
            .expect("page should map");
        memory
            .write(GuestAddress(0x1000), &0xDEADu64.to_le_bytes())
            .expect("write should succeed");

        let outcome = evaluate(&function, &mut memory).expect("eval should succeed");
        assert_eq!(outcome.state().x(0), 0xDEAD);
    }

    #[test]
    fn eval_release_store_writes_value() {
        let function = atomic_test_function(vec![
            (
                Some(Value(0)),
                Op::Const {
                    ty: Type::I64,
                    value: 0x1000,
                },
            ),
            (
                Some(Value(2)),
                Op::Const {
                    ty: Type::I64,
                    value: 0xBEEF,
                },
            ),
            (
                None,
                Op::StoreRelease {
                    ty: Type::I64,
                    address: Value(0),
                    value: Value(2),
                },
            ),
        ]);
        let mut memory = GuestMemory::new_logical();
        memory
            .map_page(GuestAddress(0x1000), nx86_vmm::PagePermissions::READ_WRITE)
            .expect("page should map");

        let _outcome = evaluate(&function, &mut memory).expect("eval should succeed");
        let bytes = memory
            .read(GuestAddress(0x1000), 8)
            .expect("read should succeed");
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&bytes);
        assert_eq!(u64::from_le_bytes(buf), 0xBEEF);
    }
}
