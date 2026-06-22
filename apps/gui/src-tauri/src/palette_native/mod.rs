//! Native palette panel creation for macOS (NSPanel + WKWebView) and
//! Windows (placeholder, implemented in Task 6.4).
//!
//! This module provides the cross-platform types and re-exports the
//! platform-specific implementations.

use std::ffi::c_void;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Platform-specific modules
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

// ---------------------------------------------------------------------------
// Cross-platform types
// ---------------------------------------------------------------------------

/// How the palette panel loads its initial content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteLoadStrategy {
    /// Load the palette from the Vite dev server (debug mode).
    DevServerRequest,
    /// Load the palette from a local HTML string with a file:// base URL
    /// (production mode).
    LocalHtmlStringWithBaseUrl,
}

/// Choose the load strategy based on whether we're in debug mode.
pub fn palette_load_strategy(is_debug: bool) -> PaletteLoadStrategy {
    if is_debug {
        PaletteLoadStrategy::DevServerRequest
    } else {
        PaletteLoadStrategy::LocalHtmlStringWithBaseUrl
    }
}

/// Compute the path to `index.html` inside the bundle resource directory.
pub fn palette_index_html_path(bundle_resource_dir: &str) -> PathBuf {
    PathBuf::from(bundle_resource_dir.trim_end_matches('/')).join("index.html")
}

/// Configuration for the native palette window.
#[derive(Debug, Clone)]
pub struct PaletteNativeConfig {
    pub width: u32,
    pub height: u32,
    pub is_debug: bool,
    pub bundle_resource_dir: String,
}

impl Default for PaletteNativeConfig {
    fn default() -> Self {
        Self {
            width: 760,
            height: 520,
            is_debug: cfg!(debug_assertions),
            bundle_resource_dir: String::new(),
        }
    }
}

/// Owning handle for the native panel, webview, and message-handler objects.
pub struct PaletteNativeWindow {
    pub panel: *mut c_void,
    pub webview: *mut c_void,
    pub handler: *mut c_void,
}

// The panel objects are managed on the main thread; marking Send allows
// storing the handle behind a mutex usable from any thread.
//
// SAFETY: On macOS, `panel`, `webview`, and `handler` are raw pointers into
// single-threaded AppKit / WebKit objects that must only be touched on the
// main thread. On Windows, `panel` and `webview` deliberately alias the same
// `Box<PanelSlot>` (see `palette_native::windows::create_panel`), and the
// handler is a `Box<CallbackStorage>`. In both cases every access path goes
// through `Mutex::lock()` (or the macOS main-thread dispatch), so cross-thread
// access is serialized and no concurrent mutation can occur. The raw pointers
// themselves have no interior mutability that would race.
unsafe impl Send for PaletteNativeWindow {}

/// Build the dev server URL for the palette WKWebView.
///
/// Only used in debug mode. Production loads via `loadHTMLString:baseURL:`.
pub fn palette_dev_url() -> String {
    "http://localhost:1420/?window=prompt-palette".to_string()
}

/// Callback invoked when JavaScript posts a message to `busytokPanelBridge`.
pub type MessageCallback = Box<dyn Fn(&str) + Send>;

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub use macos::{
    cleanup_handler, create_message_handler, create_panel, destroy_panel, eval_js, hide_panel,
    palette_collection_behavior, palette_style_mask, palette_webview_draws_background, show_panel,
    CAN_JOIN_ALL_SPACES, FULL_SCREEN_AUXILIARY, IGNORES_CYCLE, MOVE_TO_ACTIVE_SPACE,
    NS_FULL_SIZE_CONTENT_VIEW_MASK, NS_NONACTIVATING_PANEL_MASK, NS_TITLED_WINDOW_MASK,
};

#[cfg(target_os = "windows")]
pub use windows::{
    cleanup_handler, create_message_handler, create_panel, destroy_panel, eval_js, hide_panel,
    show_panel,
};
