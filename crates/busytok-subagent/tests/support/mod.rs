//! Shared test helpers for busytok-subagent integration tests.
//!
//! Provides Windows-compatible sidecar shell + bundle path resolution
//! so tests that spawn the mock sidecar work across all CI platforms.
//!
//! `#![allow(dead_code)]`: each integration test file compiles as its own
//! crate and only uses a subset of these helpers; unused helpers must not
//! trip `-D warnings`.

#![allow(dead_code)]

use std::path::PathBuf;

/// Path to the mock-sidecar.sh fixture, resolved relative to
/// CARGO_MANIFEST_DIR (crates/busytok-subagent).
pub fn mock_sidecar_script() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/mock-sidecar.sh");
    p
}

/// Resolve the shell binary used to launch the mock sidecar script.
/// On Windows, bash is not on PATH by default; use Git Bash.
pub fn sidecar_shell_path() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            return PathBuf::from(program_files)
                .join("Git")
                .join("bin")
                .join("bash.exe");
        }
        PathBuf::from(r"C:\Program Files\Git\bin\bash.exe")
    }

    #[cfg(not(windows))]
    {
        PathBuf::from("/bin/bash")
    }
}

/// Convert the mock sidecar script path to a form bash can execute.
/// On Windows, Git Bash expects MSYS-style paths (/c/users/...).
pub fn mock_sidecar_bundle_path() -> PathBuf {
    let path = mock_sidecar_script();
    #[cfg(windows)]
    {
        let raw = path.to_string_lossy().replace('\\', "/");
        if let Some((drive, rest)) = raw.split_once(":/") {
            let drive = drive.to_ascii_lowercase();
            return PathBuf::from(format!("/{drive}/{rest}"));
        }
        PathBuf::from(raw)
    }

    #[cfg(not(windows))]
    {
        path
    }
}
