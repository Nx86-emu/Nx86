use std::{collections::BTreeMap, fmt, marker::PhantomData, ptr::NonNull};

use nx86_cache::CacheManager;
use nx86_core::guest::CpuState;
use nx86_ir::{Function, Terminator};
use nx86_jit::{ExecError, ExecutableMemory};
use nx86_vmm::{GuestAddress, GuestMemory, PagePermissions, VmmFault};
use nx86_x64_v4::{LoweredBlock, LoweringError, NativeBlockState, lower_tiny_block};
use thiserror::Error;

pub use nx86_cache::CacheError;
pub use nx86_jit::{EmergencyJit, JitCompilation, JitError, JitEvent};
pub use nx86_object::{NativeObject, ObjectError};
pub use nx86_profile::{
    JitBlockCandidate, MemoryAccessKind, ProfileError, ProfileEvent, ProfileLog, ProfileRecord,
    ProfileSink, ProfileWriter, RecordOutcome, read_profile,
};

mod chain;
mod dispatch;
pub use chain::{ChainStats, DEFAULT_CHAIN_THRESHOLD};
pub use dispatch::{
    DEFAULT_MAX_STEPS, DispatchError, DispatchExit, DispatchOutcome, Dispatcher,
    run_dispatched_function, run_dispatched_function_in,
};

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
    pub slowmem_counters: Option<SlowmemCounters>,
}

#[derive(Debug, Error)]
pub enum NativeMemoryError {
    #[error("native guest memory operation has no attached GuestMemory")]
    MissingContext,
    #[error("native guest memory helper received unsupported access size {size}")]
    InvalidSize { size: u64 },
    #[error("native guest memory fault at guest pc {guest_pc:#x}: {source}")]
    Vmm {
        guest_pc: u64,
        #[source]
        source: VmmFault,
    },
}

/// Aggregate slowmem access counters (SPEC 16.3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlowmemCounters {
    /// Total slowmem callback invocations.
    pub total_calls: u64,
    /// Calls grouped by reason code.
    pub by_reason: BTreeMap<String, u64>,
    /// Calls grouped by guest PC (hot source blocks).
    pub by_guest_pc: BTreeMap<u64, u64>,
}

impl SlowmemCounters {
    pub fn record(&mut self, guest_pc: u64, reason: &str) {
        self.total_calls += 1;
        *self.by_reason.entry(reason.to_owned()).or_insert(0) += 1;
        *self.by_guest_pc.entry(guest_pc).or_insert(0) += 1;
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.total_calls == 0
    }

    /// Return the `n` hottest guest PCs sorted by count descending.
    #[must_use]
    pub fn top_sources(&self, n: usize) -> Vec<(u64, u64)> {
        let mut entries: Vec<(u64, u64)> = self
            .by_guest_pc
            .iter()
            .map(|(&pc, &count)| (pc, count))
            .collect();
        entries.sort_by_key(|b| std::cmp::Reverse(b.1));
        entries.truncate(n);
        entries
    }
}

/// Structured crash report for a VMM fault (SPEC 25.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaultReport {
    pub faulting_address: u64,
    pub access_kind: MemoryAccessKind,
    pub guest_pc: u64,
    pub source_block_pc: u64,
    pub page_permissions: Option<PagePermissions>,
    pub fault_kind: &'static str,
}

impl fmt::Display for FaultReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "=== VMM FAULT ===")?;
        writeln!(f, "  Faulting address: {:#018x}", self.faulting_address)?;
        writeln!(f, "  Access type:      {:?}", self.access_kind)?;
        writeln!(f, "  Guest PC:         {:#018x}", self.guest_pc)?;
        writeln!(f, "  Source block:     {:#018x}", self.source_block_pc)?;
        writeln!(f, "  Fault kind:       {}", self.fault_kind)?;
        match &self.page_permissions {
            Some(perms) => writeln!(
                f,
                "  Page permissions: R{} W{} X{}",
                perms.read as u8, perms.write as u8, perms.execute as u8
            )?,
            None => writeln!(f, "  Page permissions: None (unmapped)")?,
        }
        write!(f, "=================")
    }
}

pub(crate) struct NativeMemoryContext<'a> {
    memory: Option<NonNull<GuestMemory>>,
    failure: Option<NativeMemoryError>,
    counters: SlowmemCounters,
    pending_events: Vec<ProfileEvent>,
    _borrow: PhantomData<&'a mut GuestMemory>,
}

impl<'a> NativeMemoryContext<'a> {
    pub(crate) fn attached(memory: &'a mut GuestMemory) -> (Self, u64, u64) {
        let (base, permissions) = memory.fastmem_view().map_or((0, 0), |view| {
            (
                view.base_address() as u64,
                view.permissions_address() as u64,
            )
        });
        (
            Self {
                memory: Some(NonNull::from(memory)),
                failure: None,
                counters: SlowmemCounters::default(),
                pending_events: Vec::new(),
                _borrow: PhantomData,
            },
            base,
            permissions,
        )
    }

    pub(crate) const fn missing() -> Self {
        Self {
            memory: None,
            failure: None,
            counters: SlowmemCounters {
                total_calls: 0,
                by_reason: BTreeMap::new(),
                by_guest_pc: BTreeMap::new(),
            },
            pending_events: Vec::new(),
            _borrow: PhantomData,
        }
    }

    pub(crate) fn configure(
        &mut self,
        state: &mut NativeBlockState,
        fastmem_base: u64,
        fastmem_permissions: u64,
    ) {
        state.fastmem_base = fastmem_base;
        state.fastmem_permissions = fastmem_permissions;
        state.slowmem_context = self as *mut Self as u64;
        state.slowmem_read = slowmem_read as *const () as usize as u64;
        state.slowmem_write = slowmem_write as *const () as usize as u64;
        state.slowmem_value = 0;
        state.memory_fault = 0;
    }

    pub(crate) fn take_failure(&mut self) -> Option<NativeMemoryError> {
        self.failure.take()
    }

    pub(crate) fn take_pending_events(&mut self) -> Vec<ProfileEvent> {
        std::mem::take(&mut self.pending_events)
    }

    pub(crate) fn counters(&self) -> &SlowmemCounters {
        &self.counters
    }

    #[allow(unsafe_code)]
    pub(crate) fn build_fault_report(
        &self,
        error: &NativeMemoryError,
        source_block_pc: u64,
    ) -> FaultReport {
        match error {
            NativeMemoryError::MissingContext => FaultReport {
                faulting_address: 0,
                access_kind: MemoryAccessKind::Read,
                guest_pc: 0,
                source_block_pc,
                page_permissions: None,
                fault_kind: "no_context",
            },
            NativeMemoryError::InvalidSize { .. } => FaultReport {
                faulting_address: 0,
                access_kind: MemoryAccessKind::Read,
                guest_pc: 0,
                source_block_pc,
                page_permissions: None,
                fault_kind: "invalid_size",
            },
            NativeMemoryError::Vmm { guest_pc, source } => {
                let (fault_kind, access_kind, faulting_address, perms) = match source {
                    VmmFault::Unmapped { address } => {
                        ("unmapped", MemoryAccessKind::Read, address.0, None)
                    }
                    VmmFault::Permission {
                        address,
                        access,
                        permissions,
                    } => {
                        let kind = match access {
                            nx86_vmm::MemoryAccess::Read => MemoryAccessKind::Read,
                            nx86_vmm::MemoryAccess::Write => MemoryAccessKind::Write,
                            nx86_vmm::MemoryAccess::Execute => MemoryAccessKind::Execute,
                        };
                        ("permission", kind, address.0, Some(*permissions))
                    }
                    VmmFault::OutOfRange { address, .. } => {
                        ("out_of_range", MemoryAccessKind::Read, address.0, None)
                    }
                    VmmFault::ArenaReservation { .. } => ("arena", MemoryAccessKind::Read, 0, None),
                    VmmFault::MirrorSourceUnmapped { address } => {
                        ("mirror_unmapped", MemoryAccessKind::Read, address.0, None)
                    }
                    VmmFault::MirrorTargetMapped { address } => {
                        ("mirror_target", MemoryAccessKind::Read, address.0, None)
                    }
                };
                let page_perms = perms.or_else(|| {
                    self.memory.and_then(|m| {
                        unsafe { m.as_ref() }.page_permissions(GuestAddress(faulting_address))
                    })
                });
                FaultReport {
                    faulting_address,
                    access_kind,
                    guest_pc: *guest_pc,
                    source_block_pc,
                    page_permissions: page_perms,
                    fault_kind,
                }
            }
        }
    }
}

#[allow(unsafe_code)]
extern "C" fn slowmem_read(
    context: *mut NativeMemoryContext<'_>,
    state: *mut NativeBlockState,
    address: u64,
    _value: u64,
    size: u64,
    guest_pc: u64,
) -> u64 {
    // SAFETY: generated code receives both pointers from `configure`; the call
    // completes synchronously while their stack owners remain alive.
    let (context, state) = unsafe { (&mut *context, &mut *state) };
    let Some(memory) = context.memory else {
        context.counters.record(guest_pc, "no_context");
        context.failure = Some(NativeMemoryError::MissingContext);
        state.pc = guest_pc;
        return 1;
    };
    let Ok(size) = usize::try_from(size) else {
        context.counters.record(guest_pc, "invalid_size");
        context.failure = Some(NativeMemoryError::InvalidSize { size });
        state.pc = guest_pc;
        return 1;
    };
    if !matches!(size, 4 | 8) {
        context.counters.record(guest_pc, "invalid_size");
        context.failure = Some(NativeMemoryError::InvalidSize { size: size as u64 });
        state.pc = guest_pc;
        return 1;
    }
    // SAFETY: the context owns the exclusive GuestMemory borrow for this run.
    let memory_ref = unsafe { memory.as_ref() };
    let reason = if memory_ref.is_mirror(GuestAddress(address)) {
        "mirror"
    } else if state.fastmem_base == 0 {
        "no_fastmem"
    } else if state.fastmem_permissions == 0 {
        "no_permissions"
    } else {
        "checked"
    };
    match memory_ref.read(GuestAddress(address), size) {
        Ok(bytes) => {
            context.counters.record(guest_pc, reason);
            let mut value = [0u8; 8];
            value[..size].copy_from_slice(&bytes);
            state.slowmem_value = u64::from_le_bytes(value);
            context.pending_events.push(ProfileEvent::Slowmem {
                guest_pc,
                address,
                size_bytes: size as u32,
                access: MemoryAccessKind::Read,
                reason_code: reason.to_owned(),
            });
            0
        }
        Err(source) => {
            let fault_reason = match &source {
                VmmFault::Unmapped { .. } => "unmapped",
                VmmFault::Permission { .. } => "permission",
                VmmFault::OutOfRange { .. } => "out_of_range",
                VmmFault::ArenaReservation { .. } => "arena",
                VmmFault::MirrorSourceUnmapped { .. } => "mirror_unmapped",
                VmmFault::MirrorTargetMapped { .. } => "mirror_target",
            };
            context.counters.record(guest_pc, fault_reason);
            context.pending_events.push(ProfileEvent::Slowmem {
                guest_pc,
                address,
                size_bytes: size as u32,
                access: MemoryAccessKind::Read,
                reason_code: fault_reason.to_owned(),
            });
            context.failure = Some(NativeMemoryError::Vmm { guest_pc, source });
            state.pc = guest_pc;
            1
        }
    }
}

#[allow(unsafe_code)]
extern "C" fn slowmem_write(
    context: *mut NativeMemoryContext<'_>,
    state: *mut NativeBlockState,
    address: u64,
    value: u64,
    size: u64,
    guest_pc: u64,
) -> u64 {
    // SAFETY: generated code receives both pointers from `configure`; the call
    // completes synchronously while their stack owners remain alive.
    let (context, state) = unsafe { (&mut *context, &mut *state) };
    let Some(memory) = context.memory else {
        context.counters.record(guest_pc, "no_context");
        context.failure = Some(NativeMemoryError::MissingContext);
        state.pc = guest_pc;
        return 1;
    };
    let Ok(size) = usize::try_from(size) else {
        context.counters.record(guest_pc, "invalid_size");
        context.failure = Some(NativeMemoryError::InvalidSize { size });
        state.pc = guest_pc;
        return 1;
    };
    if !matches!(size, 4 | 8) {
        context.counters.record(guest_pc, "invalid_size");
        context.failure = Some(NativeMemoryError::InvalidSize { size: size as u64 });
        state.pc = guest_pc;
        return 1;
    }
    // SAFETY: the context owns the exclusive GuestMemory borrow for this run.
    let memory_ref = unsafe { memory.as_ref() };
    let reason = if memory_ref.is_mirror(GuestAddress(address)) {
        "mirror"
    } else if state.fastmem_base == 0 {
        "no_fastmem"
    } else if state.fastmem_permissions == 0 {
        "no_permissions"
    } else {
        "checked"
    };
    // SAFETY: the context owns the exclusive GuestMemory borrow for this run.
    // `NonNull` is `Copy` so `memory` is still valid after the `Some` guard.
    let mut memory = memory;
    match unsafe { memory.as_mut() }.write(GuestAddress(address), &value.to_le_bytes()[..size]) {
        Ok(()) => {
            context.counters.record(guest_pc, reason);
            context.pending_events.push(ProfileEvent::Slowmem {
                guest_pc,
                address,
                size_bytes: size as u32,
                access: MemoryAccessKind::Write,
                reason_code: reason.to_owned(),
            });
            0
        }
        Err(source) => {
            let fault_reason = match &source {
                VmmFault::Unmapped { .. } => "unmapped",
                VmmFault::Permission { .. } => "permission",
                VmmFault::OutOfRange { .. } => "out_of_range",
                VmmFault::ArenaReservation { .. } => "arena",
                VmmFault::MirrorSourceUnmapped { .. } => "mirror_unmapped",
                VmmFault::MirrorTargetMapped { .. } => "mirror_target",
            };
            context.counters.record(guest_pc, fault_reason);
            context.pending_events.push(ProfileEvent::Slowmem {
                guest_pc,
                address,
                size_bytes: size as u32,
                access: MemoryAccessKind::Write,
                reason_code: fault_reason.to_owned(),
            });
            context.failure = Some(NativeMemoryError::Vmm { guest_pc, source });
            state.pc = guest_pc;
            1
        }
    }
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
            slowmem_counters: None,
        }
    }

    /// Build an outcome from a lowering failure, classifying "program shape not
    /// yet supported by the tiny path" (benign) apart from genuine failures.
    fn from_lowering_error(dump: String, error: LoweringError) -> Self {
        let status = match &error {
            LoweringError::UnsupportedBlockCount { .. }
            | LoweringError::UnsupportedOp { .. }
            | LoweringError::UnsupportedMemoryType { .. }
            | LoweringError::UnsupportedTerminator { .. } => NativeStatus::Unsupported,
            LoweringError::InvalidIr(_)
            | LoweringError::MissingResult { .. }
            | LoweringError::ValueOutOfRange { .. }
            | LoweringError::RegisterOutOfRange { .. }
            | LoweringError::StackTooLarge { .. }
            | LoweringError::AddressOverflow { .. }
            | LoweringError::UnknownBranchTarget { .. }
            | LoweringError::Assembler(_) => NativeStatus::Error,
        };
        Self {
            status,
            dump,
            final_state: None,
            error: Some(error.to_string()),
            slowmem_counters: None,
        }
    }

    fn unavailable(dump: String, error: impl ToString) -> Self {
        Self {
            status: NativeStatus::Unavailable,
            dump,
            final_state: None,
            error: Some(error.to_string()),
            slowmem_counters: None,
        }
    }
}

/// The result of a profile-guided rebuild pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebuildOutcome {
    /// Total unique `JitBlock` events found in the profile.
    pub total_jit_blocks: usize,
    /// Blocks successfully recompiled and inserted into the cache.
    pub promoted: usize,
    /// Blocks skipped because the source function has no matching block.
    pub skipped_unknown_pc: usize,
    /// Blocks that failed compilation.
    pub errors: usize,
    /// Diagnostic messages from failed compilations (one per error).
    pub error_messages: Vec<String>,
    /// Snapshot of native coverage after the rebuild.
    pub coverage: CoverageSnapshot,
}

/// Native coverage metrics derived from a rebuild pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoverageSnapshot {
    /// Blocks promoted from JIT to AOT by this rebuild.
    pub promoted_blocks: usize,
    /// Total native objects in the cache after rebuild.
    pub total_native_blocks: usize,
    /// Profile blocks whose source PC has no matching block (unpromotable).
    pub unpromotable_unknown_pc: usize,
    /// Profile blocks that failed compilation (potentially transient).
    pub failed_compilation: usize,
    /// Slowmem calls observed in the profile (SPEC 16.3).
    pub slowmem_calls: u64,
    /// Total memory accesses (fast + slow) if known; 0 = not tracked.
    pub total_accesses: u64,
}

/// A failure during profile-guided rebuild.
#[derive(Debug, Error)]
pub enum RebuildError {
    #[error("cache scan during rebuild failed: {0}")]
    Cache(#[source] CacheError),
}

/// Read a runtime profile and recompile every JIT-discovered block through the
/// AOT pipeline, inserting promoted objects into the cache. The source function
/// is taken from `jit`, which must have been created with the same function that
/// produced the profile.
pub fn rebuild_from_profile(
    profile: &ProfileLog,
    jit: &EmergencyJit,
    cache: &CacheManager,
) -> Result<RebuildOutcome, RebuildError> {
    let candidates = profile.jit_block_candidates();
    let total_jit_blocks = candidates.len();
    let mut promoted = 0usize;
    let mut skipped_unknown_pc = 0usize;
    let mut errors = 0usize;
    let mut error_messages = Vec::new();

    for candidate in &candidates {
        match jit.compile(candidate.guest_pc) {
            Ok(Some(_compilation)) => {
                promoted += 1;
            }
            Ok(None) => {
                skipped_unknown_pc += 1;
            }
            Err(error) => {
                errors += 1;
                error_messages.push(format!("{:#x}: {error}", candidate.guest_pc));
            }
        }
    }

    let total_native_blocks = cache.scan().map_err(RebuildError::Cache)?.object_count();

    let slowmem_calls = profile
        .records
        .iter()
        .filter(|r| matches!(r.event, ProfileEvent::Slowmem { .. }))
        .count() as u64;

    Ok(RebuildOutcome {
        total_jit_blocks,
        promoted,
        skipped_unknown_pc,
        errors,
        error_messages,
        coverage: CoverageSnapshot {
            promoted_blocks: promoted,
            total_native_blocks,
            unpromotable_unknown_pc: skipped_unknown_pc,
            failed_compilation: errors,
            slowmem_calls,
            // Fastmem access counting requires per-block instrumentation (future
            // phase); only slowmem is observable from profile events today.
            total_accesses: 0,
        },
    })
}

pub fn run_tiny_native_block(
    function: &Function,
    initial_state: &CpuState,
    interpreter_state: &CpuState,
) -> NativeOutcome {
    run_tiny_native_block_with_memory(function, initial_state, interpreter_state, None)
}

/// Execute one lowered block with guest memory attached for fastmem and checked
/// slowmem fallback.
pub fn run_tiny_native_block_in(
    function: &Function,
    initial_state: &CpuState,
    interpreter_state: &CpuState,
    memory: &mut GuestMemory,
) -> NativeOutcome {
    run_tiny_native_block_with_memory(function, initial_state, interpreter_state, Some(memory))
}

fn run_tiny_native_block_with_memory(
    function: &Function,
    initial_state: &CpuState,
    interpreter_state: &CpuState,
    memory: Option<&mut GuestMemory>,
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
    let (mut memory_context, fastmem_base, fastmem_permissions) = match memory {
        Some(memory) => NativeMemoryContext::attached(memory),
        None => (NativeMemoryContext::missing(), 0, 0),
    };
    memory_context.configure(&mut native_state, fastmem_base, fastmem_permissions);
    if let Err(error) = call_generated_block(&executable, &mut native_state) {
        return NativeOutcome::error(dump, error);
    }
    if native_state.memory_fault != 0 {
        return NativeOutcome::error(
            dump,
            memory_context
                .take_failure()
                .unwrap_or(NativeMemoryError::MissingContext),
        );
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
        slowmem_counters: Some(memory_context.counters().clone()),
    }
}

fn halt_reason(function: &Function) -> Option<&str> {
    let block = function.blocks.first()?;
    match &block.terminator {
        Terminator::Halt { reason } => Some(reason.as_str()),
        Terminator::Branch { .. }
        | Terminator::CondBranch { .. }
        | Terminator::Guard { .. }
        | Terminator::Return => None,
    }
}

#[allow(unsafe_code)]
fn call_generated_block(
    executable: &ExecutableMemory,
    state: &mut NativeBlockState,
) -> Result<(), ExecError> {
    // SAFETY: the bytes behind `executable` are either produced directly by the
    // trusted `nx86-x64-v4` lowerer or admitted through `Dispatcher::from_objects`,
    // whose unsafe contract requires that provenance. The lowerer emits an
    // `extern "C" fn(*mut NativeBlockState)` block that reads and writes only
    // fields within the provided state pointer.
    unsafe { executable.call_with_state(state) }
}

#[cfg(test)]
mod tests {
    use nx86_core::guest::CpuState;
    use nx86_ir::{Block, BlockId, Function, Inst, Op, Reg, Terminator, Type, Value};

    use super::{
        CoverageSnapshot, NativeObject, NativeStatus, RebuildOutcome, lower_tiny_block,
        native_object, rebuild_from_profile, run_dispatched_function, run_tiny_native_block,
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

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn native_memory_uses_fastmem_and_slowmem_fallback() {
        use nx86_vmm::{GuestAddress, GuestMemory, PagePermissions};

        let function = tiny_memory_function();
        let initial = CpuState::new();
        let mut expected = CpuState::new();
        expected.set_x(0, 0x1122_3344_5566_7788);
        expected.set_pc(20);
        expected.halt("svc #0x0");

        let mut fastmem = GuestMemory::new().expect("fastmem arena");
        fastmem
            .map_page(GuestAddress(0x1000), PagePermissions::READ_WRITE)
            .expect("fastmem page");
        let fast = super::run_tiny_native_block_in(&function, &initial, &expected, &mut fastmem);
        assert_eq!(
            fast.status,
            NativeStatus::MatchesInterpreter,
            "{:?}",
            fast.error
        );
        assert_eq!(
            fastmem.read(GuestAddress(0x1000), 8).expect("stored bytes"),
            0x1122_3344_5566_7788u64.to_le_bytes()
        );

        let mut slowmem = GuestMemory::new_logical();
        slowmem
            .map_page(GuestAddress(0x1000), PagePermissions::READ_WRITE)
            .expect("logical page");
        let slow = super::run_tiny_native_block_in(&function, &initial, &expected, &mut slowmem);
        assert_eq!(
            slow.status,
            NativeStatus::MatchesInterpreter,
            "{:?}",
            slow.error
        );

        let mut unmapped = GuestMemory::new().expect("fastmem arena");
        let fault = super::run_tiny_native_block_in(&function, &initial, &expected, &mut unmapped);
        assert_eq!(fault.status, NativeStatus::Error);
        assert!(
            fault
                .error
                .as_deref()
                .is_some_and(|error| error.contains("not mapped"))
        );
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
            deopt_points: Vec::new(),
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

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn tiny_memory_function() -> Function {
        Function {
            name: "tiny_memory".to_owned(),
            entry_address: 0,
            value_count: 3,
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
                        op: Op::Const {
                            ty: Type::I64,
                            value: 0x1122_3344_5566_7788,
                        },
                        guest_address: 4,
                    },
                    Inst {
                        result: None,
                        op: Op::Store {
                            ty: Type::I64,
                            address: Value(0),
                            value: Value(1),
                        },
                        guest_address: 8,
                    },
                    Inst {
                        result: Some(Value(2)),
                        op: Op::Load {
                            ty: Type::I64,
                            address: Value(0),
                        },
                        guest_address: 12,
                    },
                    Inst {
                        result: None,
                        op: Op::SetReg {
                            reg: Reg::X(0),
                            value: Value(2),
                        },
                        guest_address: 12,
                    },
                ],
                terminator: Terminator::Halt {
                    reason: "svc #0x0".to_owned(),
                },
                terminator_address: 16,
            }],
        }
    }

    /// Two straight-line blocks connected by an unconditional branch: block 0
    /// sets x0 = 5 and branches to block 1 (at guest PC 0x8), which copies x0
    /// into x1 and halts. Final state: x0 = x1 = 5, pc = 0xC, halted.
    fn two_block_branch_function() -> Function {
        Function {
            name: "two_block".to_owned(),
            entry_address: 0,
            value_count: 2,
            deopt_points: Vec::new(),
            blocks: vec![
                Block {
                    instructions: vec![
                        Inst {
                            result: Some(Value(0)),
                            op: Op::Const {
                                ty: Type::I64,
                                value: 5,
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
                    ],
                    terminator: Terminator::Branch { target: BlockId(1) },
                    terminator_address: 4,
                },
                Block {
                    instructions: vec![
                        Inst {
                            result: Some(Value(1)),
                            op: Op::GetReg { reg: Reg::X(0) },
                            guest_address: 8,
                        },
                        Inst {
                            result: None,
                            op: Op::SetReg {
                                reg: Reg::X(1),
                                value: Value(1),
                            },
                            guest_address: 8,
                        },
                    ],
                    terminator: Terminator::Halt {
                        reason: "svc #0x0".to_owned(),
                    },
                    terminator_address: 8,
                },
            ],
        }
    }

    fn two_block_expected_state() -> CpuState {
        let mut expected = CpuState::new();
        expected.set_x(0, 5);
        expected.set_x(1, 5);
        expected.set_pc(12);
        expected.halt("svc #0x0");
        expected
    }

    #[cfg(all(
        target_os = "linux",
        target_arch = "x86_64",
        feature = "native-patch-chaining"
    ))]
    fn three_block_forward_function() -> Function {
        Function {
            name: "three_block_forward".to_owned(),
            entry_address: 0,
            value_count: 2,
            deopt_points: Vec::new(),
            blocks: vec![
                Block {
                    instructions: vec![
                        Inst {
                            result: Some(Value(0)),
                            op: Op::Const {
                                ty: Type::I64,
                                value: 5,
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
                    ],
                    terminator: Terminator::Branch { target: BlockId(1) },
                    terminator_address: 4,
                },
                Block {
                    instructions: vec![
                        Inst {
                            result: Some(Value(1)),
                            op: Op::GetReg { reg: Reg::X(0) },
                            guest_address: 8,
                        },
                        Inst {
                            result: None,
                            op: Op::SetReg {
                                reg: Reg::X(1),
                                value: Value(1),
                            },
                            guest_address: 8,
                        },
                    ],
                    terminator: Terminator::Branch { target: BlockId(2) },
                    terminator_address: 8,
                },
                Block {
                    instructions: Vec::new(),
                    terminator: Terminator::Halt {
                        reason: "svc #0x0".to_owned(),
                    },
                    terminator_address: 12,
                },
            ],
        }
    }

    #[cfg(all(
        target_os = "linux",
        target_arch = "x86_64",
        feature = "native-patch-chaining"
    ))]
    fn three_block_expected_state() -> CpuState {
        let mut expected = CpuState::new();
        expected.set_x(0, 5);
        expected.set_x(1, 5);
        expected.set_pc(16);
        expected.halt("svc #0x0");
        expected
    }

    /// Per-block native objects for `function`, as the cache would persist them.
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn objects_for(function: &Function) -> Vec<NativeObject> {
        nx86_x64_v4::lower_function(function)
            .expect("function should lower")
            .into_iter()
            .map(|block| NativeObject {
                entry_address: block.entry_pc,
                guest_end: block.entry_pc,
                stack_size: u32::try_from(block.lowered.stack_size()).unwrap_or(0),
                code: block.lowered.bytes().to_vec(),
            })
            .collect()
    }

    #[test]
    fn dispatcher_runs_two_blocks_or_reports_host() {
        let function = two_block_branch_function();
        let mut initial = CpuState::new();
        initial.set_pc(0);

        let outcome = run_dispatched_function(&function, &initial, &two_block_expected_state());

        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        assert_eq!(outcome.status, NativeStatus::MatchesInterpreter);

        #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
        assert_eq!(outcome.status, NativeStatus::Unavailable);

        assert!(!outcome.dump.is_empty());
    }

    #[test]
    fn dispatcher_rejects_duplicate_object_entries_before_execution() {
        let object = NativeObject {
            entry_address: 0x1000,
            guest_end: 0x1004,
            stack_size: 0,
            code: vec![0xc3],
        };
        // SAFETY: `ret` is valid trusted code for the state-pointer ABI and
        // touches no memory. Duplicate validation also occurs before mapping.
        #[allow(unsafe_code)]
        let error = unsafe { super::Dispatcher::from_objects([&object, &object]) }
            .expect_err("duplicate entries must be rejected");

        assert!(matches!(
            error,
            super::DispatchError::DuplicateBlock { pc: 0x1000 }
        ));
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn dispatcher_routes_blocks_loaded_from_cache() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cache = nx86_cache::CacheManager::open(dir.path()).expect("open cache");
        let function = two_block_branch_function();

        // Persist each block as a cached object, then reload and dispatch them.
        for object in objects_for(&function) {
            cache.insert(&object).expect("insert object");
        }
        let manifest = cache.scan().expect("scan");
        let loaded: Vec<NativeObject> = manifest
            .entries
            .iter()
            .map(|entry| cache.load(entry.entry_address).expect("load object"))
            .collect();

        // SAFETY: every object was emitted by `objects_for`, persisted in this
        // test's private temporary cache, and loaded without outside mutation.
        #[allow(unsafe_code)]
        let mut dispatcher =
            unsafe { super::Dispatcher::from_objects(loaded.iter()) }.expect("build dispatcher");
        assert_eq!(dispatcher.block_count(), 2);

        let mut initial = CpuState::new();
        initial.set_pc(0);
        let outcome = dispatcher
            .run(&initial, Some("svc #0x0"))
            .expect("dispatch run");

        assert_eq!(outcome.exit, super::DispatchExit::Halted);
        assert_eq!(outcome.final_state, two_block_expected_state());
    }

    #[cfg(all(
        target_os = "linux",
        target_arch = "x86_64",
        feature = "native-patch-chaining"
    ))]
    #[test]
    fn hot_forward_edge_executes_natively_and_unpatches_on_invalidation() {
        let function = three_block_forward_function();
        let mut dispatcher = super::Dispatcher::from_function(&function)
            .expect("build dispatcher")
            .with_native_patch_chaining(true);
        let mut initial = CpuState::new();
        initial.set_pc(0);

        let first = dispatcher.run(&initial, None).expect("first warmup run");
        assert_eq!(first.final_state, three_block_expected_state());
        assert_eq!(first.chain_stats.native_installs, 0);

        let second = dispatcher.run(&initial, None).expect("second warmup run");
        assert_eq!(second.final_state, three_block_expected_state());
        assert_eq!(second.chain_stats.native_installs, 1);

        let chained = dispatcher.run(&initial, None).expect("native chained run");
        assert_eq!(chained.final_state, three_block_expected_state());
        assert_eq!(chained.chain_stats.native_chain_entries, 1);

        dispatcher
            .set_native_patch_chaining(false)
            .expect("disable native chaining");
        let native_disabled = dispatcher.run(&initial, None).expect("software-only run");
        assert_eq!(native_disabled.final_state, three_block_expected_state());
        assert_eq!(native_disabled.chain_stats.native_chain_entries, 0);

        dispatcher
            .set_native_patch_chaining(true)
            .expect("re-enable native chaining");
        let rearmed = dispatcher.run(&initial, None).expect("re-arm native chain");
        assert_eq!(rearmed.chain_stats.native_installs, 1);
        let rechained = dispatcher.run(&initial, None).expect("re-chained run");
        assert_eq!(rechained.chain_stats.native_chain_entries, 1);

        dispatcher.invalidate(8).expect("invalidate chained target");
        assert_eq!(dispatcher.chain_stats().invalidations, 1);
        let restored = dispatcher
            .run(&initial, None)
            .expect("restored dispatch run");
        assert_eq!(restored.final_state, three_block_expected_state());
        assert_eq!(restored.chain_stats.native_chain_entries, 0);

        dispatcher
            .set_chaining_enabled(false)
            .expect("disable all chaining");
        let debug_run = dispatcher.run(&initial, None).expect("debug dispatch run");
        assert_eq!(debug_run.chain_stats, super::ChainStats::default());
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn dispatcher_uses_the_reached_blocks_halt_reason() {
        let function = Function {
            name: "two_halts".to_owned(),
            entry_address: 0,
            value_count: 0,
            deopt_points: Vec::new(),
            blocks: vec![
                Block {
                    instructions: Vec::new(),
                    terminator: Terminator::Halt {
                        reason: "first halt".to_owned(),
                    },
                    terminator_address: 0,
                },
                Block {
                    instructions: Vec::new(),
                    terminator: Terminator::Halt {
                        reason: "second halt".to_owned(),
                    },
                    terminator_address: 8,
                },
            ],
        };
        let mut dispatcher = super::Dispatcher::from_function(&function).expect("build dispatcher");
        let mut initial = CpuState::new();
        initial.set_pc(8);

        let outcome = dispatcher.run(&initial, None).expect("dispatch run");

        assert_eq!(outcome.exit, super::DispatchExit::Halted);
        assert_eq!(outcome.final_state.halt_reason(), Some("second halt"));
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn dispatcher_reports_missing_block() {
        // Register only the entry block, so routing to block 1 finds no block.
        let function = two_block_branch_function();
        let objects = objects_for(&function);
        // SAFETY: `objects_for` directly wraps bytes from the trusted lowerer;
        // no persistence or external mutation occurs in this test.
        #[allow(unsafe_code)]
        let mut dispatcher =
            unsafe { super::Dispatcher::from_objects(&objects[..1]) }.expect("build dispatcher");

        let mut initial = CpuState::new();
        initial.set_pc(0);
        let outcome = dispatcher.run(&initial, None).expect("dispatch run");

        assert_eq!(outcome.exit, super::DispatchExit::MissingBlock { pc: 0x8 });
        assert_eq!(outcome.chain_stats.software_installs, 0);
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn dispatcher_jits_missing_block_caches_it_and_continues() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cache = nx86_cache::CacheManager::open(dir.path()).expect("open cache");
        let profile_path = dir.path().join("runtime-v1.jsonl");
        let profile_writer =
            nx86_profile::ProfileWriter::open(&profile_path).expect("open runtime profile");
        let function = two_block_branch_function();
        let objects = objects_for(&function);
        // SAFETY: `objects_for` directly wraps bytes from the trusted lowerer;
        // no persistence or external mutation occurs before construction.
        #[allow(unsafe_code)]
        let dispatcher =
            unsafe { super::Dispatcher::from_objects(&objects[..1]) }.expect("build dispatcher");
        let jit = nx86_jit::EmergencyJit::new(function, cache.clone()).expect("create JIT");
        let mut dispatcher = dispatcher
            .with_emergency_jit(jit)
            .with_profile_sink(profile_writer);

        let mut initial = CpuState::new();
        initial.set_pc(0);
        let outcome = dispatcher.run(&initial, None).expect("dispatch with JIT");

        assert_eq!(outcome.exit, super::DispatchExit::Halted);
        assert_eq!(outcome.final_state, two_block_expected_state());
        assert_eq!(outcome.jit_events.len(), 1);
        assert_eq!(outcome.jit_events[0].guest_pc, 0x8);
        assert_eq!(cache.load(0x8).expect("load JIT object").entry_address, 0x8);
        let second = dispatcher
            .run(&initial, None)
            .expect("dispatch installed block");
        assert_eq!(second.exit, super::DispatchExit::Halted);
        assert!(second.jit_events.is_empty());
        drop(dispatcher);

        let profile = nx86_profile::read_profile(profile_path).expect("read runtime profile");
        assert_eq!(profile.records.len(), 2);
        assert!(matches!(
            profile.records[0].event,
            nx86_profile::ProfileEvent::BranchTarget {
                source_pc: 0,
                target_pc: 0x8
            }
        ));
        assert!(matches!(
            &profile.records[1].event,
            nx86_profile::ProfileEvent::JitBlock {
                guest_pc: 0x8,
                cache_file_name,
                ..
            } if cache_file_name == "0000000000000008.nxo"
        ));
    }

    #[test]
    fn rebuild_from_profile_promotes_jit_blocks() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cache = nx86_cache::CacheManager::open(dir.path()).expect("open cache");
        let function = two_block_branch_function();
        let jit = nx86_jit::EmergencyJit::new(function, cache.clone()).expect("create JIT");

        let profile = nx86_profile::ProfileLog {
            records: vec![nx86_profile::ProfileRecord::new(
                nx86_profile::ProfileEvent::JitBlock {
                    guest_pc: 0x8,
                    code_size_bytes: 42,
                    cache_file_name: "0000000000000008.nxo".to_owned(),
                },
            )],
            recovered_truncated_tail: false,
        };

        let outcome = rebuild_from_profile(&profile, &jit, &cache).expect("rebuild");

        assert_eq!(
            outcome,
            RebuildOutcome {
                total_jit_blocks: 1,
                promoted: 1,
                skipped_unknown_pc: 0,
                errors: 0,
                error_messages: Vec::new(),
                coverage: CoverageSnapshot {
                    promoted_blocks: 1,
                    total_native_blocks: 1,
                    unpromotable_unknown_pc: 0,
                    failed_compilation: 0,
                    slowmem_calls: 0,
                    total_accesses: 0,
                },
            }
        );
        assert_eq!(
            cache.load(0x8).expect("load promoted object").entry_address,
            0x8
        );
    }

    #[test]
    fn rebuild_from_profile_skips_unknown_pc() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cache = nx86_cache::CacheManager::open(dir.path()).expect("open cache");
        let function = two_block_branch_function();
        let jit = nx86_jit::EmergencyJit::new(function, cache.clone()).expect("create JIT");

        let profile = nx86_profile::ProfileLog {
            records: vec![nx86_profile::ProfileRecord::new(
                nx86_profile::ProfileEvent::JitBlock {
                    guest_pc: 0xdead,
                    code_size_bytes: 16,
                    cache_file_name: "000000000000dead.nxo".to_owned(),
                },
            )],
            recovered_truncated_tail: false,
        };

        let outcome = rebuild_from_profile(&profile, &jit, &cache).expect("rebuild");

        assert_eq!(outcome.promoted, 0);
        assert_eq!(outcome.skipped_unknown_pc, 1);
        assert_eq!(outcome.coverage.unpromotable_unknown_pc, 1);
        assert_eq!(outcome.coverage.failed_compilation, 0);
        assert_eq!(cache.scan().expect("scan").object_count(), 0);
    }

    #[test]
    fn rebuild_from_profile_handles_empty_profile() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cache = nx86_cache::CacheManager::open(dir.path()).expect("open cache");
        let function = two_block_branch_function();
        let jit = nx86_jit::EmergencyJit::new(function, cache.clone()).expect("create JIT");

        let profile = nx86_profile::ProfileLog::default();
        let outcome = rebuild_from_profile(&profile, &jit, &cache).expect("rebuild");

        assert_eq!(outcome.total_jit_blocks, 0);
        assert_eq!(outcome.promoted, 0);
        assert_eq!(outcome.coverage.total_native_blocks, 0);
    }

    #[test]
    fn rebuild_from_profile_counts_compilation_errors() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cache = nx86_cache::CacheManager::open(dir.path()).expect("open cache");
        // Build a function with an out-of-range register (x255) at guest_pc 0x0.
        // The verifier does not check register indices, so `EmergencyJit::new`
        // succeeds, but lowering fails with `RegisterOutOfRange`.
        let function = Function {
            name: "bad_reg".to_owned(),
            entry_address: 0,
            value_count: 1,
            deopt_points: Vec::new(),
            blocks: vec![Block {
                instructions: vec![Inst {
                    result: Some(Value(0)),
                    op: Op::GetReg { reg: Reg::X(255) },
                    guest_address: 0,
                }],
                terminator: Terminator::Halt {
                    reason: "svc #0x0".to_owned(),
                },
                terminator_address: 4,
            }],
        };
        let jit = nx86_jit::EmergencyJit::new(function, cache.clone()).expect("create JIT");

        let profile = nx86_profile::ProfileLog {
            records: vec![nx86_profile::ProfileRecord::new(
                nx86_profile::ProfileEvent::JitBlock {
                    guest_pc: 0x0,
                    code_size_bytes: 16,
                    cache_file_name: "0000000000000000.nxo".to_owned(),
                },
            )],
            recovered_truncated_tail: false,
        };

        let outcome = rebuild_from_profile(&profile, &jit, &cache).expect("rebuild");

        assert_eq!(outcome.total_jit_blocks, 1);
        assert_eq!(outcome.promoted, 0);
        assert_eq!(outcome.errors, 1);
        assert_eq!(outcome.error_messages.len(), 1);
        assert!(
            outcome.error_messages[0].contains("register"),
            "error should mention register: {}",
            outcome.error_messages[0]
        );
        assert_eq!(outcome.coverage.failed_compilation, 1);
        assert_eq!(outcome.coverage.unpromotable_unknown_pc, 0);
    }

    #[test]
    fn rebuild_from_profile_is_idempotent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cache = nx86_cache::CacheManager::open(dir.path()).expect("open cache");
        let function = two_block_branch_function();
        let jit = nx86_jit::EmergencyJit::new(function, cache.clone()).expect("create JIT");

        let profile = nx86_profile::ProfileLog {
            records: vec![nx86_profile::ProfileRecord::new(
                nx86_profile::ProfileEvent::JitBlock {
                    guest_pc: 0x8,
                    code_size_bytes: 42,
                    cache_file_name: "0000000000000008.nxo".to_owned(),
                },
            )],
            recovered_truncated_tail: false,
        };

        let first = rebuild_from_profile(&profile, &jit, &cache).expect("first rebuild");
        let second = rebuild_from_profile(&profile, &jit, &cache).expect("second rebuild");

        assert_eq!(first.promoted, 1);
        assert_eq!(second.promoted, 1);
        assert_eq!(
            cache
                .load(0x8)
                .expect("load after second rebuild")
                .entry_address,
            0x8
        );
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    #[allow(unsafe_code)]
    fn promoted_blocks_skip_jit_on_second_run() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cache = nx86_cache::CacheManager::open(dir.path()).expect("open cache");
        let profile_path = dir.path().join("runtime-v1.jsonl");
        let function = two_block_branch_function();

        // Run 1: load only block 0, let block 1 get JIT-compiled and profiled.
        let objects = objects_for(&function);
        cache.insert(&objects[0]).expect("cache AOT entry block");
        // SAFETY: `objects_for` directly wraps bytes from the trusted lowerer;
        // no persistence or external mutation occurs before construction.
        let dispatcher =
            unsafe { super::Dispatcher::from_objects(&objects[..1]) }.expect("build dispatcher");
        let jit = nx86_jit::EmergencyJit::new(function, cache.clone()).expect("create JIT");
        let profile_writer =
            nx86_profile::ProfileWriter::open(&profile_path).expect("open runtime profile");
        let mut dispatcher = dispatcher
            .with_emergency_jit(jit)
            .with_profile_sink(profile_writer);

        let mut initial = CpuState::new();
        initial.set_pc(0);
        let outcome = dispatcher.run(&initial, None).expect("run 1");
        assert_eq!(outcome.exit, super::DispatchExit::Halted);
        assert_eq!(outcome.jit_events.len(), 1);
        assert_eq!(outcome.jit_events[0].guest_pc, 0x8);
        drop(dispatcher);

        // Rebuild: read the profile and promote JIT blocks to AOT.
        let profile = nx86_profile::read_profile(&profile_path).expect("read profile");
        let jit_blocks = profile.jit_block_candidates();
        assert_eq!(jit_blocks.len(), 1);
        assert_eq!(jit_blocks[0].guest_pc, 0x8);

        let function = two_block_branch_function();
        let jit =
            nx86_jit::EmergencyJit::new(function, cache.clone()).expect("create JIT for rebuild");
        let outcome = rebuild_from_profile(&profile, &jit, &cache).expect("rebuild");
        assert_eq!(outcome.promoted, 1);

        // Run 2: load the original AOT entry block and promoted block 1 from
        // cache. The complete cache should execute without emergency JIT.
        let manifest = cache.scan().expect("scan cache");
        let loaded: Vec<NativeObject> = manifest
            .entries
            .iter()
            .map(|entry| cache.load(entry.entry_address).expect("load object"))
            .collect();
        // SAFETY: every object was emitted by the trusted lowerer through
        // `EmergencyJit::compile`, persisted in this test's private temporary
        // cache, and loaded without outside mutation.
        let mut dispatcher = unsafe { super::Dispatcher::from_objects(loaded.iter()) }
            .expect("build dispatcher from cache");

        let mut initial = CpuState::new();
        initial.set_pc(0);
        let outcome = dispatcher.run(&initial, Some("svc #0x0")).expect("run 2");
        assert_eq!(outcome.exit, super::DispatchExit::Halted);
        assert_eq!(outcome.final_state, two_block_expected_state());
        assert!(
            outcome.jit_events.is_empty(),
            "second run should use promoted objects, not JIT"
        );
    }
}
