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
        // Shaders lag CPU compilation early in the run; the min-gated headline
        // (Phase 50) is therefore gated by the shader axis until it catches up.
        let cpu_functional = percent.min(100.0);
        let shader_readiness = (percent * 0.8).min(100.0);
        let coverage = nx86_core::coverage::NativeCoverage::new(
            (cpu_functional * 100.0) as u16,
            (shader_readiness * 100.0) as u16,
        );
        emit_event(&IpcEvent::Progress(CompileProgress {
            title_id: None,
            phase: (*phase).to_owned(),
            percent,
            current_module: Some("smoke".to_owned()),
            functions_discovered: (index as u64) * 8,
            functions_compiled: (index as u64) * 5,
            native_coverage_estimate: coverage.combined_percent(),
            native_coverage_static: (percent * 0.9).min(100.0),
            native_coverage_executed: percent.min(100.0),
            fastmem_coverage: (100.0 - (index as f32 * 3.0)).max(0.0),
            slowmem_penalty: (index as f32 * 3.0).min(100.0),
            // Report the bps-derived shader axis (not the raw `shader_readiness`
            // local) so it is consistent with the bps-truncated headline; this
            // keeps the GUI's "gated by" comparison exact.
            shader_readiness: coverage.shader_readiness_percent(),
            cache_size_bytes: (index as u64) * 4096,
        }))?;
        thread::sleep(Duration::from_millis(120));
    }

    // The isolated runtime process owns the renderer (Phase 48). Produce a frame
    // and report it over the versioned JSON-line IPC, exercising the GPU path on
    // Linux and the deterministic software fallback elsewhere.
    if matches!(kind, WorkerKind::RuntimeSmoke) {
        emit_event(&IpcEvent::Log(render_demo_report()))?;
    }

    // The initial compile pipeline compiles shaders where possible (SPEC §14.1).
    // Phase 49 exercises that as a skeleton: translate a synthetic shader to a
    // placeholder and cache it, reporting the result over the JSON-line IPC.
    if matches!(kind, WorkerKind::CompilerSmoke) {
        emit_event(&IpcEvent::Log(shader_compile_report()))?;
    }

    emit_event(&IpcEvent::Completed(CompletedEvent {
        job_id: kind.label().to_owned(),
        success: true,
        message: format!("{} worker completed", kind.label()),
    }))?;

    Ok(())
}

/// Render the Phase 48 demonstration frame and summarize it as a log event.
fn render_demo_report() -> LogEvent {
    let renderer = nx86_gpu::Renderer::new();
    let frame = renderer.render_demo(16, 12);
    let painted = frame
        .bytes
        .chunks_exact(4)
        .filter(|pixel| *pixel != nx86_gpu::BACKGROUND)
        .count();
    LogEvent {
        level: LogLevel::Info,
        message: format!(
            "rendered {}x{} frame via {} ({painted} foreground px)",
            frame.width,
            frame.height,
            renderer.backend().label(),
        ),
    }
}

/// Batch-compile a title's shader set during initial compile and summarize how
/// shader readiness gates Native Coverage (Phase 50). Caches into a temporary
/// directory because the smoke worker has no title context; failures degrade to
/// a status string rather than aborting the worker.
fn shader_compile_report() -> LogEvent {
    let message =
        compile_shader_set().unwrap_or_else(|error| format!("shader AOT failed: {error}"));
    LogEvent {
        level: LogLevel::Info,
        message,
    }
}

fn compile_shader_set() -> Result<String, Box<dyn std::error::Error>> {
    use nx86_core::coverage::{COVERAGE_FULL_BPS, NativeCoverage};

    // Build the AOT input set from the clean-room sample shaders, hinting the
    // first one hot so the shared-profile ordering is exercised.
    let shaders = nx86_testsuite::SyntheticShader::sample_set();
    let mut inputs = Vec::with_capacity(shaders.len());
    for shader in &shaders {
        let stage: nx86_shader::ShaderStage = shader.metadata.stage.parse()?;
        inputs.push(nx86_shader::ShaderAotInput::new(
            stage,
            shader.source.bytes.clone(),
            shader.metadata.entry.clone(),
        ));
    }
    let hints = match inputs.first() {
        Some(first) => {
            nx86_shader::ShaderProfileHints::from_entries(vec![nx86_shader::ShaderHint {
                source_hash: first.source_hash(),
                stage: first.stage,
                hot: true,
            }])
        }
        None => nx86_shader::ShaderProfileHints::new(),
    };
    let known: Vec<nx86_shader::ShaderHash> =
        inputs.iter().map(|input| input.source_hash()).collect();

    let dir = tempfile::tempdir()?;
    let cache = nx86_shader::ShaderCache::open(dir.path())?;

    // Assume a fully CPU-ready title so the headline is gated purely by the
    // shader axis (min-gate), making the shader contribution visible.
    let cpu_bps = COVERAGE_FULL_BPS;

    // Before the AOT pass, no shaders are cached: readiness 0% gates the
    // combined Native Coverage to 0% even though the CPU axis is full.
    let before_bps = nx86_shader::cached_readiness_bps(&cache, &known)?;
    let before = NativeCoverage::new(cpu_bps, before_bps);

    let report = nx86_shader::compile_shaders(&inputs, &hints, &cache)?;
    let after = NativeCoverage::new(cpu_bps, report.readiness_bps);

    Ok(format!(
        "shader AOT: compiled {}/{} shaders (readiness {:.2}%, hot {}/{}); \
         Native Coverage {:.2}% [{}] -> {:.2}% [{}] (CPU 100% assumed)",
        report.cached_ok,
        report.total,
        report.readiness_percent(),
        report.hot_cached_ok,
        report.hot_total,
        before.combined_percent(),
        before.band().label(),
        after.combined_percent(),
        after.band().label(),
    ))
}

fn emit_event(event: &IpcEvent) -> Result<(), Box<dyn std::error::Error>> {
    let encoded = encode_event(event)?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(encoded.as_bytes())?;
    stdout.flush()?;
    Ok(())
}
