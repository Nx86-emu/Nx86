use nx86_core::guest::CpuState;
use nx86_ir::{Function, Terminator};
use nx86_jit::{ExecError, ExecutableMemory};
use nx86_x64_v4::{NativeBlockState, lower_tiny_block};

pub const CRATE_NAME: &str = "nx86-backend";

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeStatus {
    MatchesInterpreter,
    DisagreesWithInterpreter,
    Unavailable,
    Error,
}

impl NativeStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::MatchesInterpreter => "matches interpreter",
            Self::DisagreesWithInterpreter => "disagrees with interpreter",
            Self::Unavailable => "unavailable",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeOutcome {
    pub status: NativeStatus,
    pub dump: String,
    pub final_state: Option<CpuState>,
    pub error: Option<String>,
}

impl NativeOutcome {
    #[must_use]
    pub const fn agrees(&self) -> bool {
        matches!(self.status, NativeStatus::MatchesInterpreter)
    }

    pub fn error(dump: String, error: impl ToString) -> Self {
        Self {
            status: NativeStatus::Error,
            dump,
            final_state: None,
            error: Some(error.to_string()),
        }
    }

    fn unavailable(dump: String, error: impl ToString) -> Self {
        Self {
            status: NativeStatus::Unavailable,
            dump,
            final_state: None,
            error: Some(error.to_string()),
        }
    }
}

pub fn run_tiny_native_block(
    function: &Function,
    initial_state: &CpuState,
    interpreter_state: &CpuState,
) -> NativeOutcome {
    let lowered = match lower_tiny_block(function) {
        Ok(lowered) => lowered,
        Err(error) => return NativeOutcome::error(String::new(), error),
    };
    let dump = lowered.dump().to_owned();

    let executable = match ExecutableMemory::new(lowered.bytes()) {
        Ok(executable) => executable,
        Err(error @ ExecError::UnsupportedHost { .. }) => {
            return NativeOutcome::unavailable(dump, error);
        }
        Err(error) => return NativeOutcome::error(dump, error),
    };

    let mut native_state = NativeBlockState::from_cpu_state(initial_state);
    if let Err(error) = call_generated_block(&executable, &mut native_state) {
        return NativeOutcome::error(dump, error);
    }

    let final_state = native_state.apply_to_cpu_state(initial_state.clone(), halt_reason(function));
    let status = if &final_state == interpreter_state {
        NativeStatus::MatchesInterpreter
    } else {
        NativeStatus::DisagreesWithInterpreter
    };

    NativeOutcome {
        status,
        dump,
        final_state: Some(final_state),
        error: None,
    }
}

fn halt_reason(function: &Function) -> Option<&str> {
    let block = function.blocks.first()?;
    match &block.terminator {
        Terminator::Halt { reason } => Some(reason.as_str()),
        Terminator::Branch { .. } | Terminator::CondBranch { .. } | Terminator::Return => None,
    }
}

#[allow(unsafe_code)]
fn call_generated_block(
    executable: &ExecutableMemory,
    state: &mut NativeBlockState,
) -> Result<(), ExecError> {
    // SAFETY: `lower_tiny_block` is the only producer of bytes passed here. It
    // emits an `extern "C" fn(*mut NativeBlockState)` block that only reads and
    // writes fields within the provided state pointer.
    unsafe { executable.call_with_state(state) }
}

#[cfg(test)]
mod tests {
    use nx86_core::guest::CpuState;
    use nx86_ir::{Block, Function, Inst, Op, Reg, Terminator, Type, Value};

    use super::{NativeStatus, run_tiny_native_block};

    #[test]
    fn native_attempt_reports_host_or_match() {
        let function = tiny_add_function();
        let mut initial = CpuState::new();
        initial.set_pc(0);
        let mut expected = CpuState::new();
        expected.set_x(0, 1);
        expected.set_x(1, 3);
        expected.set_pc(12);
        expected.halt("svc #0x0");

        let outcome = run_tiny_native_block(&function, &initial, &expected);

        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        assert_eq!(outcome.status, NativeStatus::MatchesInterpreter);

        #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
        assert_eq!(outcome.status, NativeStatus::Unavailable);

        assert!(!outcome.dump.is_empty());
    }

    #[test]
    fn native_attempt_reports_lowering_error() {
        let mut function = tiny_add_function();
        function.blocks.push(Block {
            instructions: Vec::new(),
            terminator: Terminator::Return,
            terminator_address: 12,
        });
        let initial = CpuState::new();
        let expected = CpuState::new();

        let outcome = run_tiny_native_block(&function, &initial, &expected);

        assert_eq!(outcome.status, NativeStatus::Error);
        assert!(
            outcome
                .error
                .as_deref()
                .is_some_and(|error| { error.contains("exactly one block") })
        );
    }

    fn tiny_add_function() -> Function {
        Function {
            name: "tiny_add".to_owned(),
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
                        guest_address: 0,
                    },
                    Inst {
                        result: None,
                        op: Op::SetReg {
                            reg: Reg::X(0),
                            value: Value(0),
                        },
                        guest_address: 0,
                    },
                    Inst {
                        result: Some(Value(1)),
                        op: Op::GetReg { reg: Reg::X(0) },
                        guest_address: 4,
                    },
                    Inst {
                        result: Some(Value(2)),
                        op: Op::Const {
                            ty: Type::I64,
                            value: 2,
                        },
                        guest_address: 4,
                    },
                    Inst {
                        result: Some(Value(3)),
                        op: Op::Binary {
                            op: nx86_ir::BinaryOp::Add,
                            ty: Type::I64,
                            lhs: Value(1),
                            rhs: Value(2),
                        },
                        guest_address: 4,
                    },
                    Inst {
                        result: None,
                        op: Op::SetReg {
                            reg: Reg::X(1),
                            value: Value(3),
                        },
                        guest_address: 4,
                    },
                ],
                terminator: Terminator::Halt {
                    reason: "svc #0x0".to_owned(),
                },
                terminator_address: 8,
            }],
        }
    }
}
