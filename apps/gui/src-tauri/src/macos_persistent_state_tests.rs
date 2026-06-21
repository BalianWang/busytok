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
