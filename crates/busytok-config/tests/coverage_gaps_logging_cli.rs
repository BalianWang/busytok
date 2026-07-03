//! Coverage gap test for `logging.rs` — Cli-first `init_logging` path.
//!
//! `tracing::subscriber::set_global_default` can succeed at most once per
//! process. The existing test in `coverage_gaps.rs` calls `init_logging` with
//! `LogSource::Service` first, and the test in `coverage_gaps_config.rs` calls
//! `LogSource::Gui` first. Neither covers the `init_cli_logging` Ok arm
//! (logging.rs lines 179-183) because the subscriber is already set by the
//! time Cli is called.
//!
//! This file is a SEPARATE test binary — it has its own process, so
//! `try_init` succeeds when Cli is called first. This covers the Cli Ok arm:
//! ```text
//! Ok(()) => {
//!     let _ = tracing_log::LogTracer::init();   // line 179
//!     Some(LoggingGuards {                       // line 180
//!         file_guard: None,                       // line 181
//!         bootstrap_guard: None,                  // line 182
//!     })                                          // line 183
//! }
//! ```

#![allow(clippy::unwrap_used, clippy::uninlined_format_args)]

use std::fs;

use busytok_config::{init_logging, LogSource};
use tempfile::TempDir;

#[test]
fn init_logging_cli_first_succeeds() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    fs::create_dir_all(&log_dir).unwrap();

    // Ensure BUSYTOK_LOG_DIR is not set — otherwise init_logging routes
    // through the file-layer path instead of init_cli_logging.
    std::env::remove_var("BUSYTOK_LOG_DIR");

    // Cli init_logging — first call in this binary, so init_cli_logging's
    // try_init succeeds. Covers logging.rs lines 179-183 (Cli Ok arm).
    let guards = init_logging(&log_dir, LogSource::Cli, "cli-first");
    assert!(
        guards.is_some(),
        "Cli init_logging must succeed when called first in a fresh process"
    );
    let guards = guards.unwrap();
    assert!(
        guards.file_guard.is_none(),
        "Cli (no BUSYTOK_LOG_DIR) has no file guard"
    );
    assert!(
        guards.bootstrap_guard.is_none(),
        "Cli has no bootstrap guard"
    );
    drop(guards);
}
