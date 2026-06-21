use busytok_protocol::method_manifest;

#[test]
fn manifest_contains_surge_ui_methods() {
    let methods = method_manifest();
    assert!(methods.contains(&"service.health".to_string()));
    assert!(methods.contains(&"service.status".to_string()));
    assert!(methods.contains(&"shell.status".to_string()));
    assert!(methods.contains(&"overview.summary".to_string()));
    assert!(methods.contains(&"activity.list".to_string()));
    assert!(methods.contains(&"activity.detail".to_string()));
    assert!(methods.contains(&"breakdown.list".to_string()));
    assert!(methods.contains(&"breakdown.detail".to_string()));
    assert!(methods.contains(&"clients.snapshot".to_string()));
    assert!(methods.contains(&"clients.detail".to_string()));
    assert!(methods.contains(&"settings.snapshot".to_string()));
    assert!(methods.contains(&"settings.update".to_string()));
    assert!(methods.contains(&"settings.diagnostics".to_string()));
    assert!(methods.contains(&"settings.recovery_action".to_string()));
    // Old Phase 1 method names — should NOT be in the manifest anymore
    assert!(!methods.iter().any(|m| m == "usage.dashboard"));
    assert!(!methods.iter().any(|m| m == "sources.list"));
    assert!(!methods.iter().any(|m| m == "diagnostics.scan_status"));
    assert!(!methods.iter().any(|m| m == "usage.export"));
    // Sanity checks
    assert!(!methods.iter().any(|m| m.contains("proxy")));
    assert!(!methods.iter().any(|m| m.contains("tracking")));
    assert!(!methods.iter().any(|m| m.contains("leases")));
}

/// Asserts the full modular realtime audit method surface is present in the
/// manifest — all 18 methods from the four-plane architecture.
#[test]
fn manifest_contains_modular_realtime_audit_methods() {
    let methods = method_manifest();
    // Shell
    assert!(
        methods.contains(&"shell.status".to_string()),
        "shell.status"
    );
    // Overview (modular — replaces single overview.snapshot)
    assert!(
        methods.contains(&"overview.summary".to_string()),
        "overview.summary"
    );
    assert!(
        methods.contains(&"overview.trend".to_string()),
        "overview.trend"
    );
    assert!(
        methods.contains(&"overview.heatmap".to_string()),
        "overview.heatmap"
    );
    assert!(
        methods.contains(&"overview.rankings".to_string()),
        "overview.rankings"
    );
    // Activity
    assert!(
        methods.contains(&"activity.recent".to_string()),
        "activity.recent"
    );
    assert!(
        methods.contains(&"activity.list".to_string()),
        "activity.list"
    );
    assert!(
        methods.contains(&"activity.detail".to_string()),
        "activity.detail"
    );
    // Breakdown
    assert!(
        methods.contains(&"breakdown.list".to_string()),
        "breakdown.list"
    );
    assert!(
        methods.contains(&"breakdown.detail".to_string()),
        "breakdown.detail"
    );
    // Clients
    assert!(
        methods.contains(&"clients.snapshot".to_string()),
        "clients.snapshot"
    );
    assert!(
        methods.contains(&"clients.detail".to_string()),
        "clients.detail"
    );
    // Settings
    assert!(
        methods.contains(&"settings.snapshot".to_string()),
        "settings.snapshot"
    );
    assert!(
        methods.contains(&"settings.update".to_string()),
        "settings.update"
    );
    assert!(
        methods.contains(&"settings.diagnostics".to_string()),
        "settings.diagnostics"
    );
    assert!(
        methods.contains(&"settings.recovery_action".to_string()),
        "settings.recovery_action"
    );
    // Live
    assert!(methods.contains(&"live.window".to_string()), "live.window");
    // Events
    assert!(
        methods.contains(&"events.subscribe".to_string()),
        "events.subscribe"
    );
}

#[test]
fn manifest_contains_prompt_palette_methods() {
    let methods = method_manifest();
    for method in [
        "prompts.list",
        "prompts.get",
        "prompts.create",
        "prompts.update",
        "prompts.delete",
        "prompts.use",
        "prompts.suggest_tags",
    ] {
        assert!(methods.contains(&method.to_string()), "missing {method}");
    }
}
