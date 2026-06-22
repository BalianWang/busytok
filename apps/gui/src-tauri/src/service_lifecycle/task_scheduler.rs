//! Windows Task Scheduler lifecycle.

use super::command_runner::{CommandRunner, CommandStatus, SystemCommandRunner};
use super::{EnsureRunningOutcome, InstallOutcome, LifecycleStatus, ServiceLifecycle};
use anyhow::{Context, Result};
use busytok_config::BusytokPaths;
use busytok_platform::PlatformPaths;
use std::path::PathBuf;

const TASK_TEMPLATE: &str =
    include_str!("../../../../../packaging/windows/assets/task-template.xml");

pub struct TaskSchedulerLifecycle {
    paths: BusytokPaths,
    runner: Box<dyn CommandRunner>,
}

impl TaskSchedulerLifecycle {
    pub fn new() -> Self {
        Self {
            paths: BusytokPaths::new(),
            runner: Box::new(SystemCommandRunner),
        }
    }

    #[cfg(test)]
    pub fn with_runner(paths: BusytokPaths, runner: Box<dyn CommandRunner>) -> Self {
        Self { paths, runner }
    }

    fn definition_path(&self) -> PathBuf {
        PlatformPaths::new().service_definition_path()
    }

    fn render_xml(&self, binary: &str, workdir: &str, user: &str) -> String {
        TASK_TEMPLATE
            .replace("{BINARY}", &xml_escape(binary))
            .replace("{WORKDIR}", &xml_escape(workdir))
            .replace("{USER}", &xml_escape(user))
    }

    /// Probe whether the service is actually responding to RPC by issuing
    /// `service.health` and checking the `ready` field. Used to detect
    /// stale marker files left behind by a crashed service.
    ///
    /// Returns `false` if any step fails (endpoint resolution, tokio
    /// runtime not available, connect/handshake/call failure, or `ready=false`).
    fn probe_service_ready(&self) -> bool {
        let endpoint = match self.paths.control_endpoint() {
            Ok(ep) => ep,
            Err(_) => return false,
        };
        // Don't try to enter the runtime if we're not inside one. The
        // try_current() check handles being called from outside tokio.
        let rt = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => return false,
        };
        rt.block_on(async move {
            // Tokio timeout returns Result<T, Elapsed>; the inner connect
            // future returns Result<ControlClient, anyhow::Error>. We need
            // to handle both: timeout, then connect error.
            let connect_outcome = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                busytok_control::ControlClient::<busytok_control::transport::PlatformTransport>::connect(&endpoint),
            )
            .await;
            let mut client = match connect_outcome {
                Ok(Ok(c)) => c,
                _ => return false,
            };
            let req = busytok_protocol::dto::ControlRequest::with_meta(
                "service.health",
                serde_json::json!({}),
                busytok_protocol::dto::RequestMeta::default(),
            );
            let call_outcome = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                client.call(req),
            )
            .await;
            match call_outcome {
                Ok(Ok(busytok_protocol::dto::ControlResponse::Ok(v))) => {
                    v.get("ready").and_then(|r| r.as_bool()).unwrap_or(false)
                }
                _ => false,
            }
        })
    }
}

/// Write `xml` to `path` as UTF-16 LE with a BOM.
///
/// `packaging/windows/assets/task-template.xml` declares
/// `<?xml version="1.0" encoding="UTF-16"?>`, so `schtasks /Create /XML`
/// expects the file bytes to be UTF-16 with a BOM -- writing UTF-8 (the
/// Rust `std::fs::write` default) yields a parse error.
fn write_xml_utf16(path: &std::path::Path, xml: &str) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)
        .with_context(|| format!("creating task XML at {}", path.display()))?;
    // UTF-16 LE BOM
    file.write_all(&[0xFF, 0xFE])?;
    // Encode as UTF-16 LE
    let mut bytes = Vec::with_capacity(xml.len() * 2);
    for unit in xml.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    file.write_all(&bytes)?;
    Ok(())
}

/// Decode the UTF-16 LE bytes written by [`write_xml_utf16`] back into a
/// Rust `String` for comparison. Strips a leading BOM if present and is
/// tolerant of UTF-8 (used in unit tests via `render_xml`).
fn decode_utf16_le_with_bom(bytes: &[u8]) -> String {
    let mut body = bytes;
    // Strip a UTF-16 LE BOM if present.
    if body.starts_with(&[0xFF, 0xFE]) {
        body = &body[2..];
    }
    if body.len() % 2 != 0 {
        // Odd byte count means this can't be valid UTF-16; return empty
        // string so the install() comparison always fails → Upgraded path
        // rewrites the file rather than silently treating it as AlreadyPresent.
        return String::new();
    }
    let units: Vec<u16> = body
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

impl Default for TaskSchedulerLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

/// XML-escape a string for safe substitution into the task XML template.
/// Paths or SIDs containing `&`, `<`, `>`, `"`, or `'` would otherwise
/// produce malformed XML that `schtasks /Create` rejects.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

impl ServiceLifecycle for TaskSchedulerLifecycle {
    fn ensure_registered(&self) -> Result<InstallOutcome> {
        let binary = std::env::current_exe()
            .context("current_exe")?
            .parent()
            .context("exe parent")?
            .join("busytok-service.exe")
            .display()
            .to_string();
        let workdir = self.paths.data_dir().display().to_string();
        // Resolve the current user's SID rather than the bare USERNAME env
        // var. In domain-joined / Azure-AD environments schtasks /Create
        // requires DOMAIN\user or a SID; the bare username fails. The SID
        // works in all configurations.
        let user_sid = busytok_config::platform::current_user_sid_string()
            .context("failed to resolve current user SID for task principal")?;
        let xml = self.render_xml(&binary, &workdir, &user_sid);
        let path = self.definition_path();
        std::fs::create_dir_all(path.parent().context("parent")?)?;

        let outcome = match std::fs::read(&path) {
            Ok(existing_bytes) => {
                // The on-disk file is UTF-16 LE with a BOM; decode so we
                // can compare to the just-rendered (UTF-8 in memory) XML.
                let existing = decode_utf16_le_with_bom(&existing_bytes);
                if existing == xml {
                    InstallOutcome::AlreadyPresent
                } else {
                    InstallOutcome::Upgraded
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => InstallOutcome::NewlyInstalled,
            Err(e) => return Err(e).context("reading existing task XML"),
        };
        write_xml_utf16(&path, &xml)?;
        let create_status = self.runner.run(
            "schtasks",
            &[
                "/Create".into(),
                "/TN".into(),
                PlatformPaths::new().service_identifier().to_string(),
                "/XML".into(),
                path.display().to_string(),
                "/F".into(),
            ],
        )?;
        if !create_status.success {
            tracing::error!(
                event_code = "service_lifecycle.task_scheduler.create_failed",
                stdout = %create_status.stdout,
                stderr = %create_status.stderr,
                "schtasks /Create returned non-zero exit"
            );
            anyhow::bail!("schtasks /Create failed: {}", create_status.stderr);
        }
        tracing::info!(
            event_code = "service_lifecycle.task_scheduler.installed",
            ?outcome
        );
        Ok(outcome)
    }

    fn ensure_running(&self) -> Result<EnsureRunningOutcome> {
        // Marker is a fast-path hint, not authoritative. After a crash the marker
        // may be stale. Verify via RPC before declaring AlreadyRunning.
        if busytok_config::service_marker::exists(self.paths.data_dir()) {
            if self.probe_service_ready() {
                return Ok(EnsureRunningOutcome::AlreadyRunning);
            }
            tracing::warn!(
                event_code = "service_lifecycle.task_scheduler.stale_marker",
                "marker present but service not responding to RPC; removing stale marker and restarting"
            );
            let _ = busytok_config::service_marker::remove(self.paths.data_dir());
        }
        let install_outcome = self.ensure_registered()?;
        let run_status = self.runner.run(
            "schtasks",
            &[
                "/Run".into(),
                "/TN".into(),
                PlatformPaths::new().service_identifier().to_string(),
            ],
        )?;
        if !run_status.success {
            tracing::error!(
                event_code = "service_lifecycle.task_scheduler.run_failed",
                stdout = %run_status.stdout,
                stderr = %run_status.stderr,
                "schtasks /Run returned non-zero exit"
            );
            anyhow::bail!(
                "schtasks /Run failed (exit non-zero): stderr={}",
                run_status.stderr
            );
        }
        // Poll marker up to 30s
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if busytok_config::service_marker::exists(self.paths.data_dir()) {
                return Ok(EnsureRunningOutcome::Started { install_outcome });
            }
            if std::time::Instant::now() > deadline {
                anyhow::bail!("task scheduler service did not write marker within 30s");
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }

    fn status(&self) -> Result<LifecycleStatus> {
        let csv = self.runner.run(
            "schtasks",
            &[
                "/Query".into(),
                "/TN".into(),
                PlatformPaths::new().service_identifier().to_string(),
                "/FO".into(),
                "CSV".into(),
                "/V".into(),
            ],
        );
        match csv {
            Ok(status) => {
                if !status.success {
                    let stderr = &status.stderr;
                    if stderr.contains("cannot find") || stderr.contains("does not exist") {
                        return Ok(LifecycleStatus::NotRegistered);
                    }
                    tracing::warn!(
                        event_code = "service_lifecycle.task_scheduler.query_failed",
                        stderr = %stderr,
                        "schtasks /Query returned non-zero"
                    );
                    return Ok(LifecycleStatus::NeedsAttention);
                }
                let stdout = &status.stdout;
                if stdout.contains("\"Running\"") {
                    Ok(LifecycleStatus::Running)
                } else if stdout.contains("\"Disabled\"") {
                    Ok(LifecycleStatus::Disabled)
                } else if stdout.contains("\"Ready\"") {
                    Ok(LifecycleStatus::RegisteredInactive)
                } else {
                    tracing::warn!(
                        event_code = "service_lifecycle.task_scheduler.unknown_status",
                        stdout = %stdout
                    );
                    Ok(LifecycleStatus::NeedsAttention)
                }
            }
            Err(e) => {
                tracing::warn!(
                    event_code = "service_lifecycle.task_scheduler.query_invoke_failed",
                    error = %e
                );
                Ok(LifecycleStatus::NeedsAttention)
            }
        }
    }

    fn stop_for_current_session(&self) -> Result<()> {
        // Best-effort stop of the running task instance. We deliberately
        // keep the registration (schtasks definition + on-disk XML) intact
        // so the next GUI launch can start the service again via
        // `ensure_running`. A failure here is logged but not propagated
        // because the caller (whole-product quit) is already tearing down.
        let stop_status = self.runner.run(
            "schtasks",
            &[
                "/End".into(),
                "/TN".into(),
                PlatformPaths::new().service_identifier().to_string(),
            ],
        );
        match stop_status {
            Ok(s) if s.success => {
                tracing::info!(event_code = "service_lifecycle.task_scheduler.stopped_for_session");
            }
            Ok(s) => {
                tracing::warn!(
                    event_code = "service_lifecycle.task_scheduler.stop_failed",
                    stdout = %s.stdout,
                    stderr = %s.stderr,
                    "schtasks /End returned non-zero"
                );
            }
            Err(e) => {
                tracing::warn!(
                    event_code = "service_lifecycle.task_scheduler.stop_invoke_failed",
                    error = %e
                );
            }
        }
        Ok(())
    }

    fn uninstall(&self) -> Result<()> {
        let _ = self.runner.run(
            "schtasks",
            &[
                "/Delete".into(),
                "/TN".into(),
                PlatformPaths::new().service_identifier().to_string(),
                "/F".into(),
            ],
        );
        let _ = std::fs::remove_file(self.definition_path());
        tracing::info!(event_code = "service_lifecycle.task_scheduler.uninstalled");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::command_runner::{CommandStatus, FakeCommandRunner};
    use super::*;
    use tempfile::tempdir;

    fn ok_csv(status: &str) -> CommandStatus {
        CommandStatus {
            success: true,
            exit_code: None,
            stdout: format!("\"TaskName\",\"Status\"\n\"\\\\Busytok\\\\Service\",\"{status}\""),
            stderr: String::new(),
        }
    }

    fn ok_status() -> CommandStatus {
        CommandStatus {
            success: true,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    #[test]
    fn status_running() {
        let tmp = tempdir().unwrap();
        let paths = BusytokPaths::for_test(tmp.path());
        let runner = FakeCommandRunner::new();
        runner.enqueue("schtasks", "/Query", ok_csv("Running"));
        let lc = TaskSchedulerLifecycle::with_runner(paths, Box::new(runner));
        assert_eq!(lc.status().unwrap(), LifecycleStatus::Running);
    }

    #[test]
    fn status_ready_maps_to_registered_inactive() {
        let tmp = tempdir().unwrap();
        let paths = BusytokPaths::for_test(tmp.path());
        let runner = FakeCommandRunner::new();
        runner.enqueue("schtasks", "/Query", ok_csv("Ready"));
        let lc = TaskSchedulerLifecycle::with_runner(paths, Box::new(runner));
        assert_eq!(lc.status().unwrap(), LifecycleStatus::RegisteredInactive);
    }

    #[test]
    fn render_xml_substitutes_all_placeholders() {
        let lc = TaskSchedulerLifecycle::new();
        // A SID-shaped principal string, matching what
        // busytok_config::platform::current_user_sid_string() returns.
        let sid = "S-1-5-21-1001-2002-3003-500";
        let xml = lc.render_xml("C:\\Busytok\\busytok-service.exe", "C:\\Busytok", sid);
        assert!(!xml.contains("{BINARY}"));
        assert!(!xml.contains("{WORKDIR}"));
        assert!(!xml.contains("{USER}"));
        assert!(xml.contains("C:\\Busytok\\busytok-service.exe"));
        assert!(xml.contains(sid));
    }

    #[test]
    fn render_xml_escapes_special_characters() {
        // A path containing XML-special characters must not produce
        // malformed XML. Regression test for paths like
        // `C:\Apps\Tom & Jerry\busytok-service.exe`.
        let lc = TaskSchedulerLifecycle::new();
        let xml = lc.render_xml(
            "C:\\Apps\\Tom & Jerry\\busytok-service.exe",
            "C:\\Apps\\Tom & Jerry",
            "S-1-5-21-1001-2002-3003-500",
        );
        // The raw ampersand must never appear unescaped in the output.
        assert!(
            !xml.contains(" & "),
            "raw ampersand in path leaked into XML unescaped: {xml}"
        );
        // The escaped form must be present.
        assert!(xml.contains("&amp;"));
    }

    #[test]
    fn write_xml_utf16_round_trips_and_has_bom() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("task.xml");
        let body = "<?xml version=\"1.0\" encoding=\"UTF-16\"?><Task>v</Task>";
        write_xml_utf16(&path, body).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        // UTF-16 LE BOM
        assert_eq!(&bytes[0..2], &[0xFF, 0xFE]);
        assert_eq!(decode_utf16_le_with_bom(&bytes), body);
    }

    #[test]
    fn status_not_registered() {
        let tmp = tempdir().unwrap();
        let paths = BusytokPaths::for_test(tmp.path());
        let runner = FakeCommandRunner::new();
        runner.enqueue(
            "schtasks",
            "/Query",
            CommandStatus {
                success: false,
                exit_code: None,
                stdout: String::new(),
                stderr: "ERROR: The system cannot find the file specified.".into(),
            },
        );
        let lc = TaskSchedulerLifecycle::with_runner(paths, Box::new(runner));
        assert_eq!(lc.status().unwrap(), LifecycleStatus::NotRegistered);
    }
}
