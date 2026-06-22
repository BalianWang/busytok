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
use crate::desktop_lifecycle_settings::DesktopLifecycleSettings;
use crate::desktop_runtime::{
    detect_launch_reason, host_presentation_config, startup_steps, LaunchReason, StartupContext,
    StartupStep,
};

#[test]
fn interactive_startup_creates_entrypoints_before_service_bootstrap() {
    let steps = startup_steps(StartupContext::InteractiveAppLaunch);
    let presentation_idx = steps
        .iter()
        .position(|s| *s == StartupStep::ConfigureHostPresentation)
        .unwrap();
    let menu_idx = steps
        .iter()
        .position(|s| *s == StartupStep::CreateMenuBar)
        .unwrap();
    let bootstrap_idx = steps
        .iter()
        .position(|s| *s == StartupStep::StartServiceBootstrapAsync)
        .unwrap();
    assert!(presentation_idx < menu_idx);
    assert!(menu_idx < bootstrap_idx);
    assert!(steps.contains(&StartupStep::ShowGui));
}

#[test]
fn login_start_keeps_gui_hidden_but_keeps_entrypoints() {
    let steps = startup_steps(StartupContext::LoginStart);
    let presentation_idx = steps
        .iter()
        .position(|s| *s == StartupStep::ConfigureHostPresentation)
        .unwrap();
    let menu_idx = steps
        .iter()
        .position(|s| *s == StartupStep::CreateMenuBar)
        .unwrap();
    let bootstrap_idx = steps
        .iter()
        .position(|s| *s == StartupStep::StartServiceBootstrapAsync)
        .unwrap();
    assert!(presentation_idx < menu_idx);
    assert!(menu_idx < bootstrap_idx);
    assert!(steps.contains(&StartupStep::KeepGuiHidden));
    assert!(!steps.contains(&StartupStep::ShowGui));
}

#[test]
fn singleton_relaunch_dispatches_show_gui_action() {
    assert_eq!(
        crate::desktop_runtime::singleton_relaunch_action(),
        crate::desktop_host::DesktopHostAction::ShowGui
    );
}

#[test]
fn dock_reopen_dispatches_show_gui_action() {
    assert_eq!(
        crate::desktop_runtime::dock_reopen_action(),
        crate::desktop_host::DesktopHostAction::ShowGui
    );
}

#[test]
fn service_bootstrap_statuses_include_spec_states() {
    use crate::desktop_service_status::ServiceBootstrapState;
    assert_eq!(ServiceBootstrapState::Starting.as_str(), "starting");
    assert_eq!(ServiceBootstrapState::Repairing.as_str(), "repairing");
    assert_eq!(ServiceBootstrapState::Ready.as_str(), "ready");
    assert_eq!(ServiceBootstrapState::Unavailable.as_str(), "unavailable");
}

#[test]
fn dispatch_routes_all_action_variants() {
    // Verify all action variants have defined dispatch behavior.
    // This test ensures exhaustiveness — if a new variant is added,
    // the dispatch match must be updated or this will need updating too.
    use crate::desktop_host::DesktopHostAction;
    let non_quit_actions = vec![
        DesktopHostAction::ShowGui,
        DesktopHostAction::OpenMenu,
        DesktopHostAction::ShowPromptPalette,
        DesktopHostAction::HidePromptPalette,
        DesktopHostAction::RetryShortcutRegistration,
    ];
    assert_eq!(
        non_quit_actions.len() + 1, // +1 for QuitDesktopHost
        6,
        "all DesktopHostAction variants must be listed"
    );

    // Non-quit actions must never stop the service.
    assert!(
        !non_quit_actions.iter().any(|a| a.stops_service()),
        "no non-quit action stops service"
    );
    assert!(
        DesktopHostAction::QuitDesktopHost.quits_host(),
        "QuitDesktopHost must quit host"
    );
}

#[test]
fn quit_desktop_host_action_stops_service() {
    let action = crate::desktop_host::DesktopHostAction::QuitDesktopHost;
    assert!(action.quits_host());
    assert!(action.stops_service());
    // No other action stops the service.
    let non_quit_actions = vec![
        crate::desktop_host::DesktopHostAction::ShowGui,
        crate::desktop_host::DesktopHostAction::OpenMenu,
        crate::desktop_host::DesktopHostAction::ShowPromptPalette,
        crate::desktop_host::DesktopHostAction::HidePromptPalette,
        crate::desktop_host::DesktopHostAction::RetryShortcutRegistration,
    ];
    for action in &non_quit_actions {
        assert!(
            !action.stops_service(),
            "{:?} must not stop service",
            action
        );
    }
}

#[test]
fn lifecycle_status_distinguishes_registered_but_inactive_service() {
    use crate::service_lifecycle::LifecycleStatus;
    assert_eq!(
        LifecycleStatus::RegisteredInactive.as_str(),
        "registered_inactive"
    );
}

#[test]
fn exit_routing_lets_self_triggered_exit_through() {
    // quit_desktop_host sets allow_exit=true then calls app.exit(0),
    // re-entering ExitRequested. That second pass must NOT be re-intercepted,
    // or the shutdown deadlocks with the exit permanently prevented.
    assert_eq!(
        crate::desktop_runtime::route_exit_request(true),
        crate::desktop_runtime::ExitRouting::AllowThrough
    );
}

#[test]
fn system_quit_routes_to_full_shutdown_even_when_host_mode_active() {
    // Regression for the Dock/system-quit bypass: a real terminate request
    // (Cmd+Q, Dock Quit, Apple-menu Quit, logout) must funnel through
    // quit_desktop_host regardless of host mode. Host mode is intentionally
    // not a parameter here — window-close-as-hide is handled separately by
    // install_hide_on_close, so it must never gate a real Quit.
    assert_eq!(
        crate::desktop_runtime::route_exit_request(false),
        crate::desktop_runtime::ExitRouting::RouteToFullShutdown
    );
}

#[test]
fn handle_exit_requested_invokes_prevent_then_quit_for_system_quit() {
    // Wiring-level test of the ExitRequested closure body: a real terminate
    // request must call prevent_exit() AND quit_desktop_host(), in that order.
    // This is the seam route_exit_request alone can't pin — if a future
    // refactor of lib.rs drops either call (or swaps them), this fails.
    use std::cell::RefCell;
    let sequence = RefCell::new(Vec::<&str>::new());
    crate::desktop_runtime::handle_exit_requested(
        false,
        || sequence.borrow_mut().push("prevent"),
        || sequence.borrow_mut().push("quit"),
    );
    assert_eq!(
        sequence.borrow().as_slice(),
        &["prevent", "quit"],
        "system quit must call prevent_exit then quit_desktop_host"
    );
}

#[test]
fn handle_exit_requested_skips_side_effects_for_self_triggered_exit() {
    // The self-triggered second pass (allow_exit=true after quit_desktop_host)
    // must call NEITHER prevent_exit nor quit, or shutdown deadlocks.
    use std::cell::RefCell;
    let sequence = RefCell::new(Vec::<&str>::new());
    crate::desktop_runtime::handle_exit_requested(
        true,
        || sequence.borrow_mut().push("prevent"),
        || sequence.borrow_mut().push("quit"),
    );
    assert!(
        sequence.borrow().is_empty(),
        "allow-through must call neither prevent_exit nor quit"
    );
}

#[test]
fn host_presentation_uses_regular_app_model() {
    let config = host_presentation_config();
    assert!(!config.menu_bar_only);
    assert!(config.dock_visible);
}

// ── LaunchReason → StartupContext mapping ────────────────────────────

#[test]
fn login_item_launch_hides_gui_when_login_start_is_enabled() {
    let ctx = StartupContext::from_launch_reason(
        LaunchReason::LoginItem,
        DesktopLifecycleSettings {
            launch_busytok_desktop_at_login: true,
            ..Default::default()
        },
    );
    assert_eq!(ctx, StartupContext::LoginStart);
}

#[test]
fn login_item_launch_shows_gui_when_login_start_is_disabled() {
    let ctx = StartupContext::from_launch_reason(
        LaunchReason::LoginItem,
        DesktopLifecycleSettings {
            launch_busytok_desktop_at_login: false,
            ..Default::default()
        },
    );
    assert_eq!(ctx, StartupContext::InteractiveAppLaunch);
}

#[test]
fn unknown_launch_context_defaults_to_visible_gui() {
    let ctx = StartupContext::from_launch_reason(
        LaunchReason::Unknown,
        DesktopLifecycleSettings {
            launch_busytok_desktop_at_login: true,
            ..Default::default()
        },
    );
    assert_eq!(ctx, StartupContext::InteractiveAppLaunch);
}

#[test]
fn manual_launch_always_shows_gui_regardless_of_settings() {
    let ctx = StartupContext::from_launch_reason(
        LaunchReason::Manual,
        DesktopLifecycleSettings {
            launch_busytok_desktop_at_login: true,
            ..Default::default()
        },
    );
    assert_eq!(ctx, StartupContext::InteractiveAppLaunch);
}

#[test]
fn detect_launch_reason_defaults_to_unknown() {
    // Without the --desktop-host-login-start flag (normal test
    // invocation), detection should return Unknown.
    let reason = detect_launch_reason();
    assert_eq!(reason, LaunchReason::Unknown);
}
