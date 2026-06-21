use std::process;

use anyhow::Result;
use tracing::{error, info};

use busytok_config::{init_logging, BusytokPaths, LogSource};
use busytok_runtime::ServiceApp;

#[tokio::main]
async fn main() {
    let paths = BusytokPaths::new();
    let _ = paths.ensure_dirs_exist();

    let service_session_id = uuid::Uuid::new_v4().to_string();
    let _guards = init_logging(&paths.log_dir(), LogSource::Service, &service_session_id);

    let _root = tracing::info_span!(
        "service_process",
        session_id = %service_session_id,
        source = "service",
        pid = std::process::id(),
    )
    .entered();

    if let Err(err) = run_main(paths).await {
        error!(
            event_code = "service.startup.fatal",
            stage = "service_app",
            error = %err,
            "service fatal error"
        );
        drop(_guards);
        drop(_root);
        process::exit(1);
    }
}

async fn run_main(paths: BusytokPaths) -> Result<()> {
    let startup = std::time::Instant::now();
    info!(
        event_code = "service.startup.begin",
        pid = std::process::id(),
        version = env!("CARGO_PKG_VERSION"),
        "Busytok service starting"
    );

    let app = ServiceApp::boot(paths, startup).await?;
    app.run().await
}
