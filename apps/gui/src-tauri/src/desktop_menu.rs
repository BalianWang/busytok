use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::AppHandle;

pub const MENU_OPEN_ID: &str = "open-busytok";
pub const MENU_QUIT_HOST_ID: &str = "quit-busytok-desktop";

const MENU_BAR_ICON_PNG: &[u8] = include_bytes!("../icons/menu-bar-template.png");

pub fn install_menu_bar(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, MENU_OPEN_ID, "Open Busytok", true, None::<&str>)?;
    // The CmdOrCtrl+Q accelerator coexists with the system Cmd+Q handler.
    // Both paths converge on quit_desktop_host — the accelerator ensures the
    // menu item works even if the system handler is intercepted.
    let quit = MenuItem::with_id(
        app,
        MENU_QUIT_HOST_ID,
        "Quit Busytok Desktop",
        true,
        Some("CmdOrCtrl+Q"),
    )?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    let icon = Image::from_bytes(MENU_BAR_ICON_PNG)?;

    TrayIconBuilder::with_id("busytok-desktop-host")
        .icon(icon)
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_OPEN_ID => {
                tracing::info!(
                    event_code = "desktop_host.menu_open_clicked",
                    "open menu item clicked"
                );
                crate::desktop_runtime::dispatch_host_action(
                    app,
                    crate::desktop_host::DesktopHostAction::ShowGui,
                );
            }
            MENU_QUIT_HOST_ID => {
                tracing::info!(
                    event_code = "desktop_host.menu_quit_clicked",
                    "quit menu item clicked"
                );
                crate::desktop_runtime::quit_desktop_host(app);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::desktop_runtime::dispatch_host_action(
                    tray.app_handle(),
                    crate::desktop_host::DesktopHostAction::ShowGui,
                );
            }
        })
        .build(app)?;
    Ok(())
}
