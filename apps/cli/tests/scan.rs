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
