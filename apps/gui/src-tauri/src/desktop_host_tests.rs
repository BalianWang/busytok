use crate::desktop_host::{
    shortcut_menu_label, tray_click_action, DesktopHostAction,
    ShortcutRegistrationState, TrayClickKind,
};

#[test]
fn tray_primary_click_shows_gui_and_secondary_click_opens_menu() {
    assert_eq!(
        tray_click_action(TrayClickKind::Primary),
        DesktopHostAction::ShowGui
    );
    assert_eq!(
        tray_click_action(TrayClickKind::Secondary),
        DesktopHostAction::OpenMenu
    );
}

#[test]
fn cmd_q_and_menu_quit_quit_host() {
    let action = DesktopHostAction::QuitDesktopHost;
    assert!(action.quits_host());
}

#[test]
fn quit_desktop_host_action_stops_service() {
    let action = DesktopHostAction::QuitDesktopHost;
    assert!(action.quits_host());
    assert!(action.stops_service());
}

#[test]
fn shortcut_failure_has_menu_diagnostic_label() {
    let status = ShortcutRegistrationState::Failed {
        shortcut: "CommandOrControl+Shift+K".into(),
        reason: "already registered".into(),
    };
    assert_eq!(shortcut_menu_label(&status), Some("Shortcut Unavailable"));
}
