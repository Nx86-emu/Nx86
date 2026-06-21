use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::prelude::*;

/// Default per-target filter, mirroring the previous `EnvFilter` default
/// (`nx86=info,nx86_app=info,nx86_gui=info,warn`).
fn default_targets() -> Targets {
    Targets::new()
        .with_target("nx86", LevelFilter::INFO)
        .with_target("nx86_app", LevelFilter::INFO)
        .with_target("nx86_gui", LevelFilter::INFO)
        .with_default(LevelFilter::WARN)
}

pub fn init_logging() {
    // `Targets` parses the same comma-separated `target=level` directives as the
    // common `RUST_LOG` usage without pulling in the regex-backed `env-filter`
    // feature. Span/field directives are intentionally unsupported.
    let filter = std::env::var("RUST_LOG")
        .ok()
        .and_then(|raw| raw.parse::<Targets>().ok())
        .unwrap_or_else(default_targets);

    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_target(true).compact())
        .with(filter)
        .try_init();
}
