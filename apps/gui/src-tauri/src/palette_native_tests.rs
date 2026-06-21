use crate::palette_native::{palette_dev_url, PaletteNativeConfig};

#[test]
fn config_defaults_match_spec() {
    let config = PaletteNativeConfig::default();
    assert_eq!(config.width, 760);
    assert_eq!(config.height, 520);
}

#[test]
fn dev_url_is_localhost() {
    let url = palette_dev_url();
    assert!(url.starts_with("http://localhost:1420/"));
    assert!(url.contains("window=prompt-palette"));
}

#[test]
fn production_palette_uses_html_string_base_url_loading() {
    assert_eq!(
        crate::palette_native::palette_load_strategy(false),
        crate::palette_native::PaletteLoadStrategy::LocalHtmlStringWithBaseUrl,
        "local WKWebView palette loading must read index.html and use loadHTMLString:baseURL: so module scripts execute"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn native_palette_uses_transparent_webview_backing() {
    assert!(
        !crate::palette_native::palette_webview_draws_background(),
        "native palette WKWebView should stay transparent; the React surface draws the visible card"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn native_palette_style_mask_uses_titlebar_backed_nonactivating_panel() {
    let style_mask = crate::palette_native::palette_style_mask();

    assert_ne!(
        style_mask & crate::palette_native::NS_TITLED_WINDOW_MASK,
        0,
        "WKWebView should be hosted in a titled/full-size-content panel so AppKit opts into stable layer backing"
    );
    assert_ne!(
        style_mask & crate::palette_native::NS_FULL_SIZE_CONTENT_VIEW_MASK,
        0,
        "full-size content view keeps the titlebar hidden while preserving titled-window backing"
    );
    assert_ne!(
        style_mask & crate::palette_native::NS_NONACTIVATING_PANEL_MASK,
        0,
        "palette must remain non-activating"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn native_palette_collection_behavior_targets_current_space() {
    let behavior = crate::palette_native::palette_collection_behavior();

    assert_eq!(
        behavior & crate::palette_native::CAN_JOIN_ALL_SPACES,
        0,
        "palette should not remain visible across every Space after the user switches desktops"
    );
    assert_ne!(
        behavior & crate::palette_native::MOVE_TO_ACTIVE_SPACE,
        0,
        "palette should move to the currently active Space when shown"
    );
    assert_ne!(
        behavior & crate::palette_native::FULL_SCREEN_AUXILIARY,
        0,
        "palette should still be allowed above a fullscreen app"
    );
    assert_ne!(
        behavior & crate::palette_native::IGNORES_CYCLE,
        0,
        "palette should not appear in the window cycle"
    );
}

// ─── Mouse-screen placement policy ───────────────────────────────────────────
//
// The palette must open on the screen the mouse is on (launcher semantics),
// not the screen the app's main window lives on. The policy lives in pure
// functions so the selection / fallback / centering math is testable without
// any AppKit FFI. macOS coordinate space is bottom-left origin.

#[cfg(target_os = "macos")]
#[allow(clippy::approx_constant)]
mod placement {
    use crate::palette_native::macos::{
        resolve_panel_frame, screen_index_containing, NSPoint, NSRect, NSSize, ScreenGeometry,
    };

    const PANEL: NSSize = NSSize {
        width: 760.0,
        height: 520.0,
    };

    fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect {
            origin: NSPoint { x, y },
            size: NSSize { width: w, height: h },
        }
    }

    /// Build a screen from a full frame plus an explicit visible-frame origin
    /// and size (so tests can model menu-bar / Dock insets precisely).
    fn screen(frame: NSRect, vis_x: f64, vis_y: f64, vis_w: f64, vis_h: f64) -> ScreenGeometry {
        ScreenGeometry {
            frame,
            visible_frame: rect(vis_x, vis_y, vis_w, vis_h),
        }
    }

    /// Two side-by-side displays, bottom-left origin. Screen 0 is the main
    /// (left) screen with a 60px Dock at the bottom; screen 1 sits to its right.
    fn dual_screen_layout() -> Vec<ScreenGeometry> {
        vec![
            screen(rect(0.0, 0.0, 1440.0, 900.0), 0.0, 60.0, 1440.0, 840.0),
            screen(rect(1440.0, 0.0, 1920.0, 1080.0), 1440.0, 0.0, 1920.0, 1050.0),
        ]
    }

    #[test]
    fn selects_left_screen_when_mouse_is_on_it() {
        let screens = dual_screen_layout();
        // Center of screen 0.
        let frame = resolve_panel_frame(NSPoint { x: 720.0, y: 450.0 }, &screens, PANEL);
        // Centered in screen 0's visible frame (origin 0,60 size 1440x840).
        assert_eq!(frame.origin.x, (1440.0 - 760.0) / 2.0);
        assert_eq!(frame.origin.y, 60.0 + (840.0 - 520.0) / 2.0);
    }

    #[test]
    fn selects_right_screen_when_mouse_is_on_it() {
        let screens = dual_screen_layout();
        // Within screen 1 (x range 1440..3360).
        let frame = resolve_panel_frame(NSPoint { x: 2400.0, y: 500.0 }, &screens, PANEL);
        // Centered in screen 1's visible frame (origin 1440,0 size 1920x1050).
        assert_eq!(frame.origin.x, 1440.0 + (1920.0 - 760.0) / 2.0);
        assert_eq!(frame.origin.y, (1050.0 - 520.0) / 2.0);
    }

    #[test]
    fn falls_back_to_main_screen_when_mouse_is_off_host() {
        let screens = dual_screen_layout();
        // Off to the left of every screen.
        let frame = resolve_panel_frame(NSPoint { x: -500.0, y: 500.0 }, &screens, PANEL);
        // Index 0 is the main screen; fallback must land there.
        assert_eq!(frame.origin.x, (1440.0 - 760.0) / 2.0);
        assert_eq!(frame.origin.y, 60.0 + (840.0 - 520.0) / 2.0);
    }

    #[test]
    fn containment_uses_full_frame_not_visible_frame() {
        // Menu bar occupies the top of the frame but is outside the visible
        // frame. A mouse there must still resolve to THIS screen.
        let s = screen(rect(0.0, 0.0, 1440.0, 900.0), 0.0, 0.0, 1440.0, 877.0);
        let mouse_in_menu_bar = NSPoint { x: 720.0, y: 890.0 }; // y in [877, 900)
        assert_eq!(
            screen_index_containing(mouse_in_menu_bar, std::slice::from_ref(&s)),
            Some(0),
            "a mouse over the menu bar is still on that screen"
        );
        // ...and centering still uses the visible frame (height 877), not 900.
        let frame = resolve_panel_frame(mouse_in_menu_bar, std::slice::from_ref(&s), PANEL);
        assert_eq!(frame.origin.y, (877.0 - 520.0) / 2.0);
    }

    #[test]
    fn centering_honours_visible_frame_origin_above_dock() {
        // Screen 0's visible frame starts at y=60 (Dock at the bottom). The
        // panel must center within the visible band, never start below y=60.
        let screens = dual_screen_layout();
        let frame = resolve_panel_frame(NSPoint { x: 720.0, y: 450.0 }, &screens, PANEL);
        assert!(
            frame.origin.y >= 60.0,
            "panel must not start below the visible frame origin (Dock zone)"
        );
    }

    #[test]
    fn screen_index_returns_none_for_off_host_mouse() {
        let screens = dual_screen_layout();
        assert_eq!(
            screen_index_containing(NSPoint { x: -1.0, y: -1.0 }, &screens),
            None
        );
    }
}
