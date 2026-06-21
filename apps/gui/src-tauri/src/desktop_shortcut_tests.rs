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
        "CommandOrControl+Shift+K",
        "already registered",
    );
    assert_eq!(diagnostics.state, "failed");
    assert_eq!(diagnostics.shortcut, "CommandOrControl+Shift+K");
    assert_eq!(
        diagnostics.failure_reason.as_deref(),
        Some("already registered")
    );
}
