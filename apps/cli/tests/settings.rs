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

fn run_busytok(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_busytok"))
        .args(args)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn settings_help_exposes_snapshot_and_update() {
    let output = run_busytok(&["settings", "--help"]);
    assert!(
        output.contains("snapshot"),
        "settings help should mention 'snapshot'"
    );
    assert!(
        output.contains("update"),
        "settings help should mention 'update'"
    );
}

#[test]
fn settings_snapshot_is_recognized() {
    // Just test that the subcommand is recognized (will fail on RPC without service,
    // but should not be a clap parse error).
    let output = Command::new(env!("CARGO_BIN_EXE_busytok"))
        .args(["settings", "snapshot"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should either succeed or fail with a connection/RPC error (not a clap
    // parse error). A read_timeout occurs when a stale service socket exists
    // but the service is unresponsive — still proof the subcommand was
    // recognized and dispatched to the RPC layer.
    if !output.status.success() {
        assert!(
            stderr.contains("connecting to Busytok service")
                || stderr.contains("is it running?")
                || stderr.contains("RPC error")
                || stderr.contains("read_timeout"),
            "unexpected error: {stderr}"
        );
    }
}

#[test]
fn settings_update_help_shows_options() {
    let output = run_busytok(&["settings", "update", "--help"]);
    assert!(
        output.contains("--timezone"),
        "update help should mention --timezone"
    );
    assert!(
        output.contains("--discovery-default"),
        "update help should mention --discovery-default"
    );
    assert!(
        output.contains("--add-root"),
        "update help should mention --add-root"
    );
}

#[test]
fn settings_update_rejects_bad_discovery_default_format() {
    let output = Command::new(env!("CARGO_BIN_EXE_busytok"))
        .args(["settings", "update", "--discovery-default", "badformat"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "bad discovery-default format should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("expected agent:bool") || stderr.contains("error"),
        "should report parse error: {stderr}"
    );
}

#[test]
fn top_level_help_mentions_settings() {
    let output = run_busytok(&["--help"]);
    assert!(
        output.contains("settings"),
        "top-level help should mention 'settings'"
    );
}
