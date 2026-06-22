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
#[test]
fn disables_appkit_persistent_ui_restore_defaults() {
    let overrides = crate::macos_persistent_state::persistent_state_default_overrides();

    assert!(
        overrides.contains(&("NSQuitAlwaysKeepsWindows", false)),
        "Busytok is not a document-style app; macOS Resume must not keep old windows"
    );
    assert!(
        overrides.contains(&("ApplePersistenceIgnoreState", true)),
        "existing crash-history saved state must be ignored so launch is never blocked by AppKit's restore prompt"
    );
}
