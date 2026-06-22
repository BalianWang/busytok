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
fn help_mentions_busytok() {
    let output = Command::new(env!("CARGO_BIN_EXE_busytok"))
        .arg("--help")
        .output()
        .expect("run busytok --help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("Local-first agent usage audit"));
}

#[test]
fn help_shows_subcommands() {
    let output = Command::new(env!("CARGO_BIN_EXE_busytok"))
        .arg("--help")
        .output()
        .expect("run busytok --help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    // Top-level commands
    assert!(stdout.contains("status"), "help should mention 'status'");
    assert!(stdout.contains("scan"), "help should mention 'scan'");
    assert!(stdout.contains("sources"), "help should mention 'sources'");
    assert!(stdout.contains("usage"), "help should mention 'usage'");
    assert!(
        stdout.contains("diagnostics"),
        "help should mention 'diagnostics'"
    );
}

#[test]
fn sources_subcommand_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_busytok"))
        .args(["sources", "--help"])
        .output()
        .expect("run busytok sources --help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("list"),
        "sources help should mention 'list'"
    );
    assert!(
        stdout.contains("rescan"),
        "sources help should mention 'rescan'"
    );
}

#[test]
fn usage_subcommand_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_busytok"))
        .args(["usage", "--help"])
        .output()
        .expect("run busytok usage --help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("summary"),
        "usage help should mention 'summary'"
    );
    assert!(
        stdout.contains("timeline"),
        "usage help should mention 'timeline'"
    );
    assert!(
        stdout.contains("events"),
        "usage help should mention 'events'"
    );
    assert!(
        stdout.contains("projects"),
        "usage help should mention 'projects'"
    );
    assert!(
        stdout.contains("models"),
        "usage help should mention 'models'"
    );
    assert!(
        stdout.contains("sessions"),
        "usage help should mention 'sessions'"
    );
}

#[test]
fn diagnostics_subcommand_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_busytok"))
        .args(["diagnostics", "--help"])
        .output()
        .expect("run busytok diagnostics --help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("scan-status"),
        "diagnostics help should mention 'scan-status'"
    );
    assert!(
        stdout.contains("store-health"),
        "diagnostics help should mention 'store-health'"
    );
}
