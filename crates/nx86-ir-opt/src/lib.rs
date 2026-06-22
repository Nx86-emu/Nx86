//! NxIR optimization passes (Phase 15).
//!
//! The first pass is dead-flag elimination. Lazy flags ([`Op::SetFlags`]) are
//! only ever consumed by a block terminator, so any `SetFlags` followed by
//! another `SetFlags` in the same block is overwritten before it can be read and
//! is therefore dead. The last `SetFlags` in a block is kept conservatively
//! because it may still be observable (a conditional branch, or the final NZCV).
//!
//! Per `SPEC.md` §21.5 the verifier should run after each pass; callers should
//! re-verify the function after applying this pass.

use nx86_ir::{Function, Op};

/// Remove `SetFlags` operations whose flags are overwritten before any consumer.
/// Returns the number of operations removed.
pub fn eliminate_dead_flags(function: &mut Function) -> usize {
    let mut removed = 0;
    for block in &mut function.blocks {
        let Some(last_setflags) = block
            .instructions
            .iter()
            .rposition(|inst| matches!(inst.op, Op::SetFlags { .. }))
        else {
            continue;
        };

        let before = block.instructions.len();
        let mut index = 0;
        block.instructions.retain(|inst| {
            let keep = !matches!(inst.op, Op::SetFlags { .. }) || index == last_setflags;
            index += 1;
            keep
        });
        removed += before - block.instructions.len();
    }
    removed
}

#[cfg(test)]
mod tests {
    use nx86_ir::verify::verify;
    use nx86_ir::{Block, Cond, FlagOp, Function, Inst, Op, Terminator, Type, Value};

    use super::eliminate_dead_flags;

    fn setflags(lhs: Value, rhs: Value, address: u64) -> Inst {
        Inst {
            result: None,
            op: Op::SetFlags {
                op: FlagOp::Sub,
                lhs,
                rhs,
            },
            guest_address: address,
        }
    }

    fn const_i64(result: Value, value: u64, address: u64) -> Inst {
        Inst {
            result: Some(result),
            op: Op::Const {
                ty: Type::I64,
                value,
            },
            guest_address: address,
        }
    }

    #[test]
    fn removes_overwritten_flags_and_keeps_last() {
        // Two CMPs feeding one conditional branch; the first SetFlags is dead.
        let mut function = Function {
            name: "two-cmp".to_owned(),
            entry_address: 0,
            value_count: 4,
            deopt_points: Vec::new(),
            blocks: vec![
                Block {
                    instructions: vec![
                        const_i64(Value(0), 1, 0x0),
                        const_i64(Value(1), 1, 0x0),
                        setflags(Value(0), Value(1), 0x0),
                        const_i64(Value(2), 2, 0x4),
                        const_i64(Value(3), 2, 0x4),
                        setflags(Value(2), Value(3), 0x4),
                    ],
                    terminator: Terminator::CondBranch {
                        cond: Cond::Eq,
                        if_true: nx86_ir::BlockId(1),
                        if_false: nx86_ir::BlockId(1),
                    },
                    terminator_address: 0x8,
                },
                Block {
                    instructions: Vec::new(),
                    terminator: Terminator::Return,
                    terminator_address: 0xc,
                },
            ],
        };

        let removed = eliminate_dead_flags(&mut function);

        assert_eq!(removed, 1);
        let remaining: Vec<_> = function.blocks[0]
            .instructions
            .iter()
            .filter(|inst| matches!(inst.op, Op::SetFlags { .. }))
            .collect();
        assert_eq!(remaining.len(), 1);
        // The surviving SetFlags is the later one (operands v2, v3).
        assert!(matches!(
            remaining[0].op,
            Op::SetFlags {
                lhs: Value(2),
                rhs: Value(3),
                ..
            }
        ));
        // The pass preserves verifiability.
        assert_eq!(verify(&function), Ok(()));
    }

    #[test]
    fn keeps_single_flag_source() {
        let mut function = Function {
            name: "one-cmp".to_owned(),
            entry_address: 0,
            value_count: 2,
            deopt_points: Vec::new(),
            blocks: vec![Block {
                instructions: vec![
                    const_i64(Value(0), 1, 0x0),
                    const_i64(Value(1), 1, 0x0),
                    setflags(Value(0), Value(1), 0x0),
                ],
                terminator: Terminator::CondBranch {
                    cond: Cond::Eq,
                    if_true: nx86_ir::BlockId(0),
                    if_false: nx86_ir::BlockId(0),
                },
                terminator_address: 0x4,
            }],
        };

        let removed = eliminate_dead_flags(&mut function);

        assert_eq!(removed, 0);
        assert_eq!(
            function.blocks[0]
                .instructions
                .iter()
                .filter(|inst| matches!(inst.op, Op::SetFlags { .. }))
                .count(),
            1
        );
    }
}
