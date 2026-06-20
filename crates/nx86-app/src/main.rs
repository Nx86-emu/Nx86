use std::{io::Write, thread, time::Duration};

use clap::{Parser, ValueEnum};
use nx86_core::ipc::{
    CompileProgress, CompletedEvent, IpcEvent, LogEvent, LogLevel, WorkerKind, encode_event,
};

#[derive(Debug, Parser)]
#[command(name = "nx86-app")]
#[command(about = "Nx86 GUI shell and worker process entrypoint")]
struct Cli {
    #[arg(long)]
    worker: Option<WorkerMode>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum WorkerMode {
    CompilerSmoke,
    RuntimeSmoke,
}

impl From<WorkerMode> for WorkerKind {
    fn from(value: WorkerMode) -> Self {
        match value {
            WorkerMode::CompilerSmoke => Self::CompilerSmoke,
            WorkerMode::RuntimeSmoke => Self::RuntimeSmoke,
        }
    }
}

fn main() {
    nx86_debug::logging::init_logging();
    let cli = Cli::parse();

    if let Some(worker) = cli.worker {
        if let Err(error) = run_worker(worker) {
            tracing::error!(%error, "worker exited with an error");
            std::process::exit(1);
        }
        return;
    }

    tracing::info!("starting Nx86 GUI");
    if let Err(error) = nx86_gui::run() {
        tracing::error!(%error, "Nx86 exited with an error");
        std::process::exit(1);
    }
}

fn run_worker(worker: WorkerMode) -> Result<(), Box<dyn std::error::Error>> {
    let kind = WorkerKind::from(worker);
    emit_event(&IpcEvent::Log(LogEvent {
        level: LogLevel::Info,
        message: format!("starting {} worker", kind.label()),
    }))?;

    let phases = match kind {
        WorkerKind::CompilerSmoke => [
            "scan", "hash", "decode", "lift", "optimize", "emit", "report",
        ],
        WorkerKind::RuntimeSmoke => [
            "prepare",
            "map",
            "launch",
            "heartbeat",
            "profile",
            "shutdown",
            "report",
        ],
    };

    for (index, phase) in phases.iter().enumerate() {
        let percent = (index as f32 / (phases.len().saturating_sub(1) as f32)) * 100.0;
        emit_event(&IpcEvent::Progress(CompileProgress {
            title_id: None,
            phase: (*phase).to_owned(),
            percent,
            current_module: Some("smoke".to_owned()),
            functions_discovered: (index as u64) * 8,
            functions_compiled: (index as u64) * 5,
            native_coverage_estimate: percent.min(100.0),
            cache_size_bytes: (index as u64) * 4096,
        }))?;
        thread::sleep(Duration::from_millis(120));
    }

    emit_event(&IpcEvent::Completed(CompletedEvent {
        job_id: kind.label().to_owned(),
        success: true,
        message: format!("{} worker completed", kind.label()),
    }))?;

    Ok(())
}

fn emit_event(event: &IpcEvent) -> Result<(), Box<dyn std::error::Error>> {
    let encoded = encode_event(event)?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(encoded.as_bytes())?;
    stdout.flush()?;
    Ok(())
}
