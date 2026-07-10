use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;

/// Remove log files older than `keep_days` from `log_dir`.
///
/// Only removes files matching the tracing-appender daily rotation
/// pattern `{name}.YYYY-MM-DD`. The current (non-rotated) base file
/// is never removed.
pub fn prune_old_logs(log_dir: &Path, keep_days: u64) {
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(keep_days * 86400))
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let entries = match fs::read_dir(log_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Only clean rotated files: base.YYYY-MM-DD
        let Some(dot_pos) = name.rfind('.') else {
            continue;
        };
        let date_part = &name[dot_pos + 1..];
        if date_part.len() != 10 || date_part.chars().filter(|c| *c == '-').count() != 2 {
            continue;
        }

        if let Ok(meta) = entry.metadata() {
            if let Ok(mtime) = meta.modified() {
                if mtime < cutoff {
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }
}

/// Identifies which binary is initializing logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSource {
    Service,
    Gui,
    Cli,
}

/// Return the default `RUST_LOG` filter level for a given source.
///
/// CLI defaults to `warn` (quiet — only show warnings/errors, like
/// `gh`/`docker`/`kubectl`). Service and GUI default to `info`.
/// `RUST_LOG` env var always takes precedence over this default.
fn default_log_level(source: LogSource) -> &'static str {
    match source {
        LogSource::Cli => "warn",
        _ => "info",
    }
}

/// Holds guards that must outlive the process to keep non-blocking
/// writers alive. Drop to flush on shutdown.
pub struct LoggingGuards {
    pub file_guard: Option<tracing_appender::non_blocking::WorkerGuard>,
    pub bootstrap_guard: Option<tracing_appender::non_blocking::WorkerGuard>,
}

/// Initialize the tracing subscriber with terminal + file layers.
///
/// - Terminal: stderr, human-readable (always)
/// - File: JSON, rolling daily, non-blocking (always)
/// - `target=bootstrap` events are routed to `bootstrap.log`;
///   everything else goes to the main log file.
/// - The `log` → `tracing` bridge is enabled.
///
/// Callers must guarantee single-call per process. Implementation
/// uses `try_init` so test re-entry is a graceful no-op.
///
/// Returns `LoggingGuards` — drop to flush non-blocking writers.
pub fn init_logging(log_dir: &Path, source: LogSource, session_id: &str) -> Option<LoggingGuards> {
    prune_old_logs(log_dir, 7);

    // Registry-level filter: always defaults to `info` so file logging
    // captures full diagnostics. The terminal layer gets its own per-layer
    // filter below to quiet CLI stderr.
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // CLI: terminal-only by default; enables file layer when
    // BUSYTOK_LOG_DIR is set (useful for installed-environment debugging).
    let cli_log_dir: Option<std::path::PathBuf> = if source == LogSource::Cli {
        let dir = std::env::var("BUSYTOK_LOG_DIR")
            .ok()
            .map(std::path::PathBuf::from);
        if dir.is_none() {
            return init_cli_logging(env_filter, session_id);
        }
        dir
    } else {
        None
    };

    // Terminal layer (shared by Service, GUI, and CLI+BUSYTOK_LOG_DIR).
    // CLI gets a per-layer filter defaulting to `warn` so the terminal is
    // quiet; file logging still gets `info` from the registry-level filter.
    // `RUST_LOG` overrides both (via `try_from_default_env`).
    // Service/GUI: no per-layer filter — terminal follows registry default.
    let term_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(default_log_level(source))),
        );

    let main_name = match source {
        LogSource::Service => "service.log",
        LogSource::Gui => "gui.log",
        LogSource::Cli => "cli.log",
    };

    let appender_dir = cli_log_dir.as_deref().unwrap_or(log_dir);

    let main_appender = tracing_appender::rolling::daily(appender_dir, main_name);
    let (main_writer, main_guard) = tracing_appender::non_blocking(main_appender);

    let mut opt_bootstrap_guard: Option<tracing_appender::non_blocking::WorkerGuard> = None;

    if source == LogSource::Gui {
        // Tauri: split gui.log (non-bootstrap) and bootstrap.log
        let gui_filter = tracing_subscriber::filter::filter_fn(|meta| {
            meta.target() != "bootstrap" && !meta.target().starts_with("bootstrap::")
        });
        let gui_json_layer = tracing_subscriber::fmt::layer()
            .json()
            .with_writer(main_writer)
            .with_filter(gui_filter);

        let bootstrap_appender = tracing_appender::rolling::daily(log_dir, "bootstrap.log");
        let (bootstrap_writer, bg) = tracing_appender::non_blocking(bootstrap_appender);
        opt_bootstrap_guard = Some(bg);
        let bootstrap_filter = tracing_subscriber::filter::filter_fn(|meta| {
            meta.target() == "bootstrap" || meta.target().starts_with("bootstrap::")
        });
        let bootstrap_json_layer = tracing_subscriber::fmt::layer()
            .json()
            .with_writer(bootstrap_writer)
            .with_filter(bootstrap_filter);

        match tracing_subscriber::registry()
            .with(env_filter)
            .with(term_layer)
            .with(gui_json_layer)
            .with(bootstrap_json_layer)
            .try_init()
        {
            Ok(()) => {}
            Err(_) => return None,
        }
    } else {
        // Service: single file
        let json_layer = tracing_subscriber::fmt::layer()
            .json()
            .with_writer(main_writer);
        match tracing_subscriber::registry()
            .with(env_filter)
            .with(term_layer)
            .with(json_layer)
            .try_init()
        {
            Ok(()) => {}
            Err(_) => return None,
        }
    }

    let _ = tracing_log::LogTracer::init();

    Some(LoggingGuards {
        file_guard: Some(main_guard),
        bootstrap_guard: opt_bootstrap_guard,
    })
}

fn init_cli_logging(env_filter: EnvFilter, _session_id: &str) -> Option<LoggingGuards> {
    // Terminal layer: quiet by default (warn), RUST_LOG overrides.
    // env_filter (registry) stays at "info" so if BUSYTOK_LOG_DIR is later
    // enabled, file logging captures full diagnostics. But this path has no
    // file layer, so the per-layer filter is the effective terminal level.
    let term_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")));

    match tracing_subscriber::registry()
        .with(env_filter)
        .with(term_layer)
        .try_init()
    {
        Ok(()) => {
            let _ = tracing_log::LogTracer::init();
            Some(LoggingGuards {
                file_guard: None,
                bootstrap_guard: None,
            })
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    #[test]
    fn prune_does_not_remove_current_log() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("service.log");
        fs::write(&current, "test").unwrap();
        prune_old_logs(dir.path(), 7);
        assert!(current.exists());
    }

    #[test]
    #[ignore = "file mtime/deletion behavior differs on Windows CI"]
    fn prune_removes_rotated_file_with_old_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("service.log.2020-01-01");
        fs::write(&old, "test").unwrap();
        // Set mtime to 30 days ago so prune removes it
        let thirty_days = SystemTime::now() - Duration::from_secs(30 * 86400);
        let file = std::fs::File::open(&old).unwrap();
        file.set_modified(thirty_days).ok();
        prune_old_logs(dir.path(), 7);
        assert!(!old.exists());
    }

    #[test]
    fn prune_keeps_rotated_file_with_recent_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let recent = dir.path().join("service.log.2026-05-22");
        fs::write(&recent, "test").unwrap();
        // mtime defaults to now, so it should be kept
        prune_old_logs(dir.path(), 7);
        assert!(recent.exists());
    }

    #[test]
    fn prune_ignores_non_rotated_files() {
        let dir = tempfile::tempdir().unwrap();
        let not_rotated = dir.path().join("not-a-log-file.txt");
        fs::write(&not_rotated, "test").unwrap();
        prune_old_logs(dir.path(), 7);
        assert!(not_rotated.exists());
    }

    #[test]
    fn prune_handles_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        prune_old_logs(dir.path(), 7);
    }

    #[test]
    fn prune_handles_nonexistent_dir() {
        let dir = std::path::Path::new("/nonexistent/prune/test/dir");
        prune_old_logs(dir, 7);
    }

    #[test]
    fn default_log_level_cli_is_warn() {
        assert_eq!(default_log_level(LogSource::Cli), "warn");
    }

    #[test]
    fn default_log_level_service_is_info() {
        assert_eq!(default_log_level(LogSource::Service), "info");
    }

    #[test]
    fn default_log_level_gui_is_info() {
        assert_eq!(default_log_level(LogSource::Gui), "info");
    }
}
