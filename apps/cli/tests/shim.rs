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
//! Integration tests for the CLI shim subcommands.
//!
//! These tests exercise the CLI argument parsing and error paths by
//! running the `busytok` binary as a subprocess. Unit-level coverage of
//! `resolve_app_bundle_for_shim` and `ShimManager` lives in
//! `apps/cli/src/sim.rs` under `#[cfg(test)] mod tests`.

use std::process::Command;

fn busytok_binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_busytok"))
}

#[test]
fn cli_install_subcommand_is_defined() {
    let output = busytok_binary()
        .arg("cli")
        .arg("install")
        .arg("--help")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("busytok") && stdout.contains("install") && stdout.contains("bin-dir"),
        "cli install --help should explain the subcommand: {stdout}"
    );
}

#[test]
fn cli_status_subcommand_is_defined() {
    let output = busytok_binary()
        .arg("cli")
        .arg("status")
        .arg("--help")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("busytok") && stdout.contains("status"),
        "cli status --help should explain the subcommand: {stdout}"
    );
}

#[test]
fn cli_uninstall_subcommand_is_defined() {
    let output = busytok_binary()
        .arg("cli")
        .arg("uninstall")
        .arg("--help")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("busytok") && stdout.contains("uninstall"),
        "cli uninstall --help should explain the subcommand: {stdout}"
    );
}

#[test]
fn cli_help_lists_cli_subcommand() {
    let output = busytok_binary().arg("--help").output().unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("cli"),
        "top-level --help should list the cli subcommand: {stdout}"
    );
}
