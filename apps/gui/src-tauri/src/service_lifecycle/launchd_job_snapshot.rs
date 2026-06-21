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
pub(crate) const LAUNCHCTL_PRINT_FIXTURE: &str = include_str!(
    "../../tests/fixtures/launchctl_print_com.busytok.service.txt",
);

/// Parsed snapshot of `launchctl print gui/<uid>/<label>` output.
///
/// `SMAppService.status` does NOT expose the registered executable path, so
/// detecting a stale registration after an app move requires parsing the
/// launchd state. This struct pulls just the program path out of the
/// `launchctl print` dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchdJobSnapshot {
    program_path: Option<PathBuf>,
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
        let mut in_program_arguments_block = false;

        for raw in launchctl_print_output.lines() {
            let line = raw.trim();

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
                if let Some(rest) = line
                    .strip_prefix("program arguments =")
                    .map(str::trim)
                {
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

        Ok(Self { program_path })
    }

    /// The absolute path launchd believes the service executable lives at,
    /// if present in the dump. `None` when the service is not registered or
    /// the dump does not include a program path.
    pub fn program_path(&self) -> Option<&Path> {
        self.program_path.as_deref()
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
            Some(Path::new(
                "/Old/Busytok.app/Contents/MacOS/busytok-service"
            ))
        );
    }

    #[test]
    fn launchctl_job_snapshot_handles_missing_program_line() {
        let snapshot = LaunchdJobSnapshot::parse(
            "	pid = 123\n\
             last exit code = 0\n",
        )
        .unwrap();
        assert!(snapshot.program_path().is_none());
    }

    #[test]
    fn launchctl_job_snapshot_falls_back_to_program_arguments() {
        let input = "	program arguments = (\n\t\t\"/Alt/Busytok.app/Contents/MacOS/busytok-service\",\n\t\t\"--serve\"\n\t)\n";
        let snapshot = LaunchdJobSnapshot::parse(input).unwrap();
        assert_eq!(
            snapshot.program_path(),
            Some(Path::new(
                "/Alt/Busytok.app/Contents/MacOS/busytok-service"
            ))
        );
    }
}
