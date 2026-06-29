/// Returns the complete list of Surge UI control API method names.
pub fn method_manifest() -> Vec<String> {
    vec![
        // Service (kept from Phase 1)
        "service.health".to_string(),
        "service.status".to_string(),
        // Shell
        "shell.status".to_string(),
        // Overview (modular — replaces single overview.snapshot)
        "overview.summary".to_string(),
        "overview.trend".to_string(),
        "overview.heatmap".to_string(),
        "overview.rankings".to_string(),
        // Receipt
        "receipt.daily".to_string(),
        // Activity
        "activity.recent".to_string(),
        "activity.list".to_string(),
        "activity.detail".to_string(),
        // Breakdown
        "breakdown.list".to_string(),
        "breakdown.detail".to_string(),
        // Clients
        "clients.snapshot".to_string(),
        "clients.detail".to_string(),
        // Settings
        "settings.snapshot".to_string(),
        "settings.update".to_string(),
        "settings.diagnostics".to_string(),
        "settings.recovery_action".to_string(),
        // Prompt Palette
        "prompts.list".to_string(),
        "prompts.get".to_string(),
        "prompts.create".to_string(),
        "prompts.update".to_string(),
        "prompts.delete".to_string(),
        "prompts.use".to_string(),
        "prompts.suggest_tags".to_string(),
        // Events
        "events.subscribe".to_string(),
        // Live
        "live.window".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_manifest_contains_page_oriented_ui_methods() {
        let methods = method_manifest();
        assert!(
            methods.iter().any(|m| m == "shell.status"),
            "shell.status not found"
        );
        assert!(
            methods.iter().any(|m| m == "overview.summary"),
            "overview.summary not found"
        );
        assert!(
            methods.iter().any(|m| m == "activity.list"),
            "activity.list not found"
        );
        assert!(
            methods.iter().any(|m| m == "breakdown.detail"),
            "breakdown.detail not found"
        );
        assert!(
            !methods.iter().any(|m| m == "usage.dashboard"),
            "usage.dashboard should not be in manifest"
        );
    }

    #[test]
    fn method_manifest_contains_events_and_live_methods() {
        let methods = method_manifest();
        assert!(
            methods.iter().any(|m| m == "events.subscribe"),
            "events.subscribe not found in method manifest"
        );
        assert!(
            methods.iter().any(|m| m == "live.window"),
            "live.window not found in method manifest"
        );
    }
}
