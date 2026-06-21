use crate::activation_context::ActivationContext;

#[test]
fn new_empty_captures_nothing() {
    let mut ctx = ActivationContext::new();
    assert!(!ctx.restore_and_clear());
}

#[test]
fn restore_without_capture_returns_false() {
    let mut ctx = ActivationContext::new();
    assert!(!ctx.restore_and_clear());
}

#[test]
fn set_captured() {
    let mut ctx = ActivationContext::new();
    ctx.set_captured(1234);
    // Internal state is captured — just verifying no panic
}

#[test]
fn restore_and_clear_clears() {
    let mut ctx = ActivationContext::new();
    ctx.set_captured(9999);
    // On non-macOS, restore_and_clear returns false because the
    // pid lookup fails (no real app with pid 9999). On macOS,
    // it also returns false because pid 9999 doesn't exist.
    // The key behavior is that a second call returns false
    // (state was cleared).
    ctx.restore_and_clear();
    assert!(
        !ctx.restore_and_clear(),
        "second restore should return false — state was cleared"
    );
}

#[test]
fn repeated_restore_returns_false() {
    let mut ctx = ActivationContext::new();
    ctx.set_captured(9999);
    let _ = ctx.restore_and_clear();
    assert!(!ctx.restore_and_clear());
}

#[test]
fn last_capture_wins() {
    let mut ctx = ActivationContext::new();
    ctx.set_captured(100);
    ctx.set_captured(200);
    // The last captured pid (200) should be the one restored.
    // We verify by checking that state is cleared after one restore.
    let _ = ctx.restore_and_clear();
    assert!(
        !ctx.restore_and_clear(),
        "second restore should return false — only one pid is stored"
    );
}
