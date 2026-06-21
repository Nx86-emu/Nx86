use std::{io::Write, thread, time::Duration};

use nx86_core::ipc::{
    CompileProgress, CompletedEvent, IpcEvent, LogEvent, LogLevel, WorkerKind, encode_event,
};

#[derive(Clone, Copy, Debug)]
enum WorkerMode {
    CompilerSmoke,
    RuntimeSmoke,
    RebuildProfile,
}

impl WorkerMode {
    /// Parse the kebab-case worker name accepted on the command line. The names
    /// mirror [`WorkerKind::label`].
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "compiler-smoke" => Ok(Self::CompilerSmoke),
            "runtime-smoke" => Ok(Self::RuntimeSmoke),
            "rebuild-profile" => Ok(Self::RebuildProfile),
            other => Err(format!(
                "unknown worker mode '{other}' \
                 (expected 'compiler-smoke', 'runtime-smoke', or 'rebuild-profile')"
            )),
        }
    }
}

impl From<WorkerMode> for WorkerKind {
    fn from(value: WorkerMode) -> Self {
        match value {
            WorkerMode::CompilerSmoke => Self::CompilerSmoke,
            WorkerMode::RuntimeSmoke => Self::RuntimeSmoke,
            WorkerMode::RebuildProfile => Self::RebuildProfile,
        }
    }
}

const USAGE: &str = "\
Nx86 GUI shell and worker process entrypoint

Usage: nx86-app [--worker <compiler-smoke|runtime-smoke|rebuild-profile>]

Options:
      --worker <MODE>  Run a worker process instead of launching the GUI
  -h, --help           Print help";

/// Parse the command line for an optional `--worker <mode>` selection.
///
/// Returns `Ok(None)` for the default GUI launch, `Ok(Some(mode))` for a worker
/// run, and `Err(message)` for an invalid invocation. Accepts both the
/// space-separated (`--worker compiler-smoke`) and `=` (`--worker=compiler-smoke`)
/// forms.
fn parse_args() -> Result<Option<WorkerMode>, String> {
    let mut worker = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--worker" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--worker requires a value".to_owned())?;
                worker = Some(WorkerMode::parse(&value)?);
            }
            other => match other.strip_prefix("--worker=") {
                Some(value) => worker = Some(WorkerMode::parse(value)?),
                None => return Err(format!("unexpected argument '{other}'")),
            },
        }
    }
    Ok(worker)
}

fn main() {
    nx86_debug::logging::init_logging();

    let worker = match parse_args() {
        Ok(worker) => worker,
        Err(message) => {
            eprintln!("error: {message}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    if let Some(worker) = worker {
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
        WorkerKind::RebuildProfile => [
            "read-profile",
            "identify",
            "recompile",
            "insert",
            "coverage",
            "verify",
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
