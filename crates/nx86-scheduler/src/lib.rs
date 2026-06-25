use nx86_core::guest::{CpuState, ThreadState};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CRATE_NAME: &str = "nx86-scheduler";

const DEFAULT_REPLAY_CAP: usize = 256;

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SchedulerMode {
    HostThreads,
    Fibers,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GuestThreadStatus {
    Runnable,
    Running,
    Halted,
    Crashed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct HostThreadMapping {
    pub host_thread_index: usize,
    pub fiber_slot: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct GuestThread {
    pub cpu: CpuState,
    pub status: GuestThreadStatus,
    pub host: HostThreadMapping,
    pub cpu_ticks: u64,
    remaining_work: u64,
}

impl GuestThread {
    #[must_use]
    pub fn thread_id(&self) -> u64 {
        self.cpu.thread().thread_id
    }

    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.cpu.thread().name.as_deref()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct SyntheticThread {
    pub name: String,
    pub entry_pc: u64,
    pub work_units: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct SyntheticThreadProgram {
    pub threads: Vec<SyntheticThread>,
    pub quantum: u64,
}

impl SyntheticThreadProgram {
    #[must_use]
    pub fn two_thread_counter() -> Self {
        Self {
            quantum: 1,
            threads: vec![
                SyntheticThread {
                    name: "main".to_owned(),
                    entry_pc: 0x1000,
                    work_units: 3,
                },
                SyntheticThread {
                    name: "worker".to_owned(),
                    entry_pc: 0x2000,
                    work_units: 2,
                },
            ],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct Scheduler {
    mode: SchedulerMode,
    threads: Vec<GuestThread>,
    replay: ReplayLog,
    next_thread_id: u64,
}

impl Scheduler {
    #[must_use]
    pub fn new(mode: SchedulerMode) -> Self {
        Self::with_replay_cap(mode, DEFAULT_REPLAY_CAP)
    }

    #[must_use]
    pub fn with_replay_cap(mode: SchedulerMode, replay_cap: usize) -> Self {
        Self {
            mode,
            threads: Vec::new(),
            replay: ReplayLog::new(replay_cap),
            next_thread_id: 1,
        }
    }

    #[must_use]
    pub const fn mode(&self) -> SchedulerMode {
        self.mode
    }

    #[must_use]
    pub fn threads(&self) -> &[GuestThread] {
        &self.threads
    }

    #[must_use]
    pub const fn replay(&self) -> &ReplayLog {
        &self.replay
    }

    pub fn spawn(&mut self, name: impl Into<String>, entry_pc: u64, work_units: u64) -> u64 {
        let thread_id = self.next_thread_id;
        self.next_thread_id += 1;
        let deterministic_index = self.threads.len() as u64;
        let name = name.into();

        let mut cpu = CpuState::new();
        cpu.set_pc(entry_pc);
        cpu.set_thread(ThreadState {
            thread_id,
            name: Some(name),
            deterministic_index,
        });

        let host = match self.mode {
            SchedulerMode::HostThreads => HostThreadMapping {
                host_thread_index: deterministic_index as usize,
                fiber_slot: None,
            },
            SchedulerMode::Fibers => HostThreadMapping {
                host_thread_index: 0,
                fiber_slot: Some(deterministic_index as usize),
            },
        };

        self.threads.push(GuestThread {
            cpu,
            status: GuestThreadStatus::Runnable,
            host,
            cpu_ticks: 0,
            remaining_work: work_units,
        });
        self.record(thread_id, entry_pc, ReplayEventKind::Spawned);
        thread_id
    }

    pub fn run_synthetic(
        &mut self,
        program: &SyntheticThreadProgram,
    ) -> Result<SchedulerRunReport, SchedulerError> {
        if program.threads.is_empty() {
            return Err(SchedulerError::EmptyProgram);
        }
        if program.quantum == 0 {
            return Err(SchedulerError::ZeroQuantum);
        }

        for thread in &program.threads {
            self.spawn(&thread.name, thread.entry_pc, thread.work_units);
        }

        let mut dispatch_count = 0u64;
        while self
            .threads
            .iter()
            .any(|thread| thread.status != GuestThreadStatus::Halted)
        {
            for index in 0..self.threads.len() {
                if self.threads[index].status == GuestThreadStatus::Halted {
                    continue;
                }
                self.run_thread_quantum(index, program.quantum);
                dispatch_count += 1;
            }
        }

        Ok(SchedulerRunReport {
            mode: self.mode,
            thread_count: self.threads.len(),
            dispatch_count,
            rows: self.gui_rows(),
            replay_metadata: self.replay.metadata(self.mode, self.threads.len()),
        })
    }

    pub fn crash_thread(
        &mut self,
        thread_id: u64,
        reason: impl Into<String>,
    ) -> Result<CrashWindow, SchedulerError> {
        let Some(index) = self
            .threads
            .iter()
            .position(|thread| thread.thread_id() == thread_id)
        else {
            return Err(SchedulerError::UnknownThread { thread_id });
        };

        let pc = self.threads[index].cpu.pc();
        self.threads[index].status = GuestThreadStatus::Crashed;
        self.record(
            thread_id,
            pc,
            ReplayEventKind::Crash {
                reason: reason.into(),
            },
        );
        Ok(self.crash_window(thread_id))
    }

    #[must_use]
    pub fn crash_window(&self, thread_id: u64) -> CrashWindow {
        let events: Vec<_> = self
            .replay
            .events
            .iter()
            .filter(|event| event.thread_id == thread_id)
            .cloned()
            .collect();
        CrashWindow {
            metadata: self.replay.metadata(self.mode, self.threads.len()),
            analysis: ReplayAnalysis::from_events(&events),
            events,
        }
    }

    #[must_use]
    pub fn gui_rows(&self) -> Vec<ThreadGuiRow> {
        self.threads
            .iter()
            .map(|thread| ThreadGuiRow {
                thread_id: thread.thread_id(),
                name: thread.name().unwrap_or("unnamed").to_owned(),
                status: thread.status,
                pc: thread.cpu.pc(),
                host_thread_index: thread.host.host_thread_index,
                fiber_slot: thread.host.fiber_slot,
                deterministic_index: thread.cpu.thread().deterministic_index,
                cpu_ticks: thread.cpu_ticks,
            })
            .collect()
    }

    fn run_thread_quantum(&mut self, index: usize, quantum: u64) {
        let thread_id = self.threads[index].thread_id();
        for _ in 0..quantum {
            if self.threads[index].remaining_work == 0 {
                self.halt_thread(index);
                return;
            }

            self.threads[index].status = GuestThreadStatus::Running;
            let pc = self.threads[index].cpu.pc();
            self.record(thread_id, pc, ReplayEventKind::Dispatch);
            self.threads[index].cpu_ticks += 1;
            self.threads[index].remaining_work -= 1;
            self.threads[index].cpu.set_pc(pc + 4);
        }

        if self.threads[index].remaining_work == 0 {
            self.halt_thread(index);
        } else {
            self.threads[index].status = GuestThreadStatus::Runnable;
            self.record(
                thread_id,
                self.threads[index].cpu.pc(),
                ReplayEventKind::Yield,
            );
        }
    }

    fn halt_thread(&mut self, index: usize) {
        let thread_id = self.threads[index].thread_id();
        self.threads[index].status = GuestThreadStatus::Halted;
        self.threads[index].cpu.halt("synthetic thread complete");
        self.record(
            thread_id,
            self.threads[index].cpu.pc(),
            ReplayEventKind::Halt,
        );
    }

    fn record(&mut self, thread_id: u64, pc: u64, kind: ReplayEventKind) {
        self.replay.record(thread_id, pc, kind);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct SchedulerRunReport {
    pub mode: SchedulerMode,
    pub thread_count: usize,
    pub dispatch_count: u64,
    pub rows: Vec<ThreadGuiRow>,
    pub replay_metadata: ReplayMetadata,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ThreadGuiRow {
    pub thread_id: u64,
    pub name: String,
    pub status: GuestThreadStatus,
    pub pc: u64,
    pub host_thread_index: usize,
    pub fiber_slot: Option<usize>,
    pub deterministic_index: u64,
    pub cpu_ticks: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ReplayLog {
    cap: usize,
    events: Vec<ReplayEvent>,
    next_sequence: u64,
    dropped_events: u64,
}

impl ReplayLog {
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            events: Vec::new(),
            next_sequence: 0,
            dropped_events: 0,
        }
    }

    #[must_use]
    pub fn events(&self) -> &[ReplayEvent] {
        &self.events
    }

    pub fn record(&mut self, thread_id: u64, pc: u64, kind: ReplayEventKind) {
        if self.cap == 0 {
            self.dropped_events += 1;
            self.next_sequence += 1;
            return;
        }
        if self.events.len() == self.cap {
            self.events.remove(0);
            self.dropped_events += 1;
        }
        self.events.push(ReplayEvent {
            sequence: self.next_sequence,
            thread_id,
            pc,
            kind,
        });
        self.next_sequence += 1;
    }

    #[must_use]
    pub fn metadata(&self, mode: SchedulerMode, thread_count: usize) -> ReplayMetadata {
        ReplayMetadata {
            mode,
            cap: self.cap,
            retained_events: self.events.len(),
            dropped_events: self.dropped_events,
            first_sequence: self.events.first().map(|event| event.sequence),
            last_sequence: self.events.last().map(|event| event.sequence),
            thread_count,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ReplayEvent {
    pub sequence: u64,
    pub thread_id: u64,
    pub pc: u64,
    pub kind: ReplayEventKind,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ReplayEventKind {
    Spawned,
    Dispatch,
    Yield,
    Halt,
    Crash { reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ReplayMetadata {
    pub mode: SchedulerMode,
    pub cap: usize,
    pub retained_events: usize,
    pub dropped_events: u64,
    pub first_sequence: Option<u64>,
    pub last_sequence: Option<u64>,
    pub thread_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct CrashWindow {
    pub metadata: ReplayMetadata,
    pub analysis: ReplayAnalysis,
    pub events: Vec<ReplayEvent>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ReplayAnalysis {
    pub dispatches: u64,
    pub yields: u64,
    pub halted: bool,
    pub crashed: bool,
    pub last_pc: Option<u64>,
}

impl ReplayAnalysis {
    #[must_use]
    pub fn from_events(events: &[ReplayEvent]) -> Self {
        let mut analysis = Self::default();
        for event in events {
            analysis.last_pc = Some(event.pc);
            match event.kind {
                ReplayEventKind::Dispatch => analysis.dispatches += 1,
                ReplayEventKind::Yield => analysis.yields += 1,
                ReplayEventKind::Halt => analysis.halted = true,
                ReplayEventKind::Crash { .. } => analysis.crashed = true,
                ReplayEventKind::Spawned => {}
            }
        }
        analysis
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct SchedulerComparison {
    pub host_threads: SchedulerRunReport,
    pub fibers: SchedulerRunReport,
    pub deterministic_trace_equal: bool,
}

pub fn compare_host_threads_and_fibers(
    program: &SyntheticThreadProgram,
) -> Result<SchedulerComparison, SchedulerError> {
    let mut host = Scheduler::new(SchedulerMode::HostThreads);
    let host_threads = host.run_synthetic(program)?;

    let mut fiber = Scheduler::new(SchedulerMode::Fibers);
    let fibers = fiber.run_synthetic(program)?;

    let host_trace: Vec<_> = host.replay().events().iter().map(trace_key).collect();
    let fiber_trace: Vec<_> = fiber.replay().events().iter().map(trace_key).collect();

    Ok(SchedulerComparison {
        host_threads,
        fibers,
        deterministic_trace_equal: host_trace == fiber_trace,
    })
}

fn trace_key(event: &ReplayEvent) -> (u64, u64, &'static str) {
    let kind = match event.kind {
        ReplayEventKind::Spawned => "spawned",
        ReplayEventKind::Dispatch => "dispatch",
        ReplayEventKind::Yield => "yield",
        ReplayEventKind::Halt => "halt",
        ReplayEventKind::Crash { .. } => "crash",
    };
    (event.thread_id, event.pc, kind)
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SchedulerError {
    #[error("synthetic thread program has no threads")]
    EmptyProgram,
    #[error("synthetic scheduler quantum must be greater than zero")]
    ZeroQuantum,
    #[error("guest thread {thread_id} does not exist")]
    UnknownThread { thread_id: u64 },
}

#[cfg(test)]
mod tests {
    use super::{
        GuestThreadStatus, ReplayEventKind, Scheduler, SchedulerMode, SyntheticThreadProgram,
        compare_host_threads_and_fibers,
    };

    #[test]
    fn synthetic_multi_thread_program_runs_on_host_threads() {
        let program = SyntheticThreadProgram::two_thread_counter();
        let mut scheduler = Scheduler::new(SchedulerMode::HostThreads);

        let report = scheduler
            .run_synthetic(&program)
            .expect("program should run");

        assert_eq!(report.thread_count, 2);
        assert_eq!(report.rows[0].status, GuestThreadStatus::Halted);
        assert_eq!(report.rows[0].cpu_ticks, 3);
        assert_eq!(report.rows[0].pc, 0x100c);
        assert_eq!(report.rows[1].host_thread_index, 1);
        assert!(scheduler.replay().events().iter().any(|event| {
            matches!(event.kind, ReplayEventKind::Dispatch) && event.thread_id == 2
        }));
    }

    #[test]
    fn replay_log_keeps_bounded_crash_window_metadata() {
        let program = SyntheticThreadProgram::two_thread_counter();
        let mut scheduler = Scheduler::with_replay_cap(SchedulerMode::HostThreads, 4);
        scheduler
            .run_synthetic(&program)
            .expect("program should run");

        let window = scheduler
            .crash_thread(1, "phase-37 synthetic crash")
            .expect("thread should exist");

        assert_eq!(window.metadata.cap, 4);
        assert!(window.metadata.dropped_events > 0);
        assert!(window.analysis.crashed);
        assert_eq!(window.events.last().map(|event| event.thread_id), Some(1));
    }

    #[test]
    fn fiber_mode_runs_same_deterministic_trace_as_host_threads() {
        let program = SyntheticThreadProgram::two_thread_counter();

        let comparison = compare_host_threads_and_fibers(&program).expect("comparison should run");

        assert!(comparison.deterministic_trace_equal);
        assert_eq!(comparison.fibers.rows[0].host_thread_index, 0);
        assert_eq!(comparison.fibers.rows[1].fiber_slot, Some(1));
        assert_eq!(comparison.host_threads.rows[1].host_thread_index, 1);
    }
}
