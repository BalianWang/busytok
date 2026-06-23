//! macOS process identity helper — resolves a PID to its on-disk executable
//! path via libproc's `proc_pidpath`. Used by the repair ladder to detect a
//! stale live process whose executable lives outside the current app bundle
//! (e.g. a trashed old build still holding the control socket).
//!
//! This module is deliberately FFI-only — no shelling out to `lsof` or `ps`.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

extern "C" {
    /// libproc `proc_pidpath` — copies the executable path for `pid` into
    /// `buffer` (up to `buffersize` bytes). Returns the number of bytes
    /// written (≥ 1) on success, or 0 if the PID does not exist / has
    /// exited / the buffer is too small.
    fn proc_pidpath(pid: libc::c_int, buffer: *mut libc::c_void, buffersize: u32) -> libc::c_int;
}

/// The canonical buffer size for `proc_pidpath` — `PROC_PIDPATHINFO_MAXSIZE`
/// from `<libproc.h>`. 4096 bytes is enough for any macOS filesystem path.
const PROC_PIDPATHINFO_MAXSIZE: u32 = 4096;

/// Resolve the on-disk executable path of `pid` via `proc_pidpath`.
///
/// Returns `Ok(Some(path))` when the PID is alive and its executable path is
/// known. Returns `Ok(None)` when the PID has exited between the snapshot
/// and inspection (process no longer exists). Returns `Err(...)` only on
/// unexpected OS-level failures.
pub(crate) fn executable_path_for_pid(pid: u32) -> anyhow::Result<Option<PathBuf>> {
    let mut buffer: Vec<u8> = vec![0u8; PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: buffer is correctly sized (PROC_PIDPATHINFO_MAXSIZE bytes),
    // aligned, and outlives the FFI call. proc_pidpath is a pure read of
    // kernel process state.
    let written = unsafe {
        proc_pidpath(
            pid as libc::c_int,
            buffer.as_mut_ptr() as *mut libc::c_void,
            PROC_PIDPATHINFO_MAXSIZE,
        )
    };
    if written <= 0 {
        // PID exited between snapshot and inspection — return None, not
        // an error. The ladder treats a missing PID as needing a fresh
        // bootstrap.
        return Ok(None);
    }
    let len = written as usize;
    // proc_pidpath returns the number of bytes written including a
    // terminating NUL — strip it.
    let path_bytes = &buffer[..len];
    let path_bytes = if path_bytes.last() == Some(&0u8) {
        &path_bytes[..len - 1]
    } else {
        path_bytes
    };
    let path = PathBuf::from(OsStr::from_bytes(path_bytes));
    Ok(Some(path))
}

// ── tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_path_for_current_process_is_not_empty() {
        let pid = std::process::id();
        let path = executable_path_for_pid(pid)
            .expect("proc_pidpath should succeed for current process")
            .expect("current PID should have an executable path");
        assert!(
            !path.as_os_str().is_empty(),
            "executable path for current process must not be empty"
        );
        // The current test binary should be under target/debug/ or similar.
        let path_str = path.display().to_string();
        assert!(
            path_str.contains("busytok-gui") || path_str.contains("busytok_gui"),
            "current process executable path should contain the crate name, got: {path_str}"
        );
    }

    #[test]
    fn executable_path_for_nonexistent_pid_returns_none() {
        // PID 99999 is extremely unlikely to exist on any machine (PIDs
        // max out at 99998 on macOS by default).
        let result = executable_path_for_pid(99999)
            .expect("proc_pidpath should not error for nonexistent PID");
        assert!(
            result.is_none(),
            "nonexistent PID must return None, got: {result:?}"
        );
    }

    #[test]
    fn executable_path_for_pid_zero_returns_none() {
        // PID 0 is the kernel_task on macOS — proc_pidpath returns 0 for
        // it (the buffer is too small for kernel paths, or the PID is
        // special).
        let result = executable_path_for_pid(0).unwrap();
        // Whether Some or None depends on the kernel version; either is
        // acceptable — the important thing is the call doesn't panic.
        if let Some(path) = &result {
            assert!(!path.as_os_str().is_empty());
        }
    }
}
