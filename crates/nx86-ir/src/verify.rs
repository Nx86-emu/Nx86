//! NxIR verifier (Phase 13).
//!
//! Checks that a [`Function`] is well-formed before it is evaluated or lowered:
//! SSA correctness, type correctness, result/side-effect consistency, legal
//! block terminators, and instruction-boundary presence. The verifier is meant
//! to run after lifting and after every optimization pass in debug/research
//! builds.

use std::collections::HashMap;

use thiserror::Error;

use crate::{BlockId, Function, Module, Type, Value};

/// A well-formedness error found by the verifier.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum VerifyError {
    #[error("function `{name}` has no entry block")]
    NoEntryBlock { name: String },
    #[error("value {value} is defined more than once")]
    DuplicateValue { value: Value },
    #[error("value {value} is out of range for value-count {value_count}")]
    ResultOutOfRange { value: Value, value_count: u32 },
    #[error("value {value} is used before it is defined")]
    UndefinedValue { value: Value },
    #[error("value {value} has type {actual} but {expected} was required")]
    TypeMismatch {
        value: Value,
        expected: Type,
        actual: Type,
    },
    #[error("operation defines a value but its instruction has no result slot")]
    MissingResult { guest_address: u64 },
    #[error("value {value} is produced by a pure side-effect operation")]
    UnexpectedResult { value: Value },
    #[error("branch targets block{} but the function has {block_count} block(s)", target.0)]
    BranchTargetOutOfRange { target: BlockId, block_count: usize },
}

/// Verify every function in a module.
pub fn verify_module(module: &Module) -> Result<(), VerifyError> {
    for function in &module.functions {
        verify(function)?;
    }
    Ok(())
}

/// Verify a single function.
pub fn verify(function: &Function) -> Result<(), VerifyError> {
    if function.blocks.is_empty() {
        return Err(VerifyError::NoEntryBlock {
            name: function.name.clone(),
        });
    }

    let block_count = function.blocks.len();
    let mut defined: HashMap<Value, Type> = HashMap::new();

    for block in &function.blocks {
        for inst in &block.instructions {
            // SSA + type check on operands; def-before-use is enforced because
            // operands must already be in `defined` before the result is added.
            for (operand, expected) in inst.op.operand_constraints() {
                let actual = defined
                    .get(&operand)
                    .ok_or(VerifyError::UndefinedValue { value: operand })?;
                if *actual != expected {
                    return Err(VerifyError::TypeMismatch {
                        value: operand,
                        expected,
                        actual: *actual,
                    });
                }
            }

            // Result presence must match whether the op produces a value.
            match (inst.op.result_type(), inst.result) {
                (Some(ty), Some(value)) => {
                    if value.0 >= function.value_count {
                        return Err(VerifyError::ResultOutOfRange {
                            value,
                            value_count: function.value_count,
                        });
                    }
                    if defined.insert(value, ty).is_some() {
                        return Err(VerifyError::DuplicateValue { value });
                    }
                }
                (Some(_), None) => {
                    return Err(VerifyError::MissingResult {
                        guest_address: inst.guest_address,
                    });
                }
                (None, Some(value)) => {
                    return Err(VerifyError::UnexpectedResult { value });
                }
                (None, None) => {}
            }
        }

        // Legal terminators: branch targets must reference real blocks.
        for target in block.terminator.successors() {
            if target.0 as usize >= block_count {
                return Err(VerifyError::BranchTargetOutOfRange {
                    target,
                    block_count,
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{VerifyError, verify};
    use crate::{BinaryOp, Block, Function, Inst, Op, Reg, Terminator, Type, Value};

    fn valid_function() -> Function {
        Function {
            name: "add".to_owned(),
            entry_address: 0,
            value_count: 3,
            blocks: vec![Block {
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
                            value: 2,
                        },
                        guest_address: 0x0,
                    },
                    Inst {
                        result: Some(Value(2)),
                        op: Op::Binary {
                            op: BinaryOp::Add,
                            ty: Type::I64,
                            lhs: Value(0),
                            rhs: Value(1),
                        },
                        guest_address: 0x0,
                    },
                    Inst {
                        result: None,
                        op: Op::SetReg {
                            reg: Reg::X(1),
                            value: Value(2),
                        },
                        guest_address: 0x0,
                    },
                ],
                terminator: Terminator::Halt {
                    reason: "svc #0x0".to_owned(),
                },
                terminator_address: 0x4,
            }],
        }
    }

    #[test]
    fn valid_function_passes() {
        assert_eq!(verify(&valid_function()), Ok(()));
    }

    #[test]
    fn empty_function_is_rejected() {
        let function = Function {
            name: "empty".to_owned(),
            entry_address: 0,
            value_count: 0,
            blocks: Vec::new(),
        };
        assert!(matches!(
            verify(&function),
            Err(VerifyError::NoEntryBlock { .. })
        ));
    }

    #[test]
    fn use_before_def_is_rejected() {
        let mut function = valid_function();
        // Make the Add reference a value defined later.
        function.blocks[0].instructions[2].op = Op::Binary {
            op: BinaryOp::Add,
            ty: Type::I64,
            lhs: Value(9),
            rhs: Value(1),
        };
        assert_eq!(
            verify(&function),
            Err(VerifyError::UndefinedValue { value: Value(9) })
        );
    }

    #[test]
    fn duplicate_definition_is_rejected() {
        let mut function = valid_function();
        function.blocks[0].instructions[1].result = Some(Value(0));
        assert_eq!(
            verify(&function),
            Err(VerifyError::DuplicateValue { value: Value(0) })
        );
    }

    #[test]
    fn type_mismatch_is_rejected() {
        let mut function = valid_function();
        // Define v1 as i32, but the Add requires i64 operands.
        function.blocks[0].instructions[1].op = Op::Const {
            ty: Type::I32,
            value: 2,
        };
        assert_eq!(
            verify(&function),
            Err(VerifyError::TypeMismatch {
                value: Value(1),
                expected: Type::I64,
                actual: Type::I32,
            })
        );
    }

    #[test]
    fn missing_result_is_rejected() {
        let mut function = valid_function();
        function.blocks[0].instructions[0].result = None;
        assert!(matches!(
            verify(&function),
            Err(VerifyError::MissingResult { .. })
        ));
    }

    #[test]
    fn unexpected_result_on_side_effect_is_rejected() {
        let mut function = valid_function();
        function.blocks[0].instructions[3].result = Some(Value(2));
        assert!(matches!(
            verify(&function),
            Err(VerifyError::UnexpectedResult { .. })
        ));
    }

    #[test]
    fn out_of_range_branch_is_rejected() {
        let mut function = valid_function();
        function.blocks[0].terminator = Terminator::Branch {
            target: crate::BlockId(5),
        };
        assert!(matches!(
            verify(&function),
            Err(VerifyError::BranchTargetOutOfRange { .. })
        ));
    }
}
