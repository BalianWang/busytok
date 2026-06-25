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
use crate::desktop_shortcut::{next_retry_delay_ms, record_shortcut_failure, ShortcutDiagnostics};

#[test]
fn shortcut_retry_uses_bounded_backoff() {
    assert_eq!(next_retry_delay_ms(0), 1_000);
    assert_eq!(next_retry_delay_ms(1), 2_000);
    assert_eq!(next_retry_delay_ms(9), 30_000);
}

#[test]
fn shortcut_failure_is_visible_in_diagnostics() {
    let diagnostics = record_shortcut_failure(
        ShortcutDiagnostics::default(),
        "CommandOrControl+Option+K",
        "already registered",
    );
    assert_eq!(diagnostics.state, "failed");
    assert_eq!(diagnostics.shortcut, "CommandOrControl+Option+K");
    assert_eq!(
        diagnostics.failure_reason.as_deref(),
        Some("already registered")
    );
}
