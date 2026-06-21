use nx86_core::guest::CpuState;
use nx86_ir::{Function, Terminator};
use nx86_jit::{ExecError, ExecutableMemory};
use nx86_x64_v4::{LoweredBlock, LoweringError, NativeBlockState, lower_tiny_block};

pub use nx86_object::{NativeObject, ObjectError};

pub const CRATE_NAME: &str = "nx86-backend";

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

/// Build a persistable [`NativeObject`] from a lowered single block and the
/// function it came from. The caller is responsible for having produced
/// `lowered` from `function` (e.g. via [`lower_tiny_block`]); the guest mapping
/// is taken from the function's entry address and its first block's span.
#[must_use]
pub fn native_object(function: &Function, lowered: &LoweredBlock) -> NativeObject {
    let guest_end = function
        .blocks
        .first()
        .map_or(function.entry_address, |block| {
            block.terminator_address.saturating_add(4)
        });
    NativeObject {
        entry_address: function.entry_address,
        guest_end,
        stack_size: u32::try_from(lowered.stack_size()).unwrap_or(0),
        code: lowered.bytes().to_vec(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeStatus {
    MatchesInterpreter,
    DisagreesWithInterpreter,
    /// The program is valid NxIR but its shape is outside the current tiny
    /// native path (e.g. multiple blocks, branches, or unlowered ops). This is
    /// expected and benign, not a failure.
    Unsupported,
    Unavailable,
    Error,
}

impl NativeStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::MatchesInterpreter => "matches interpreter",
            Self::DisagreesWithInterpreter => "disagrees with interpreter",
            Self::Unsupported => "unsupported",
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

    /// Build an outcome from a lowering failure, classifying "program shape not
    /// yet supported by the tiny path" (benign) apart from genuine failures.
    fn from_lowering_error(dump: String, error: LoweringError) -> Self {
        let status = match &error {
            LoweringError::UnsupportedBlockCount { .. }
            | LoweringError::UnsupportedOp { .. }
            | LoweringError::UnsupportedTerminator { .. } => NativeStatus::Unsupported,
            LoweringError::InvalidIr(_)
            | LoweringError::MissingResult { .. }
            | LoweringError::ValueOutOfRange { .. }
            | LoweringError::RegisterOutOfRange { .. }
            | LoweringError::StackTooLarge { .. }
            | LoweringError::AddressOverflow { .. }
            | LoweringError::Assembler(_) => NativeStatus::Error,
        };
        Self {
            status,
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
        Err(error) => return NativeOutcome::from_lowering_error(String::new(), error),
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

    use super::{
        NativeObject, NativeStatus, lower_tiny_block, native_object, run_tiny_native_block,
    };

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
    fn native_attempt_reports_unsupported_shape() {
        let mut function = tiny_add_function();
        function.blocks.push(Block {
            instructions: Vec::new(),
            terminator: Terminator::Return,
            terminator_address: 12,
        });
        let initial = CpuState::new();
        let expected = CpuState::new();

        let outcome = run_tiny_native_block(&function, &initial, &expected);

        assert_eq!(outcome.status, NativeStatus::Unsupported);
        assert!(
            outcome
                .error
                .as_deref()
                .is_some_and(|error| { error.contains("exactly one block") })
        );
    }

    #[test]
    fn native_attempt_reports_genuine_error() {
        // Corrupt the add to reference an undefined value: verification fails,
        // which is a real error rather than an unsupported program shape.
        let mut function = tiny_add_function();
        function.blocks[0].instructions[4].op = Op::Binary {
            op: nx86_ir::BinaryOp::Add,
            ty: Type::I64,
            lhs: Value(9),
            rhs: Value(2),
        };
        let initial = CpuState::new();
        let expected = CpuState::new();

        let outcome = run_tiny_native_block(&function, &initial, &expected);

        assert_eq!(outcome.status, NativeStatus::Error);
    }

    #[test]
    fn native_object_round_trips_through_bytes() {
        let function = tiny_add_function();
        let lowered = lower_tiny_block(&function).expect("tiny add should lower");

        let object = native_object(&function, &lowered);
        let restored = NativeObject::from_bytes(&object.to_bytes()).expect("valid object");

        assert_eq!(restored, object);
        assert_eq!(restored.entry_address, 0);
        assert_eq!(restored.guest_end, 12);
        assert_eq!(restored.code, lowered.bytes());
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    #[allow(unsafe_code)]
    fn persisted_object_executes_after_reload() {
        use nx86_x64_v4::NativeBlockState;

        let dir = tempfile::tempdir().expect("temp dir");
        let function = tiny_add_function();
        let lowered = lower_tiny_block(&function).expect("tiny add should lower");
        let object = native_object(&function, &lowered);

        // Persist to disk, then reload as if across a restart.
        let path = dir.path().join(object.file_name());
        object.write_to_path(&path).expect("write object");
        let loaded = NativeObject::read_from_path(&path).expect("read object");

        let executable =
            nx86_jit::ExecutableMemory::new(&loaded.code).expect("code should allocate");
        let cpu = CpuState::new();
        let mut state = NativeBlockState::from_cpu_state(&cpu);

        // SAFETY: the loaded bytes are the exact `extern "C"
        // fn(*mut NativeBlockState)` block produced by `lower_tiny_block`.
        unsafe { executable.call_with_state(&mut state) }.expect("native block should run");
        let final_state = state.to_cpu_state(Some("svc #0x0"));

        assert_eq!(final_state.x(1), 3);
        assert_eq!(final_state.pc(), 12);
        assert!(final_state.halted());
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
