#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayClickKind {
    Primary,
    Secondary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowLabel {
    Main,
    PromptPalette,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopHostAction {
    ShowGui,
    OpenMenu,
    ShowPromptPalette,
    HidePromptPalette,
    RetryShortcutRegistration,
    QuitDesktopHost,
}

impl DesktopHostAction {
    pub fn quits_host(&self) -> bool {
        matches!(self, Self::QuitDesktopHost)
    }

    pub fn stops_service(&self) -> bool {
        matches!(self, Self::QuitDesktopHost)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutRegistrationState {
    Idle,
    Registered { shortcut: String },
    Failed { shortcut: String, reason: String },
}

pub fn tray_click_action(kind: TrayClickKind) -> DesktopHostAction {
    match kind {
        TrayClickKind::Primary => DesktopHostAction::ShowGui,
        TrayClickKind::Secondary => DesktopHostAction::OpenMenu,
    }
}

pub fn shortcut_menu_label(status: &ShortcutRegistrationState) -> Option<&'static str> {
    match status {
        ShortcutRegistrationState::Failed { .. } => Some("Shortcut Unavailable"),
        _ => None,
    }
}
