#![allow(
    clippy::await_holding_lock,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::field_reassign_with_default,
    clippy::uninlined_format_args,
    clippy::inconsistent_digit_grouping,
    clippy::len_zero,
    clippy::identity_op,
    clippy::useless_vec,
    clippy::manual_dangling_ptr,
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::absurd_extreme_comparisons,
    unused_variables,
    unused_imports,
    dead_code
)]
use std::process::Command;

#[test]
fn offline_scan_command_accepts_claude_path() {
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/claude-code");
    let output = Command::new(env!("CARGO_BIN_EXE_busytok"))
        .args([
            "scan",
            "--offline",
            "--agent",
            "claude-code",
            "--path",
            fixture.to_str().unwrap(),
        ])
        .output()
        .expect("run busytok offline scan");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("scan complete"), "stdout: {stdout}");
}

#[test]
fn normal_sources_rescan_uses_control_method() {
    let output = Command::new(env!("CARGO_BIN_EXE_busytok"))
        .args(["sources", "rescan", "--dry-run"])
        .output()
        .expect("run busytok sources rescan");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("sources.rescan"),
        "stdout should contain method name, got: {stdout}"
    );
}
