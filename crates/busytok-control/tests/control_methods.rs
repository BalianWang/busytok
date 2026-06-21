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
    assert!(methods.contains(&"prompts.list".to_string()));
    assert!(methods.contains(&"prompts.get".to_string()));
    assert!(methods.contains(&"prompts.create".to_string()));
    assert!(methods.contains(&"prompts.update".to_string()));
    assert!(methods.contains(&"prompts.delete".to_string()));
    assert!(methods.contains(&"prompts.use".to_string()));
    assert!(methods.contains(&"prompts.suggest_tags".to_string()));
    // Old method names removed from the manifest
    assert!(!methods.iter().any(|m| m == "usage.dashboard"));
    assert!(!methods.iter().any(|m| m == "sources.list"));
    assert!(!methods.iter().any(|m| m == "diagnostics.scan_status"));
    assert!(!methods.iter().any(|m| m == "usage.export"));
    assert!(!methods.iter().any(|m| m.contains("proxy")));
    assert!(!methods.iter().any(|m| m.contains("tracking")));
    assert!(!methods.iter().any(|m| m.contains("leases")));
}
