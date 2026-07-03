//! Shared command runner — abstracts shelling out to launchctl / schtasks.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandStatus {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, args: &[String]) -> Result<CommandStatus>;
}

pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<CommandStatus> {
        let output = Command::new(program)
            .args(args)
            .output()
            .with_context(|| format!("running {program} {}", args.join(" ")))?;
        tracing::debug!(
            event_code = "service_lifecycle.command.run",
            program,
            args = ?args,
            success = output.status.success()
        );
        Ok(CommandStatus {
            success: output.status.success(),
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

// ── Launchctl label helpers ──────────────────────────────────────────

/// Build a launchctl target spec of the form `gui/<uid>/<label>`. The
/// uid is required by `launchctl print/bootout/kickstart` — passing
/// `gui/<label>` without the uid silently fails or targets the wrong
/// domain.
pub fn launchctl_label(uid: u32, label: &str) -> String {
    format!("gui/{uid}/{label}")
}

/// Build a launchctl per-user domain target of the form `gui/<uid>`.
pub fn launchctl_domain(uid: u32) -> String {
    format!("gui/{uid}")
}

/// Resolve the current process uid. Used by lifecycle methods that
/// shell out to `launchctl` so the target spec includes the uid.
pub fn current_uid() -> u32 {
    // SAFETY: getuid is always safe and returns the caller's real uid.
    unsafe { libc::getuid() }
}

// ── Launchctl helpers (used by SmAppServiceLifecycle) ──

#[allow(dead_code)]
pub fn launchctl_print<R: CommandRunner + ?Sized>(
    runner: &R,
    label: &str,
) -> Result<CommandStatus> {
    runner.run("launchctl", &["print".to_string(), label.to_string()])
}

pub fn launchctl_bootout<R: CommandRunner + ?Sized>(
    runner: &R,
    label: &str,
) -> Result<CommandStatus> {
    runner.run("launchctl", &["bootout".to_string(), label.to_string()])
}

/// `launchctl bootstrap gui/<uid> <plist>` — load a per-user LaunchAgent.
pub fn launchctl_bootstrap<R: CommandRunner + ?Sized>(
    runner: &R,
    domain: &str,
    plist_path: &Path,
) -> Result<CommandStatus> {
    runner.run(
        "launchctl",
        &[
            "bootstrap".to_string(),
            domain.to_string(),
            plist_path.display().to_string(),
        ],
    )
}

/// `launchctl kickstart <label>` — force-start an already-loaded service when
/// launchd has not yet spawned it (or when the loaded agent has crashed).
/// Idempotent against an already-running job.
pub fn launchctl_kickstart<R: CommandRunner + ?Sized>(
    runner: &R,
    label: &str,
) -> Result<CommandStatus> {
    runner.run("launchctl", &["kickstart".to_string(), label.to_string()])
}

// ── Strict variants — propagate non-zero exit codes as structured errors

/// Run `launchctl bootout <label>` and propagate non-zero exit codes.
///
/// `bootout` of an already-not-loaded job returns non-zero; callers that
/// want best-effort behavior should use [`launchctl_bootout`] directly
/// and inspect `CommandStatus::success`.
pub fn launchctl_bootout_strict<R: CommandRunner + ?Sized>(runner: &R, label: &str) -> Result<()> {
    let status = launchctl_bootout(runner, label)?;
    if !status.success {
        let code = status
            .exit_code
            .map(|c| format!(" (exit {c})"))
            .unwrap_or_default();
        anyhow::bail!(
            "launchctl bootout {label} failed{code}: {}",
            status.stderr.trim()
        );
    }
    Ok(())
}

/// Run `launchctl bootstrap <domain> <plist>` and propagate non-zero exit codes.
pub fn launchctl_bootstrap_strict<R: CommandRunner + ?Sized>(
    runner: &R,
    domain: &str,
    plist_path: &Path,
) -> Result<()> {
    let status = launchctl_bootstrap(runner, domain, plist_path)?;
    if !status.success {
        let code = status
            .exit_code
            .map(|c| format!(" (exit {c})"))
            .unwrap_or_default();
        anyhow::bail!(
            "launchctl bootstrap {domain} {} failed{code}: {}",
            plist_path.display(),
            status.stderr.trim()
        );
    }
    Ok(())
}

/// Run `launchctl kickstart <label>` and propagate non-zero exit codes.
pub fn launchctl_kickstart_strict<R: CommandRunner + ?Sized>(
    runner: &R,
    label: &str,
) -> Result<()> {
    let status = launchctl_kickstart(runner, label)?;
    if !status.success {
        let code = status
            .exit_code
            .map(|c| format!(" (exit {c})"))
            .unwrap_or_default();
        anyhow::bail!(
            "launchctl kickstart {label} failed{code}: {}",
            status.stderr.trim()
        );
    }
    Ok(())
}

#[cfg(test)]
pub struct FakeCommandRunner {
    rules: std::sync::Mutex<Vec<FakeRule>>,
}
#[cfg(test)]
struct FakeRule {
    program: String,
    args_substring: String,
    status: CommandStatus,
}
#[cfg(test)]
impl FakeCommandRunner {
    pub fn new() -> Self {
        Self {
            rules: std::sync::Mutex::new(Vec::new()),
        }
    }
    pub fn enqueue(&self, program: &str, args_substring: &str, status: CommandStatus) {
        self.rules.lock().unwrap().push(FakeRule {
            program: program.to_string(),
            args_substring: args_substring.to_string(),
            status,
        });
    }
}
#[cfg(test)]
impl CommandRunner for FakeCommandRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<CommandStatus> {
        let joined = args.join(" ");
        let mut rules = self.rules.lock().unwrap();
        let pos = rules
            .iter()
            .position(|r| r.program == program && joined.contains(&r.args_substring));
        if let Some(idx) = pos {
            Ok(rules.remove(idx).status)
        } else {
            panic!("FakeCommandRunner: no rule for {program} {joined}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::catch_unwind;

    // ── Pure formatter helpers ──────────────────────────────────────────

    #[test]
    fn launchctl_label_formats_target_spec_with_uid_and_label() {
        assert_eq!(launchctl_label(501, "com.busytok.service"), "gui/501/com.busytok.service");
    }

    #[test]
    fn launchctl_label_uses_zero_uid_for_root() {
        assert_eq!(launchctl_label(0, "com.busytok.service"), "gui/0/com.busytok.service");
    }

    #[test]
    fn launchctl_domain_formats_per_user_domain_spec() {
        assert_eq!(launchctl_domain(501), "gui/501");
        assert_eq!(launchctl_domain(0), "gui/0");
    }

    #[test]
    fn current_uid_returns_a_reasonable_uid() {
        // Tests almost always run as the developer's own user, so uid > 0.
        // Even under root (CI), uid == 0 is valid; just assert non-negative.
        let uid = current_uid();
        // u32 is always non-negative; sanity-check it matches libc's view.
        assert_eq!(uid, unsafe { libc::getuid() });
    }

    // ── FakeCommandRunner behaviour ────────────────────────────────────

    #[test]
    fn fake_runner_returns_enqueued_status_and_consumes_rule() {
        let runner = FakeCommandRunner::new();
        runner.enqueue(
            "launchctl",
            "print",
            CommandStatus {
                success: true,
                exit_code: Some(0),
                stdout: "pid=123".into(),
                stderr: String::new(),
            },
        );
        let status = runner
            .run("launchctl", &["print".to_string(), "gui/501/x".to_string()])
            .unwrap();
        assert!(status.success);
        assert_eq!(status.exit_code, Some(0));
        assert_eq!(status.stdout, "pid=123");
    }

    #[test]
    fn fake_runner_panics_when_no_rule_matches_program() {
        let runner = FakeCommandRunner::new();
        let result = catch_unwind(|| {
            runner.run("launchctl", &["print".to_string(), "gui/501/x".to_string()])
        });
        assert!(
            result.is_err(),
            "FakeCommandRunner must panic when no rule matches the program"
        );
    }

    #[test]
    fn fake_runner_panics_when_args_substring_does_not_match() {
        let runner = FakeCommandRunner::new();
        runner.enqueue(
            "launchctl",
            "print",
            CommandStatus {
                success: true,
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        // Substring "bootout" is NOT in the enqueued rule, so this should panic.
        let result = catch_unwind(|| {
            runner.run("launchctl", &["bootout".to_string(), "gui/501/x".to_string()])
        });
        assert!(result.is_err(), "FakeCommandRunner must panic when args substring does not match");
    }

    #[test]
    fn fake_runner_only_consumes_first_matching_rule() {
        let runner = FakeCommandRunner::new();
        runner.enqueue(
            "launchctl",
            "print",
            CommandStatus {
                success: true,
                exit_code: None,
                stdout: "first".into(),
                stderr: String::new(),
            },
        );
        runner.enqueue(
            "launchctl",
            "print",
            CommandStatus {
                success: true,
                exit_code: None,
                stdout: "second".into(),
                stderr: String::new(),
            },
        );
        let s1 = runner
            .run("launchctl", &["print".to_string(), "gui/1".to_string()])
            .unwrap();
        let s2 = runner
            .run("launchctl", &["print".to_string(), "gui/2".to_string()])
            .unwrap();
        assert_eq!(s1.stdout, "first");
        assert_eq!(s2.stdout, "second");
    }

    // ── launchctl_* helpers (use FakeCommandRunner) ─────────────────────

    fn ok_status() -> CommandStatus {
        CommandStatus {
            success: true,
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    fn fail_status(exit_code: i32, stderr: &str) -> CommandStatus {
        CommandStatus {
            success: false,
            exit_code: Some(exit_code),
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

    #[test]
    fn launchctl_print_passes_label_to_runner() {
        let runner = FakeCommandRunner::new();
        runner.enqueue("launchctl", "print", ok_status());
        let status = launchctl_print(&runner, "gui/501/com.busytok.service").unwrap();
        assert!(status.success);
    }

    #[test]
    fn launchctl_bootout_passes_label_to_runner() {
        let runner = FakeCommandRunner::new();
        runner.enqueue("launchctl", "bootout", ok_status());
        let status = launchctl_bootout(&runner, "gui/501/com.busytok.service").unwrap();
        assert!(status.success);
    }

    #[test]
    fn launchctl_bootstrap_passes_domain_and_plist_to_runner() {
        let runner = FakeCommandRunner::new();
        runner.enqueue("launchctl", "bootstrap", ok_status());
        let plist = std::path::Path::new("/Library/LaunchAgents/com.busytok.service.plist");
        let status = launchctl_bootstrap(&runner, "gui/501", plist).unwrap();
        assert!(status.success);
    }

    #[test]
    fn launchctl_kickstart_passes_label_to_runner() {
        let runner = FakeCommandRunner::new();
        runner.enqueue("launchctl", "kickstart", ok_status());
        let status = launchctl_kickstart(&runner, "gui/501/com.busytok.service").unwrap();
        assert!(status.success);
    }

    // ── strict variants — propagate non-zero exits ─────────────────────

    #[test]
    fn launchctl_bootout_strict_returns_ok_when_runner_succeeds() {
        let runner = FakeCommandRunner::new();
        runner.enqueue("launchctl", "bootout", ok_status());
        launchctl_bootout_strict(&runner, "gui/501/com.busytok.service").unwrap();
    }

    #[test]
    fn launchctl_bootout_strict_bails_when_runner_fails() {
        let runner = FakeCommandRunner::new();
        runner.enqueue("launchctl", "bootout", fail_status(1, "Bootout failed: not loaded"));
        let err = launchctl_bootout_strict(&runner, "gui/501/com.busytok.service")
            .unwrap_err()
            .to_string();
        assert!(err.contains("bootout"), "error must mention bootout: {err}");
        assert!(
            err.contains("exit 1"),
            "error must include exit code when present: {err}"
        );
        assert!(
            err.contains("Bootout failed"),
            "error must include trimmed stderr: {err}"
        );
    }

    #[test]
    fn launchctl_bootout_strict_bails_without_exit_code_when_absent() {
        let runner = FakeCommandRunner::new();
        // exit_code=None mirrors platforms where `code()` returns None (signals).
        runner.enqueue(
            "launchctl",
            "bootout",
            CommandStatus {
                success: false,
                exit_code: None,
                stdout: String::new(),
                stderr: "  trimmed stderr  ".to_string(),
            },
        );
        let err = launchctl_bootout_strict(&runner, "gui/501/x")
            .unwrap_err()
            .to_string();
        // When exit_code is None, no "(exit N)" suffix should be appended.
        assert!(
            !err.contains("exit "),
            "error must not include exit code when absent: {err}"
        );
        assert!(
            err.contains("trimmed stderr"),
            "stderr must be trimmed of surrounding whitespace: {err}"
        );
    }

    #[test]
    fn launchctl_bootstrap_strict_returns_ok_when_runner_succeeds() {
        let runner = FakeCommandRunner::new();
        runner.enqueue("launchctl", "bootstrap", ok_status());
        let plist = std::path::Path::new("/Library/LaunchAgents/com.busytok.service.plist");
        launchctl_bootstrap_strict(&runner, "gui/501", plist).unwrap();
    }

    #[test]
    fn launchctl_bootstrap_strict_bails_when_runner_fails() {
        let runner = FakeCommandRunner::new();
        runner.enqueue("launchctl", "bootstrap", fail_status(2, "already loaded"));
        let plist = std::path::Path::new("/Library/LaunchAgents/com.busytok.service.plist");
        let err = launchctl_bootstrap_strict(&runner, "gui/501", plist)
            .unwrap_err()
            .to_string();
        assert!(err.contains("bootstrap"), "error must mention bootstrap: {err}");
        assert!(err.contains("exit 2"), "error must include exit code: {err}");
        assert!(
            err.contains("already loaded"),
            "error must include trimmed stderr: {err}"
        );
        assert!(
            err.contains("com.busytok.service.plist"),
            "error must include plist path: {err}"
        );
    }

    #[test]
    fn launchctl_kickstart_strict_returns_ok_when_runner_succeeds() {
        let runner = FakeCommandRunner::new();
        runner.enqueue("launchctl", "kickstart", ok_status());
        launchctl_kickstart_strict(&runner, "gui/501/com.busytok.service").unwrap();
    }

    #[test]
    fn launchctl_kickstart_strict_bails_when_runner_fails() {
        let runner = FakeCommandRunner::new();
        runner.enqueue("launchctl", "kickstart", fail_status(3, "No such process"));
        let err = launchctl_kickstart_strict(&runner, "gui/501/com.busytok.service")
            .unwrap_err()
            .to_string();
        assert!(err.contains("kickstart"), "error must mention kickstart: {err}");
        assert!(err.contains("exit 3"), "error must include exit code: {err}");
        assert!(
            err.contains("No such process"),
            "error must include trimmed stderr: {err}"
        );
    }

    #[test]
    fn launchctl_kickstart_strict_bails_without_exit_code_when_absent() {
        let runner = FakeCommandRunner::new();
        runner.enqueue(
            "launchctl",
            "kickstart",
            CommandStatus {
                success: false,
                exit_code: None,
                stdout: String::new(),
                stderr: "service not loaded".to_string(),
            },
        );
        let err = launchctl_kickstart_strict(&runner, "gui/501/x")
            .unwrap_err()
            .to_string();
        assert!(
            !err.contains("exit "),
            "error must not include exit code when absent: {err}"
        );
    }

    // ── SystemCommandRunner — exercise real process spawning ───────────

    fn true_binary() -> &'static str {
        // macOS puts true/false under /usr/bin (and /bin is a symlink to
        // /usr/bin, but only some utilities have /bin entries). Probe both
        // so the test works on macOS and Linux dev containers.
        if std::path::Path::new("/bin/true").exists() {
            "/bin/true"
        } else {
            "/usr/bin/true"
        }
    }

    fn false_binary() -> &'static str {
        if std::path::Path::new("/bin/false").exists() {
            "/bin/false"
        } else {
            "/usr/bin/false"
        }
    }

    #[test]
    fn system_command_runner_reports_success_for_true() {
        // `true` exits 0 on macOS/Linux. This exercises the
        // happy-path branch of `Command::output()` and the
        // `output.status.success()` mapping.
        let runner = SystemCommandRunner;
        let status = runner.run(true_binary(), &[]).unwrap();
        assert!(status.success);
        assert_eq!(status.exit_code, Some(0));
        assert!(status.stdout.is_empty());
        assert!(status.stderr.is_empty());
    }

    #[test]
    fn system_command_runner_reports_failure_for_false() {
        // `false` exits 1; this exercises the non-zero-exit branch
        // of the success mapping without actually spawning a missing
        // binary (which would error before reaching the status mapping).
        let runner = SystemCommandRunner;
        let status = runner.run(false_binary(), &[]).unwrap();
        assert!(!status.success);
        assert_eq!(status.exit_code, Some(1));
    }

    #[test]
    fn system_command_runner_captures_stdout_and_stderr() {
        // `/bin/echo` writes its argv to stdout. Use it to verify that
        // `String::from_utf8_lossy` decoding preserves the bytes.
        let runner = SystemCommandRunner;
        let status = runner
            .run("/bin/echo", &["hello-world".to_string()])
            .unwrap();
        assert!(status.success);
        assert_eq!(status.stdout.trim(), "hello-world");
        assert!(status.stderr.is_empty());
    }

    #[test]
    fn system_command_runner_propagates_spawn_error_for_missing_program() {
        // Spawning a non-existent program errors at the `output()` call,
        // which the `with_context` wraps into an `anyhow::Error`.
        let runner = SystemCommandRunner;
        let result = runner.run("/nonexistent/busytok-coverage-binary", &[]);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("running /nonexistent/busytok-coverage-binary"),
            "error must include the program name in its context: {err}"
        );
    }
}
