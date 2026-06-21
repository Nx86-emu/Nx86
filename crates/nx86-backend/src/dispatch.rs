//! Multi-block dispatcher with emergency-JIT fallback (Phases 22-23).
//!
//! Each NxIR block is lowered to its own native block (see
//! [`nx86_x64_v4::lower_function`]) keyed by its guest entry PC. The
//! [`Dispatcher`] holds those blocks in a registry and runs the
//! lookup-call-route loop: read the current guest PC, call the matching block,
//! and continue. A block that exits via a branch leaves the halted flag clear
//! and writes the next guest PC; a `Halt` sets the flag and stops the loop. A
//! guest PC with no registered block is offered to an attached
//! [`EmergencyJit`]; without one, or when the source function does not contain
//! that PC, dispatch reports [`DispatchExit::MissingBlock`].

use std::collections::BTreeMap;

use nx86_core::guest::CpuState;
use nx86_ir::{Function, Terminator};
use nx86_jit::{EmergencyJit, ExecError, ExecutableMemory, JitError, JitEvent};
use nx86_object::NativeObject;
use nx86_x64_v4::{LoweringError, NativeBlockState, lower_function};
use thiserror::Error;

use crate::{NativeOutcome, NativeStatus, call_generated_block};

/// Default cap on dispatched block calls, guarding against runaway loops.
pub const DEFAULT_MAX_STEPS: usize = 10_000;

/// Why the dispatcher loop stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchExit {
    /// A block set the halted flag; execution stopped normally.
    Halted,
    /// No block is registered for this guest PC (the Phase 23 emergency-JIT seam).
    MissingBlock { pc: u64 },
    /// The dispatcher exceeded its step budget (likely a non-terminating loop).
    StepLimit { steps: usize },
}

/// The result of running the dispatcher: why it stopped and the final guest state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchOutcome {
    pub exit: DispatchExit,
    pub final_state: CpuState,
    /// Emergency-JIT blocks compiled during this dispatch run.
    pub jit_events: Vec<JitEvent>,
}

/// A registry of native blocks keyed by guest entry PC.
#[derive(Debug)]
pub struct Dispatcher {
    blocks: BTreeMap<u64, ExecutableMemory>,
    max_steps: usize,
    emergency_jit: Option<EmergencyJit>,
}

impl Dispatcher {
    /// Lower every block of `function` and register it by its guest entry PC.
    pub fn from_function(function: &Function) -> Result<Self, DispatchError> {
        let lowered = lower_function(function)?;
        let mut blocks = BTreeMap::new();
        for block in &lowered {
            blocks.insert(
                block.entry_pc,
                ExecutableMemory::new(block.lowered.bytes())?,
            );
        }
        Self::from_blocks(blocks)
    }

    /// Register native blocks loaded from cached objects (e.g. via
    /// `nx86_cache::CacheManager::load`), keyed by each object's entry address.
    ///
    /// # Safety
    ///
    /// Every object's code bytes must have been emitted by Nx86's trusted
    /// `nx86-x64-v4` lowerer for the `NativeBlockState` ABI and must not have
    /// been replaced or forged after persistence. The `.nxo` content hash
    /// detects accidental corruption; it does not establish provenance.
    #[allow(unsafe_code)]
    pub unsafe fn from_objects<'a>(
        objects: impl IntoIterator<Item = &'a NativeObject>,
    ) -> Result<Self, DispatchError> {
        let mut blocks = BTreeMap::new();
        for object in objects {
            blocks.insert(object.entry_address, ExecutableMemory::new(&object.code)?);
        }
        Self::from_blocks(blocks)
    }

    fn from_blocks(blocks: BTreeMap<u64, ExecutableMemory>) -> Result<Self, DispatchError> {
        if blocks.is_empty() {
            return Err(DispatchError::Empty);
        }
        Ok(Self {
            blocks,
            max_steps: DEFAULT_MAX_STEPS,
            emergency_jit: None,
        })
    }

    #[must_use]
    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps;
        self
    }

    /// Attach the compiler used when dispatch reaches a missing native block.
    #[must_use]
    pub fn with_emergency_jit(mut self, emergency_jit: EmergencyJit) -> Self {
        self.emergency_jit = Some(emergency_jit);
        self
    }

    #[must_use]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Run from `initial`, routing between registered blocks by guest PC until a
    /// block halts, a guest PC is missing, or the step budget is exhausted.
    /// An attached emergency JIT compiles, caches, and installs known missing
    /// blocks before dispatch continues. `halt_reason` is applied to the final
    /// state when a block halts so it can be compared against the interpreter.
    pub fn run(
        &mut self,
        initial: &CpuState,
        halt_reason: Option<&str>,
    ) -> Result<DispatchOutcome, DispatchError> {
        let mut native = NativeBlockState::from_cpu_state(initial);
        let mut jit_events = Vec::new();
        let mut steps = 0;

        while steps < self.max_steps {
            let pc = native.pc;
            if let Some(executable) = self.blocks.get(&pc) {
                call_generated_block(executable, &mut native)?;
                steps += 1;
                if native.halted != 0 {
                    return Ok(DispatchOutcome {
                        exit: DispatchExit::Halted,
                        final_state: native.apply_to_cpu_state(initial.clone(), halt_reason),
                        jit_events,
                    });
                }
                continue;
            }

            let Some(emergency_jit) = &self.emergency_jit else {
                return Ok(DispatchOutcome {
                    exit: DispatchExit::MissingBlock { pc },
                    final_state: native.apply_to_cpu_state(initial.clone(), None),
                    jit_events,
                });
            };
            let Some(compilation) = emergency_jit.compile(pc)? else {
                return Ok(DispatchOutcome {
                    exit: DispatchExit::MissingBlock { pc },
                    final_state: native.apply_to_cpu_state(initial.clone(), None),
                    jit_events,
                });
            };
            let executable = ExecutableMemory::new(&compilation.object.code)?;
            self.blocks.insert(pc, executable);
            jit_events.push(compilation.event);
        }

        Ok(DispatchOutcome {
            exit: DispatchExit::StepLimit {
                steps: self.max_steps,
            },
            final_state: native.apply_to_cpu_state(initial.clone(), None),
            jit_events,
        })
    }
}

/// A failure building or running the dispatcher.
#[derive(Debug, Error)]
pub enum DispatchError {
    #[error("lowering failed: {0}")]
    Lowering(#[from] LoweringError),
    #[error("executable memory failed: {0}")]
    Exec(#[from] ExecError),
    #[error("emergency JIT failed: {0}")]
    EmergencyJit(#[from] JitError),
    #[error("dispatcher has no native blocks")]
    Empty,
}

/// Reason string of the first `Halt` terminator anywhere in the function. The
/// lifter's entry block may be a branch, so this scans all blocks rather than
/// only the first.
fn function_halt_reason(function: &Function) -> Option<&str> {
    function
        .blocks
        .iter()
        .find_map(|block| match &block.terminator {
            Terminator::Halt { reason } => Some(reason.as_str()),
            Terminator::Branch { .. } | Terminator::CondBranch { .. } | Terminator::Return => None,
        })
}

/// Lower `function` into a dispatcher, run it from `initial`, and classify the
/// result against the interpreter — the multi-block analogue of
/// [`crate::run_tiny_native_block`].
pub fn run_dispatched_function(
    function: &Function,
    initial: &CpuState,
    interpreter_state: &CpuState,
) -> NativeOutcome {
    // Lower first so a dump is available even when execution is unavailable
    // (e.g. on the Apple Silicon dev host), mirroring the single-block path.
    let lowered = match lower_function(function) {
        Ok(lowered) => lowered,
        Err(error) => return NativeOutcome::from_lowering_error(String::new(), error),
    };
    let dump = lowered
        .iter()
        .map(|block| format!("; block @ {:#x}\n{}", block.entry_pc, block.lowered.dump()))
        .collect::<Vec<_>>()
        .join("\n");

    let mut blocks = BTreeMap::new();
    for block in &lowered {
        match ExecutableMemory::new(block.lowered.bytes()) {
            Ok(executable) => {
                blocks.insert(block.entry_pc, executable);
            }
            Err(error @ ExecError::UnsupportedHost { .. }) => {
                return NativeOutcome::unavailable(dump, error);
            }
            Err(error) => return NativeOutcome::error(dump, error),
        }
    }
    let mut dispatcher = Dispatcher {
        blocks,
        max_steps: DEFAULT_MAX_STEPS,
        emergency_jit: None,
    };

    let outcome = match dispatcher.run(initial, function_halt_reason(function)) {
        Ok(outcome) => outcome,
        Err(error) => return NativeOutcome::error(dump, error),
    };

    match outcome.exit {
        DispatchExit::Halted => {
            let status = if &outcome.final_state == interpreter_state {
                NativeStatus::MatchesInterpreter
            } else {
                NativeStatus::DisagreesWithInterpreter
            };
            NativeOutcome {
                status,
                dump,
                final_state: Some(outcome.final_state),
                error: None,
            }
        }
        DispatchExit::MissingBlock { pc } => {
            NativeOutcome::error(dump, format!("no native block for guest pc {pc:#x}"))
        }
        DispatchExit::StepLimit { steps } => {
            NativeOutcome::error(dump, format!("dispatcher exceeded {steps} steps"))
        }
    }
}
