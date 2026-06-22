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
