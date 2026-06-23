//! Pure-text parser for `launchctl print gui/<uid>/<label>` output.
//!
//! `SMAppService.status` does NOT expose the registered executable path, so
//! detecting a stale registration after an app move requires parsing the
//! launchd state. This module pulls just the program path out of the
//! `launchctl print` dump. It deliberately has no Objective-C / FFI surface
//! so it can be unit-tested on any platform.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// Well-known fixture used by the unit tests for [`LaunchdJobSnapshot::parse`].
#[cfg(test)]
pub(crate) const LAUNCHCTL_PRINT_FIXTURE: &str =
    include_str!("../../tests/fixtures/launchctl_print_com.busytok.service.txt",);

/// Parsed snapshot of `launchctl print gui/<uid>/<label>` output.
///
/// `SMAppService.status` does NOT expose the registered executable path or
/// live PID, so detecting a stale registration or a stale live process after
/// an app move requires parsing the launchd state. This struct pulls the
/// program path, pid, and state out of the `launchctl print` dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchdJobSnapshot {
    program_path: Option<PathBuf>,
    pid: Option<u32>,
    state: Option<String>,
}

impl LaunchdJobSnapshot {
    /// Parse the textual output of `launchctl print gui/<uid>/<label>`.
    ///
    /// Looks for the canonical `program = <path>;` line (and the legacy
    /// `program arguments` block) and records the first absolute path it
    /// finds. Returns `Ok` even if no program path is present — the snapshot
    /// is then just an "unknown" marker and [`Self::program_path`] will
    /// yield `None`.
    ///
    /// Both single-line forms (`program arguments = ("/path", ...)`) and the
    /// multiline block form (`program arguments = (\n\t\t"/path",\n\t)`) are
    /// supported.
    pub fn parse(launchctl_print_output: &str) -> Result<Self> {
        let mut program_path: Option<PathBuf> = None;
        let mut pid: Option<u32> = None;
        let mut state: Option<String> = None;
        let mut in_program_arguments_block = false;

        for raw in launchctl_print_output.lines() {
            let line = raw.trim();

            // `pid = <u32>`
            if pid.is_none() {
                if let Some(rest) = line.strip_prefix("pid =") {
                    if let Ok(val) = rest.trim().trim_end_matches(';').trim().parse::<u32>() {
                        pid = Some(val);
                    }
                }
            }

            // `state = <word>`
            if state.is_none() {
                if let Some(rest) = line.strip_prefix("state =") {
                    let candidate = rest.trim().trim_end_matches(';').trim();
                    if !candidate.is_empty() {
                        state = Some(candidate.to_string());
                    }
                }
            }

            // Direct `program = <path>;` form takes precedence.
            if program_path.is_none() {
                if let Some(rest) = line.strip_prefix("program =") {
                    let candidate = rest.trim().trim_end_matches(';').trim();
                    if !candidate.is_empty() {
                        program_path = Some(PathBuf::from(candidate));
                    }
                }
            }

            // `program arguments = ...` form. The opening `(` may be on the
            // same line as the key (single-line form) or on a later line
            // (multiline block form).
            if program_path.is_none() {
                if let Some(rest) = line.strip_prefix("program arguments =").map(str::trim) {
                    if let Some(first) = extract_first_quoted(rest) {
                        program_path = Some(PathBuf::from(first));
                        in_program_arguments_block = false;
                        continue;
                    }
                    if rest.starts_with('(') {
                        in_program_arguments_block = true;
                    }
                    continue;
                }
            }

            if program_path.is_none() && in_program_arguments_block {
                if let Some(first) = extract_first_quoted(line) {
                    program_path = Some(PathBuf::from(first));
                    in_program_arguments_block = false;
                } else if line.contains(')') {
                    // Block closed without yielding a path.
                    in_program_arguments_block = false;
                }
            }
        }

        Ok(Self {
            program_path,
            pid,
            state,
        })
    }

    /// The absolute path launchd believes the service executable lives at,
    /// if present in the dump. `None` when the service is not registered or
    /// the dump does not include a program path.
    pub fn program_path(&self) -> Option<&Path> {
        self.program_path.as_deref()
    }

    /// The PID of the live service process, if present in the dump.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// The launchd job state (`running`, `waiting`, etc.), if present.
    pub fn state(&self) -> Option<&str> {
        self.state.as_deref()
    }
}

fn extract_first_quoted(s: &str) -> Option<&str> {
    let start = s.find('"')?;
    let tail = &s[start + 1..];
    let end = tail.find('"')?;
    Some(&tail[..end])
}

// ── tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launchctl_job_snapshot_extracts_registered_program_path() {
        let snapshot = LaunchdJobSnapshot::parse(LAUNCHCTL_PRINT_FIXTURE).unwrap();
        assert_eq!(
            snapshot.program_path(),
            Some(Path::new("/Old/Busytok.app/Contents/MacOS/busytok-service"))
        );
        assert_eq!(snapshot.pid(), Some(4242));
        assert_eq!(snapshot.state(), Some("running"));
    }

    #[test]
    fn launchctl_job_snapshot_handles_missing_program_line() {
        let snapshot = LaunchdJobSnapshot::parse(
            "	pid = 123\n\
             last exit code = 0\n",
        )
        .unwrap();
        assert!(snapshot.program_path().is_none());
        assert_eq!(snapshot.pid(), Some(123));
        assert!(snapshot.state().is_none());
    }

    #[test]
    fn launchctl_job_snapshot_falls_back_to_program_arguments() {
        let input = "	program arguments = (\n\t\t\"/Alt/Busytok.app/Contents/MacOS/busytok-service\",\n\t\t\"--serve\"\n\t)\n";
        let snapshot = LaunchdJobSnapshot::parse(input).unwrap();
        assert_eq!(
            snapshot.program_path(),
            Some(Path::new("/Alt/Busytok.app/Contents/MacOS/busytok-service"))
        );
    }

    #[test]
    fn launchctl_job_snapshot_extracts_pid_and_state() {
        let input = "pid = 96869\nstate = running\nprogram = /Applications/Busytok.app/Contents/MacOS/busytok-service;\n";
        let snapshot = LaunchdJobSnapshot::parse(input).unwrap();
        assert_eq!(snapshot.pid(), Some(96869));
        assert_eq!(snapshot.state(), Some("running"));
    }

    #[test]
    fn launchctl_job_snapshot_handles_malformed_pid() {
        let input = "pid = not_a_number\nstate = waiting\n";
        let snapshot = LaunchdJobSnapshot::parse(input).unwrap();
        assert!(snapshot.pid().is_none(), "malformed pid should be None");
        assert_eq!(snapshot.state(), Some("waiting"));
    }

    #[test]
    fn launchctl_job_snapshot_handles_no_pid() {
        let input = "state = waiting\nlast exit code = 0\n";
        let snapshot = LaunchdJobSnapshot::parse(input).unwrap();
        assert!(snapshot.pid().is_none());
        assert_eq!(snapshot.state(), Some("waiting"));
    }
}
