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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use nx86_core::{config::CompilerConfig, guest::CpuState};
use nx86_ir::{Function, Terminator};
#[cfg(all(
    feature = "native-patch-chaining",
    target_os = "linux",
    target_arch = "x86_64"
))]
use nx86_jit::PatchStage;
use nx86_jit::{EmergencyJit, ExecError, ExecutableMemory, JitError, JitEvent};
use nx86_object::NativeObject;
use nx86_profile::{ProfileError, ProfileEvent, ProfileSink};
use nx86_vmm::GuestMemory;
#[cfg(any(
    test,
    all(
        feature = "native-patch-chaining",
        target_os = "linux",
        target_arch = "x86_64"
    )
))]
use nx86_x64_v4::ChainExitKind;
use nx86_x64_v4::{ChainExit, LoweringError, NativeBlockState, lower_function};
use thiserror::Error;

use crate::{
    ChainStats, FaultReport, NativeMemoryContext, NativeMemoryError, NativeOutcome, NativeStatus,
    SlowmemCounters, call_generated_block, chain::ChainCache,
};

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
    /// Block-chaining activity during this run.
    pub chain_stats: ChainStats,
    /// Aggregate slowmem counters from this dispatch run.
    pub slowmem_counters: SlowmemCounters,
}

#[derive(Debug)]
struct NativeBlock {
    executable: ExecutableMemory,
    chain_exits: Vec<ChainExit>,
}

impl NativeBlock {
    fn patched_successor(&self) -> Option<u64> {
        self.chain_exits
            .iter()
            .find(|exit| exit.patched)
            .map(|exit| exit.successor_pc)
    }
}

/// A registry of native blocks keyed by guest entry PC.
#[derive(Debug)]
pub struct Dispatcher {
    blocks: HashMap<u64, NativeBlock>,
    halt_reasons: BTreeMap<u64, String>,
    max_steps: usize,
    emergency_jit: Option<EmergencyJit>,
    profile_sink: Option<Box<dyn ProfileSink>>,
    chains: ChainCache,
    native_patch_chaining: bool,
    native_incoming: HashMap<u64, HashSet<u64>>,
}

impl Dispatcher {
    /// Lower every block of `function` and register it by its guest entry PC.
    pub fn from_function(function: &Function) -> Result<Self, DispatchError> {
        let lowered = lower_function(function)?;
        let mut blocks = HashMap::new();
        for block in &lowered {
            blocks.insert(
                block.entry_pc,
                NativeBlock {
                    executable: ExecutableMemory::new(block.lowered.bytes())?,
                    chain_exits: block.lowered.chain_exits().to_vec(),
                },
            );
        }
        let mut dispatcher = Self::from_blocks(blocks)?;
        dispatcher.halt_reasons = function_halt_reasons(function);
        Ok(dispatcher)
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
        let objects: Vec<&NativeObject> = objects.into_iter().collect();
        let mut entries = BTreeSet::new();
        for object in &objects {
            if !entries.insert(object.entry_address) {
                return Err(DispatchError::DuplicateBlock {
                    pc: object.entry_address,
                });
            }
        }
        let mut blocks = HashMap::new();
        for object in objects {
            blocks.insert(
                object.entry_address,
                NativeBlock {
                    executable: ExecutableMemory::new(&object.code)?,
                    // Phase 29 does not yet persist patch metadata in `.nxo`.
                    chain_exits: Vec::new(),
                },
            );
        }
        Self::from_blocks(blocks)
    }

    fn from_blocks(blocks: HashMap<u64, NativeBlock>) -> Result<Self, DispatchError> {
        if blocks.is_empty() {
            return Err(DispatchError::Empty);
        }
        Ok(Self {
            blocks,
            halt_reasons: BTreeMap::new(),
            max_steps: DEFAULT_MAX_STEPS,
            emergency_jit: None,
            profile_sink: None,
            chains: ChainCache::default(),
            native_patch_chaining: false,
            native_incoming: HashMap::new(),
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

    /// Attach a runtime profile destination. Profile failures are fatal to the
    /// dispatch run and are returned as [`DispatchError::Profile`].
    #[must_use]
    pub fn with_profile_sink(mut self, profile_sink: impl ProfileSink + 'static) -> Self {
        self.profile_sink = Some(Box::new(profile_sink));
        self
    }

    /// Enable or disable dispatcher-level chaining. Chaining defaults to enabled;
    /// pass `false` before the first run for debugger-friendly dispatch. After
    /// execution starts, use [`Self::set_chaining_enabled`] so native exits are
    /// restored before disabling.
    #[must_use]
    pub fn with_chaining(mut self, enabled: bool) -> Self {
        self.chains.set_enabled(enabled);
        if !enabled {
            self.native_patch_chaining = false;
        }
        self
    }

    /// Opt into native exit patching. The Cargo feature and Linux x86_64 host
    /// gates must also be active; otherwise software chaining remains in use.
    /// After execution starts, disable through
    /// [`Self::set_native_patch_chaining`] so installed patches are restored.
    #[must_use]
    pub const fn with_native_patch_chaining(mut self, enabled: bool) -> Self {
        self.native_patch_chaining = enabled;
        self
    }

    /// Apply the persisted experimental native-patching preference.
    #[must_use]
    pub const fn with_compiler_config(self, config: &CompilerConfig) -> Self {
        self.with_native_patch_chaining(config.native_patch_chaining)
    }

    /// Change dispatcher-level chaining at runtime. Disabling first restores
    /// every native patch; if restoration fails, chaining remains enabled and
    /// the error is returned so the caller cannot mistake a partial disable for
    /// a debugger-safe state.
    pub fn set_chaining_enabled(&mut self, enabled: bool) -> Result<(), DispatchError> {
        if !enabled {
            self.restore_all_native_exits()?;
            self.native_patch_chaining = false;
        }
        self.chains.set_enabled(enabled);
        Ok(())
    }

    /// Change only the experimental native patch layer at runtime. Disabling
    /// restores all native exits while leaving software chaining enabled.
    pub fn set_native_patch_chaining(&mut self, enabled: bool) -> Result<(), DispatchError> {
        if !enabled {
            self.restore_all_native_exits()?;
        }
        self.native_patch_chaining = enabled;
        Ok(())
    }

    #[must_use]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Cumulative chaining activity for diagnostics and tests.
    #[must_use]
    pub const fn chain_stats(&self) -> ChainStats {
        self.chains.stats()
    }

    /// Break all software and native chains entering or leaving `pc`. Call this
    /// before replacing or removing a compiled block.
    pub fn invalidate(&mut self, pc: u64) -> Result<(), DispatchError> {
        // Clone rather than remove first: if any unpatch fails, the remaining
        // reverse edges stay discoverable and invalidation can be retried.
        let mut sources = self.native_incoming.get(&pc).cloned().unwrap_or_default();
        sources.insert(pc);
        for source_pc in sources {
            self.restore_native_exits(source_pc, if source_pc == pc { None } else { Some(pc) })?;
        }
        self.native_incoming.remove(&pc);
        for incoming in self.native_incoming.values_mut() {
            incoming.remove(&pc);
        }
        self.native_incoming
            .retain(|_, sources| !sources.is_empty());
        self.chains.invalidate(pc);
        Ok(())
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
        self.run_with_memory(initial, halt_reason, None)
    }

    /// Run with guest memory attached for direct fastmem and checked slowmem
    /// fallback.
    pub fn run_in(
        &mut self,
        initial: &CpuState,
        memory: &mut GuestMemory,
        halt_reason: Option<&str>,
    ) -> Result<DispatchOutcome, DispatchError> {
        self.run_with_memory(initial, halt_reason, Some(memory))
    }

    fn run_with_memory(
        &mut self,
        initial: &CpuState,
        halt_reason: Option<&str>,
        memory: Option<&mut GuestMemory>,
    ) -> Result<DispatchOutcome, DispatchError> {
        let mut native = NativeBlockState::from_cpu_state(initial);
        let (mut memory_context, fastmem_base, fastmem_permissions) = match memory {
            Some(memory) => NativeMemoryContext::attached(memory),
            None => (NativeMemoryContext::missing(), 0, 0),
        };
        memory_context.configure(&mut native, fastmem_base, fastmem_permissions);
        let mut jit_events = Vec::new();
        let mut steps = 0;
        let chain_stats_before = self.chains.stats();

        while steps < self.max_steps {
            let pc = native.pc;
            if let Some(block) = self.blocks.get(&pc) {
                let patched_successor = block.patched_successor();
                call_generated_block(&block.executable, &mut native)?;
                if native.memory_fault != 0 {
                    let error = memory_context
                        .take_failure()
                        .unwrap_or(NativeMemoryError::MissingContext);
                    let report = memory_context.build_fault_report(&error, pc);
                    return Err(DispatchError::Memory { error, report });
                }
                for event in memory_context.take_pending_events() {
                    self.record_profile(event)?;
                }
                steps += 1;
                if patched_successor.is_some() {
                    self.chains.record_native_entry();
                }
                if native.halted != 0 {
                    let halt_reason = self
                        .halt_reasons
                        .get(&pc)
                        .map(String::as_str)
                        .or(halt_reason);
                    return Ok(DispatchOutcome {
                        exit: DispatchExit::Halted,
                        final_state: native.apply_to_cpu_state(initial.clone(), halt_reason),
                        jit_events,
                        chain_stats: self.chains.stats().difference(chain_stats_before),
                        slowmem_counters: memory_context.counters().clone(),
                    });
                }
                if let Some(target_pc) = patched_successor {
                    self.record_profile(ProfileEvent::BranchTarget {
                        source_pc: pc,
                        target_pc,
                    })?;
                    continue;
                }
                self.record_profile(ProfileEvent::BranchTarget {
                    source_pc: pc,
                    target_pc: native.pc,
                })?;
                let target_pc = native.pc;
                if !self.blocks.contains_key(&target_pc) {
                    // Missing successors route through emergency JIT or the
                    // normal MissingBlock exit; never cache a dangling chain.
                    continue;
                }
                if self.chains.hit(pc, target_pc) {
                    // A previous safe patch failure, or runtime re-enabling of
                    // native chaining, may make an established software edge
                    // eligible for another native installation attempt.
                    self.try_install_native_chain(pc, target_pc)?;
                    continue;
                }
                if self.chains.observe(pc, target_pc) {
                    self.try_install_native_chain(pc, target_pc)?;
                }
                continue;
            }

            let Some(emergency_jit) = &self.emergency_jit else {
                return Ok(DispatchOutcome {
                    exit: DispatchExit::MissingBlock { pc },
                    final_state: native.apply_to_cpu_state(initial.clone(), None),
                    jit_events,
                    chain_stats: self.chains.stats().difference(chain_stats_before),
                    slowmem_counters: memory_context.counters().clone(),
                });
            };
            let Some(compilation) = emergency_jit.compile(pc)? else {
                return Ok(DispatchOutcome {
                    exit: DispatchExit::MissingBlock { pc },
                    final_state: native.apply_to_cpu_state(initial.clone(), None),
                    jit_events,
                    chain_stats: self.chains.stats().difference(chain_stats_before),
                    slowmem_counters: memory_context.counters().clone(),
                });
            };
            self.record_profile(ProfileEvent::JitBlock {
                guest_pc: compilation.event.guest_pc,
                code_size_bytes: compilation.event.code_size_bytes as u64,
                cache_file_name: compilation.event.cache_file_name.clone(),
            })?;
            let executable = ExecutableMemory::new(&compilation.object.code)?;
            if let Some(reason) = compilation.halt_reason {
                self.halt_reasons.insert(pc, reason);
            }
            self.blocks.insert(
                pc,
                NativeBlock {
                    executable,
                    // `.nxo` v0 does not carry chain-exit metadata.
                    chain_exits: Vec::new(),
                },
            );
            jit_events.push(compilation.event);
        }

        Ok(DispatchOutcome {
            exit: DispatchExit::StepLimit {
                steps: self.max_steps,
            },
            final_state: native.apply_to_cpu_state(initial.clone(), None),
            jit_events,
            chain_stats: self.chains.stats().difference(chain_stats_before),
            slowmem_counters: memory_context.counters().clone(),
        })
    }

    #[cfg(all(
        feature = "native-patch-chaining",
        target_os = "linux",
        target_arch = "x86_64"
    ))]
    #[allow(unsafe_code)]
    fn try_install_native_chain(
        &mut self,
        source_pc: u64,
        target_pc: u64,
    ) -> Result<bool, DispatchError> {
        if !self.native_patch_chaining || !self.chains.enabled() || target_pc <= source_pc {
            return Ok(false);
        }
        let Some(target) = self.blocks.get(&target_pc) else {
            return Ok(false);
        };
        // Chaining into a Branch block ensures native execution returns to the
        // dispatcher before a Halt/Return reason must be classified.
        if !target
            .chain_exits
            .iter()
            .any(|exit| chain_exit_metadata_is_valid(target_pc, target.executable.len(), exit))
        {
            return Ok(false);
        }
        let target_addr = target.executable.entry_addr()?;
        let Some(source) = self.blocks.get(&source_pc) else {
            return Ok(false);
        };
        let Some(exit_index) = source.chain_exits.iter().position(|exit| {
            !exit.patched
                && exit.successor_pc == target_pc
                && chain_exit_metadata_is_valid(source_pc, source.executable.len(), exit)
        }) else {
            return Ok(false);
        };
        let exit = &source.chain_exits[exit_index];
        if exit.patch_size != nx86_x64_asm::CHAIN_EXIT_SIZE {
            return Ok(false);
        }
        let source_addr = source.executable.entry_addr()?;
        let Some(slot_end) = source_addr
            .checked_add(exit.patch_offset)
            .and_then(|slot| slot.checked_add(nx86_x64_asm::CHAIN_EXIT_SIZE))
        else {
            return Ok(false);
        };
        let delta = (target_addr as i128) - (slot_end as i128);
        let Ok(displacement) = i32::try_from(delta) else {
            return Ok(false);
        };
        let patch = nx86_x64_asm::encode_jmp_rel32(displacement);

        let source = self
            .blocks
            .get_mut(&source_pc)
            .ok_or(DispatchError::MissingRegisteredBlock { pc: source_pc })?;
        // SAFETY: Nx86 emitted this fixed-size chain slot for a `jmp rel32`, the
        // displacement was range-checked, and dispatch is single-threaded with
        // no generated block executing during this call.
        if let Err(error) = unsafe {
            source.executable.patch(
                source.chain_exits[exit_index].patch_offset,
                patch.as_slice(),
            )
        } {
            if matches!(
                error,
                ExecError::Patch {
                    stage: PatchStage::RestoreExecutable,
                    ..
                }
            ) {
                return Err(DispatchError::Exec(error));
            }
            return Ok(false);
        }
        source.chain_exits[exit_index].patched = true;
        self.native_incoming
            .entry(target_pc)
            .or_default()
            .insert(source_pc);
        self.chains.record_native_install();
        Ok(true)
    }

    #[cfg(not(all(
        feature = "native-patch-chaining",
        target_os = "linux",
        target_arch = "x86_64"
    )))]
    fn try_install_native_chain(
        &mut self,
        _source_pc: u64,
        _target_pc: u64,
    ) -> Result<bool, DispatchError> {
        Ok(false)
    }

    #[allow(unsafe_code)]
    fn restore_native_exits(
        &mut self,
        source_pc: u64,
        target_filter: Option<u64>,
    ) -> Result<(), DispatchError> {
        let Some(block) = self.blocks.get_mut(&source_pc) else {
            return Ok(());
        };
        for exit in &mut block.chain_exits {
            if !exit.patched || target_filter.is_some_and(|target| exit.successor_pc != target) {
                continue;
            }
            #[cfg(all(
                feature = "native-patch-chaining",
                target_os = "linux",
                target_arch = "x86_64"
            ))]
            {
                // SAFETY: `original_bytes` were captured from this exact slot at
                // lowering time, and invalidation runs between generated calls.
                unsafe {
                    block
                        .executable
                        .patch(exit.patch_offset, &exit.original_bytes)
                }?;
            }
            exit.patched = false;
            if let Some(sources) = self.native_incoming.get_mut(&exit.successor_pc) {
                sources.remove(&source_pc);
            }
        }
        Ok(())
    }

    fn restore_all_native_exits(&mut self) -> Result<(), DispatchError> {
        let sources = self
            .blocks
            .iter()
            .filter_map(|(pc, block)| {
                block
                    .chain_exits
                    .iter()
                    .any(|exit| exit.patched)
                    .then_some(*pc)
            })
            .collect::<Vec<_>>();
        for source_pc in sources {
            self.restore_native_exits(source_pc, None)?;
        }
        self.native_incoming.clear();
        Ok(())
    }

    fn record_profile(&mut self, event: ProfileEvent) -> Result<(), DispatchError> {
        if let Some(profile_sink) = &mut self.profile_sink {
            profile_sink.record(event)?;
        }
        Ok(())
    }
}

#[cfg(any(
    test,
    all(
        feature = "native-patch-chaining",
        target_os = "linux",
        target_arch = "x86_64"
    )
))]
fn chain_exit_metadata_is_valid(block_pc: u64, code_len: usize, exit: &ChainExit) -> bool {
    const ORIGINAL_CHAIN_EXIT: [u8; nx86_x64_asm::CHAIN_EXIT_SIZE] = [0xC3, 0x90, 0x90, 0x90, 0x90];
    exit.block_entry_pc == block_pc
        && exit.exit_kind == ChainExitKind::UnconditionalBranch
        && exit.patch_size == nx86_x64_asm::CHAIN_EXIT_SIZE
        && exit.original_bytes.as_slice() == ORIGINAL_CHAIN_EXIT
        && exit
            .patch_offset
            .checked_add(exit.patch_size)
            .is_some_and(|end| end <= code_len)
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
    #[error("runtime profile recording failed: {0}")]
    Profile(#[from] ProfileError),
    #[error("native memory execution failed: {error}")]
    Memory {
        error: NativeMemoryError,
        report: FaultReport,
    },
    #[error("multiple native blocks use guest entry pc {pc:#x}")]
    DuplicateBlock { pc: u64 },
    #[error("dispatcher has no native blocks")]
    Empty,
    #[error("dispatcher lost registered native block {pc:#x}")]
    MissingRegisteredBlock { pc: u64 },
}

fn function_halt_reasons(function: &Function) -> BTreeMap<u64, String> {
    function
        .blocks
        .iter()
        .filter_map(|block| match &block.terminator {
            Terminator::Halt { reason } => Some((block.entry_address(), reason.clone())),
            Terminator::Branch { .. }
            | Terminator::CondBranch { .. }
            | Terminator::Guard { .. }
            | Terminator::Return => None,
        })
        .collect()
}

/// Lower `function` into a dispatcher, run it from `initial`, and classify the
/// result against the interpreter — the multi-block analogue of
/// [`crate::run_tiny_native_block`].
pub fn run_dispatched_function(
    function: &Function,
    initial: &CpuState,
    interpreter_state: &CpuState,
) -> NativeOutcome {
    run_dispatched_function_with_memory(function, initial, interpreter_state, None)
}

pub fn run_dispatched_function_in(
    function: &Function,
    initial: &CpuState,
    interpreter_state: &CpuState,
    memory: &mut GuestMemory,
) -> NativeOutcome {
    run_dispatched_function_with_memory(function, initial, interpreter_state, Some(memory))
}

fn run_dispatched_function_with_memory(
    function: &Function,
    initial: &CpuState,
    interpreter_state: &CpuState,
    memory: Option<&mut GuestMemory>,
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

    let mut blocks = HashMap::new();
    for block in &lowered {
        match ExecutableMemory::new(block.lowered.bytes()) {
            Ok(executable) => {
                blocks.insert(
                    block.entry_pc,
                    NativeBlock {
                        executable,
                        chain_exits: block.lowered.chain_exits().to_vec(),
                    },
                );
            }
            Err(error @ ExecError::UnsupportedHost { .. }) => {
                return NativeOutcome::unavailable(dump, error);
            }
            Err(error) => return NativeOutcome::error(dump, error),
        }
    }
    let mut dispatcher = Dispatcher {
        blocks,
        halt_reasons: function_halt_reasons(function),
        max_steps: DEFAULT_MAX_STEPS,
        emergency_jit: None,
        profile_sink: None,
        chains: ChainCache::default(),
        native_patch_chaining: false,
        native_incoming: HashMap::new(),
    };

    let execution = match memory {
        Some(memory) => dispatcher.run_in(initial, memory, None),
        None => dispatcher.run(initial, None),
    };
    let outcome = match execution {
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
                slowmem_counters: Some(outcome.slowmem_counters),
            }
        }
        DispatchExit::MissingBlock { pc } => NativeOutcome {
            status: NativeStatus::Error,
            dump,
            final_state: None,
            error: Some(format!("no native block for guest pc {pc:#x}")),
            slowmem_counters: Some(outcome.slowmem_counters),
        },
        DispatchExit::StepLimit { steps } => NativeOutcome {
            status: NativeStatus::Error,
            dump,
            final_state: None,
            error: Some(format!("dispatcher exceeded {steps} steps")),
            slowmem_counters: Some(outcome.slowmem_counters),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, HashMap},
        io,
        path::PathBuf,
    };

    use nx86_profile::{ProfileError, ProfileEvent, ProfileSink, RecordOutcome};
    use nx86_x64_v4::{ChainExit, ChainExitKind};

    use super::{DEFAULT_MAX_STEPS, DispatchError, Dispatcher, chain_exit_metadata_is_valid};

    #[derive(Debug)]
    struct FailingProfileSink;

    impl ProfileSink for FailingProfileSink {
        fn record(&mut self, _event: ProfileEvent) -> Result<RecordOutcome, ProfileError> {
            Err(ProfileError::Io {
                path: PathBuf::from("profile.jsonl"),
                source: io::Error::other("injected write failure"),
            })
        }
    }

    #[test]
    fn profile_failure_is_a_dispatch_error() {
        let mut dispatcher = Dispatcher {
            blocks: HashMap::new(),
            halt_reasons: BTreeMap::new(),
            max_steps: DEFAULT_MAX_STEPS,
            emergency_jit: None,
            profile_sink: Some(Box::new(FailingProfileSink)),
            chains: crate::chain::ChainCache::default(),
            native_patch_chaining: false,
            native_incoming: HashMap::new(),
        };

        let error = dispatcher
            .record_profile(ProfileEvent::BranchTarget {
                source_pc: 0,
                target_pc: 4,
            })
            .expect_err("profile failure must stop dispatch");
        assert!(matches!(error, DispatchError::Profile(_)));
    }

    #[test]
    fn native_chain_metadata_must_match_the_emitted_slot() {
        let valid = ChainExit {
            block_entry_pc: 0x1000,
            exit_kind: ChainExitKind::UnconditionalBranch,
            patch_offset: 5,
            patch_size: nx86_x64_asm::CHAIN_EXIT_SIZE,
            original_bytes: vec![0xC3, 0x90, 0x90, 0x90, 0x90],
            successor_pc: 0x1008,
            patched: false,
        };
        assert!(chain_exit_metadata_is_valid(0x1000, 10, &valid));

        let mut wrong_owner = valid.clone();
        wrong_owner.block_entry_pc = 0x2000;
        assert!(!chain_exit_metadata_is_valid(0x1000, 10, &wrong_owner));

        let mut wrong_bytes = valid.clone();
        wrong_bytes.original_bytes[0] = 0x90;
        assert!(!chain_exit_metadata_is_valid(0x1000, 10, &wrong_bytes));

        let mut out_of_bounds = valid;
        out_of_bounds.patch_offset = 6;
        assert!(!chain_exit_metadata_is_valid(0x1000, 10, &out_of_bounds));
    }
}
