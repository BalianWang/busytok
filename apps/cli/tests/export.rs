use assert_cmd::Command;

fn run_busytok(args: &[&str]) -> String {
    let output = Command::cargo_bin("busytok")
        .unwrap()
        .args(args)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    format!("{stdout}{stderr}")
}

#[test]
fn usage_export_dry_run_prints_protocol_method() {
    let output = run_busytok(&[
        "usage",
        "export",
        "--kind",
        "events",
        "--format",
        "json",
        "--dry-run",
    ]);
    assert!(output.contains("usage.export"));
}

#[test]
fn usage_export_help_mentions_supported_kinds_and_formats() {
    let output = run_busytok(&["usage", "export", "--help"]);
    assert!(output.contains("events"));
    assert!(output.contains("timeline"));
    assert!(output.contains("json"));
    assert!(output.contains("csv"));
}

#[test]
fn usage_export_help_mentions_agent_filter() {
    let output = run_busytok(&["usage", "export", "--help"]);
    assert!(output.contains("agent"));
}
