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
use crate::desktop_windows::{
    cancel_pending_fullscreen_hide, close_hide_strategy, is_pending_hide_stale, main_window_config,
    CloseHideStrategy, MAIN_LABEL,
};

#[test]
fn close_uses_immediate_hide_when_not_fullscreen() {
    assert_eq!(close_hide_strategy(false), CloseHideStrategy::Immediate);
}

#[test]
fn close_defers_hide_until_fullscreen_exit_finishes() {
    use crate::desktop_windows::{
        FULLSCREEN_CLOSE_HIDE_MIN_DELAY_MS, FULLSCREEN_CLOSE_HIDE_POLL_INTERVAL_MS,
        FULLSCREEN_CLOSE_HIDE_TIMEOUT_MS,
    };
    assert_eq!(
        close_hide_strategy(true),
        CloseHideStrategy::AfterFullscreenExit {
            min_delay_ms: FULLSCREEN_CLOSE_HIDE_MIN_DELAY_MS,
            timeout_ms: FULLSCREEN_CLOSE_HIDE_TIMEOUT_MS,
            poll_interval_ms: FULLSCREEN_CLOSE_HIDE_POLL_INTERVAL_MS,
        }
    );
}

#[test]
fn main_window_config_matches_tauri_conf_json() {
    let config = main_window_config();

    // Read the actual tauri.conf.json so this test catches config drift.
    let conf_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let content = std::fs::read_to_string(&conf_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", conf_path.display()));
    let json: serde_json::Value = serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("failed to parse tauri.conf.json: {e}"));

    let window = &json["app"]["windows"][0];
    assert_eq!(
        config.label,
        window["label"].as_str().unwrap_or_default(),
        "label must match tauri.conf.json"
    );
    assert_eq!(
        config.title,
        window["title"].as_str().unwrap_or_default(),
        "title must match tauri.conf.json"
    );
    assert_eq!(
        config.width,
        window["width"].as_f64().unwrap_or_default(),
        "width must match tauri.conf.json"
    );
    assert_eq!(
        config.height,
        window["height"].as_f64().unwrap_or_default(),
        "height must match tauri.conf.json"
    );
    // Also verify MAIN_LABEL used for window lookup is consistent.
    assert_eq!(MAIN_LABEL, config.label);
}

#[test]
fn cancel_pending_hide_invalidates_stale_snapshot() {
    // cancel_pending_fullscreen_hide returns the pre-increment value —
    // exactly the snapshot a delayed-hide task would have captured before
    // waiting for the fullscreen exit animation.
    let stale_snapshot = cancel_pending_fullscreen_hide();
    assert!(
        is_pending_hide_stale(stale_snapshot),
        "snapshot from before show_gui should be detected as stale"
    );
}
