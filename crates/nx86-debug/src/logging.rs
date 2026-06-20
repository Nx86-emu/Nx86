use tracing_subscriber::{EnvFilter, fmt};

pub fn init_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("nx86=info,nx86_app=info,nx86_gui=info,warn"));

    let _ = fmt()
        .with_env_filter(filter)
        .with_target(true)
        .compact()
        .try_init();
}
