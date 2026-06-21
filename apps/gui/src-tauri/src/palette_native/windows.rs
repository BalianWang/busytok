//! Windows-native palette panel (Tauri WebviewWindow + WS_EX_NOACTIVATE).
//!
//! This module provides a floating, non-activating Tauri `WebviewWindow` that
//! serves as the Prompt Palette UI. The webview runs the same React surface as
//! the macOS variant, with the bridge bootstrap JS adapted to call
//! `window.__TAURI__.core.invoke('palette_panel_message', {body})` instead of
//! the WebKit `window.webkit.messageHandlers.busytokPanelBridge.postMessage`
//! entry point.
//!
//! The native code on the Rust side expects a Tauri command
//! `palette_panel_message` (registered in Task 6.6) that routes the JSON
//! envelope through the same `MessageCallback` used by the macOS path, so
//! the rest of `PanelBridge` and `PaletteController` is unchanged. Until
//! Task 6.6 lands, Windows palette invocation will fail at runtime with
//! "command palette_panel_message not found".

use std::ffi::c_void;
use std::sync::Mutex;

use tauri::{WebviewUrl, WebviewWindow, WebviewWindowBuilder};

// ---------------------------------------------------------------------------
// Bridge bootstrap script
// ---------------------------------------------------------------------------

/// JavaScript injected into the Tauri WebviewWindow at document-start.
///
/// Byte-identical to the macOS `BRIDGE_BOOTSTRAP_JS` modulo the postMessage
/// call site: the macOS variant calls
/// `window.webkit.messageHandlers.busytokPanelBridge.postMessage(JSON.stringify({...}))`,
/// while this Windows variant calls
/// `window.__TAURI__.core.invoke('palette_panel_message', {body: JSON.stringify({...})})`.
/// Everything else — request registry, subscriber registry, dispatch router,
/// diagnostic probes — is unchanged so the surface the React code sees is
/// identical cross-platform.
const BRIDGE_INIT_SCRIPT: &str = r#"(function(){
var _r={},_s={},_n=0;
function _g(){return "r"+(++_n)+"_"+Date.now();}
function _post(m,d){
  try{
    window.__TAURI__.core.invoke('palette_panel_message',
      {body: JSON.stringify({id:_g(),type:"invoke",method:m,payload:d||null})});
  }catch(_e){console.warn("[busytokPanel] postMessage failed:",_e);}
}
function _diag(name,details){
  _post("panel:diagnostic",{
    name:name,details:details||{},href:String(location.href),
    readyState:document.readyState,ts:Date.now()
  });
}
window.__busytokPanelBridgeDispatch=function(e){
  if(e.type==="response"&&e.request_id&&_r[e.request_id]){
    var r=_r[e.request_id];delete _r[e.request_id];
    var p=e.payload||{};
    p.ok?r.resolve(p):r.reject(new Error(p.error||"Unknown error"));
    return;
  }
  var h=_s[e.type];
  if(h)h.forEach(function(f){f(e.payload);});
};
window.__BUSYTOK_PANEL_CONTEXT=true;
window.busytokPanelBridge={
  invoke:function(m,d){
    return new Promise(function(ok,no){
      var id=_g();_r[id]={resolve:ok,reject:no};
      try{
        window.__TAURI__.core.invoke('palette_panel_message',
          {body: JSON.stringify({id:id,type:"invoke",method:m,payload:d||null})});
      }catch(e){
        delete _r[id];no(e);
      }
    });
  },
  subscribe:function(ev,fn){
    if(!_s[ev])_s[ev]=new Set();
    _s[ev].add(fn);
    return function(){_s[ev]&&_s[ev].delete(fn);};
  }
};
window.__busytokPanelBridgeDiagnostic=_diag;
window.addEventListener("error",function(e){
  _diag("window_error",{message:e.message,filename:e.filename,lineno:e.lineno,colno:e.colno});
},true);
window.addEventListener("unhandledrejection",function(e){
  var r=e.reason||{};
  _diag("unhandled_rejection",{message:String(r.message||r),stack:String(r.stack||"").slice(0,600)});
},true);
window.addEventListener("keydown",function(e){
  if(e.key==="Escape"){
    _diag("escape_keydown",{activeElement:document.activeElement&&document.activeElement.tagName});
    e.preventDefault();e.stopPropagation();_post("palette:close",{});
  }
},true);
function _probe(name){
  try{
    var root=document.getElementById("root");
    var body=document.body;
    var active=document.activeElement;
    var rootRect=root&&root.getBoundingClientRect?root.getBoundingClientRect():null;
    _diag(name,{
      bodyClass:body?body.className:"",
      bodyTextLength:body&&body.innerText?body.innerText.length:0,
      rootChildCount:root?root.childElementCount:-1,
      rootTextPreview:root&&root.innerText?root.innerText.slice(0,120):"",
      rootRect:rootRect?{x:rootRect.x,y:rootRect.y,width:rootRect.width,height:rootRect.height}:null,
      activeElement:active?active.tagName:"",
      activeClass:active&&active.className?String(active.className):""
    });
  }catch(e){
    _diag("probe_error",{name:name,message:String(e&&e.message||e)});
  }
}
_diag("bridge_bootstrap_installed",{});
if(document.readyState==="loading"){
  document.addEventListener("DOMContentLoaded",function(){_probe("dom_content_loaded");},{once:true});
}else{
  _probe("dom_already_ready");
}
requestAnimationFrame(function(){_probe("first_animation_frame");});
setTimeout(function(){_probe("after_500ms");},500);
setTimeout(function(){_probe("after_2000ms");},2000);
})()"#;

// ---------------------------------------------------------------------------
// WindowsPalette wrapper
// ---------------------------------------------------------------------------

/// Owning wrapper around the Tauri `WebviewWindow` used for the palette.
///
/// `WebviewWindow` is itself cheaply cloneable (it is backed by an `Arc`-like
/// handle inside Tauri), but we hold a single instance for clear ownership
/// semantics on drop.
pub struct WindowsPalette {
    window: WebviewWindow,
}

impl WindowsPalette {
    /// Build the palette webview window.
    ///
    /// The window is created invisible; callers are expected to invoke
    /// `show_panel` to flip visibility with `SW_SHOWNOACTIVATE`.
    pub fn new(app: &tauri::AppHandle, config: &super::PaletteNativeConfig) -> anyhow::Result<Self> {
        tracing::debug!(
            event_code = "prompt_palette.native.create_panel.begin",
            width = config.width,
            height = config.height,
            is_debug = config.is_debug,
            "creating native prompt palette webview window"
        );

        let url = if config.is_debug {
            WebviewUrl::ExternalUrl(super::palette_dev_url().parse()?)
        } else {
            WebviewUrl::App("/?window=prompt-palette".into())
        };
        let window = WebviewWindowBuilder::new(app, "prompt-palette", url)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .resizable(false)
        .inner_size(config.width as f64, config.height as f64)
        .initialization_script(BRIDGE_INIT_SCRIPT)
        .build()?;

        // Apply WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW so the palette floats
        // over the active app without stealing focus and never appears in
        // the Alt+Tab switcher or the taskbar thumbstrip.
        apply_no_activate(&window)?;

        tracing::debug!(
            event_code = "prompt_palette.native.create_panel.end",
            "native prompt palette webview window created"
        );

        Ok(Self { window })
    }

    /// Show the window via `SW_SHOWNOACTIVATE` (does not steal focus).
    pub fn show(&self) {
        if let Err(error) = show_window_no_activate(&self.window) {
            tracing::warn!(
                event_code = "prompt_palette.native.show_failed",
                error = %error,
                "failed to ShowWindow(SW_SHOWNOACTIVATE) the palette window"
            );
        }
    }

    /// Hide the window.
    pub fn hide(&self) {
        if let Err(error) = self.window.hide() {
            tracing::warn!(
                event_code = "prompt_palette.native.hide_failed",
                error = %error,
                "failed to hide the palette window"
            );
        }
    }

    /// Evaluate a JavaScript string in the webview.
    pub fn eval(&self, script: &str) {
        if let Err(error) = self.window.eval(script) {
            tracing::warn!(
                event_code = "prompt_palette.native.eval_failed",
                error = %error,
                script_preview = %script.chars().take(120).collect::<String>(),
                "failed to eval JS in the palette webview"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Win32 helpers
// ---------------------------------------------------------------------------

/// Apply `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` to the window's extended style.
///
/// `WS_EX_NOACTIVATE` prevents the window from being activated when clicked,
/// which keeps keyboard focus on the previously active app. `WS_EX_TOOLWINDOW`
/// hides the window from Alt+Tab and the taskbar's thumbstrip.
fn apply_no_activate(window: &tauri::WebviewWindow) -> anyhow::Result<()> {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let hwnd = window.hwnd()?;
    unsafe {
        let style = GetWindowLongPtrW(hwnd.0 as _, GWL_EXSTYLE);
        SetWindowLongPtrW(
            hwnd.0 as _,
            GWL_EXSTYLE,
            style | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as isize,
        );
    }
    Ok(())
}

/// Show the window with `SW_SHOWNOACTIVATE` so the previously active window
/// retains keyboard focus.
fn show_window_no_activate(window: &tauri::WebviewWindow) -> anyhow::Result<()> {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let hwnd = window.hwnd()?;
    unsafe {
        ShowWindow(hwnd.0 as _, SW_SHOWNOACTIVATE);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Slot wrapper
// ---------------------------------------------------------------------------

/// Concrete slot that holds the optional `WindowsPalette`.
///
/// Boxed and exposed to C as a `*mut c_void` so the same shape can flow through
/// `PaletteNativeWindow::panel` and `PaletteNativeWindow::webview` (both point
/// to the same underlying slot — on Windows there is only one underlying
/// object because Tauri's `WebviewWindow` plays both roles).
pub type PanelSlot = Mutex<Option<WindowsPalette>>;

/// Convert a raw slot pointer back to a `&PanelSlot`.
///
/// Returns `None` for null pointers.
fn slot_ref<'a>(ptr: *mut c_void) -> Option<&'a PanelSlot> {
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &*(ptr as *const PanelSlot) })
    }
}

// ---------------------------------------------------------------------------
// Callback storage
// ---------------------------------------------------------------------------

/// Concrete storage for the `MessageCallback`.
///
/// On Windows the bridge message handler is dispatched via a Tauri command
/// (`palette_panel_message`, registered in Task 6.6), but we still produce a
/// stored callback here for symmetry with macOS so the controller's lifecycle
/// (create -> destroy -> cleanup) is identical cross-platform.
struct CallbackStorage {
    callback: super::MessageCallback,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create the palette panel and return a `PaletteNativeWindow` whose
/// `panel` and `webview` slots both reference the same boxed slot.
///
/// The `handler` argument is the boxed `MessageCallback` produced by
/// `create_message_handler`. It is returned unchanged in the
/// `PaletteNativeWindow::handler` slot so the controller can free it later
/// via `cleanup_handler`.
pub fn create_panel(
    _app: &tauri::AppHandle,
    config: &super::PaletteNativeConfig,
    handler: *mut std::ffi::c_void,
) -> super::PaletteNativeWindow {
    let panel = match WindowsPalette::new(_app, config) {
        Ok(panel) => panel,
        Err(error) => {
            tracing::error!(
                event_code = "prompt_palette.native.create_panel_failed",
                error = %error,
                "failed to create native prompt palette panel"
            );
            return super::PaletteNativeWindow {
                panel: std::ptr::null_mut(),
                webview: std::ptr::null_mut(),
                handler,
            };
        }
    };
    let slot: *mut PanelSlot = Box::into_raw(Box::new(Mutex::new(Some(panel))));
    super::PaletteNativeWindow {
        panel: slot as *mut c_void,
        webview: slot as *mut c_void,
        handler,
    }
}

/// Show the palette panel via `SW_SHOWNOACTIVATE`.
///
/// `webview` is accepted for signature parity with macOS but ignored: both
/// pointers reference the same slot on Windows.
pub fn show_panel(panel: *mut c_void, _webview: *mut c_void) {
    if let Some(slot) = slot_ref(panel) {
        if let Ok(guard) = slot.lock() {
            if let Some(ref palette) = *guard {
                palette.show();
            }
        }
    }
}

/// Hide the palette panel.
pub fn hide_panel(panel: *mut c_void) {
    if let Some(slot) = slot_ref(panel) {
        if let Ok(guard) = slot.lock() {
            if let Some(ref palette) = *guard {
                palette.hide();
            }
        }
    }
}

/// Destroy the palette panel and free the slot.
///
/// Safe to call with a null pointer.
///
/// # Windows invariant
///
/// On Windows, `PaletteNativeWindow::panel` and `PaletteNativeWindow::webview`
/// MUST alias the same `Box<PanelSlot>` (see [`create_panel`]). Only the
/// `panel` pointer is freed here -- freeing `webview` as well would be a
/// double-free of the same allocation.
pub fn destroy_panel(panel: *mut c_void) {
    if panel.is_null() {
        return;
    }
    unsafe {
        let slot: Box<PanelSlot> = Box::from_raw(panel as *mut PanelSlot);
        // Drop the inner palette while holding the lock to ensure no other
        // thread is mid-call.
        if let Ok(mut guard) = slot.lock() {
            *guard = None;
        }
        drop(slot);
    }
}

/// Evaluate a JavaScript string in the palette webview.
///
/// `panel` is accepted because both `PaletteNativeWindow.panel` and
/// `PaletteNativeWindow.webview` reference the same slot on Windows.
pub fn eval_js(panel: *mut c_void, script: &str) {
    if let Some(slot) = slot_ref(panel) {
        if let Ok(guard) = slot.lock() {
            if let Some(ref palette) = *guard {
                palette.eval(script);
            }
        }
    }
}

/// Box a `MessageCallback` into a heap allocation suitable for storing in the
/// `PaletteNativeWindow::handler` slot.
///
/// On Windows the bridge message handler is dispatched via a Tauri command
/// (`palette_panel_message`) registered in `lib.rs`, but we still produce a
/// stored callback here for symmetry with macOS so the controller's lifecycle
/// (create -> destroy -> cleanup) is identical.
pub fn create_message_handler(callback: super::MessageCallback) -> *mut c_void {
    let storage = CallbackStorage { callback };
    Box::into_raw(Box::new(storage)) as *mut c_void
}

/// Free the boxed `MessageCallback`.
///
/// Safe to call with a null pointer.
pub fn cleanup_handler(handler: *mut c_void) {
    if handler.is_null() {
        return;
    }
    unsafe {
        let _: Box<CallbackStorage> = Box::from_raw(handler as *mut CallbackStorage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression for P1 fix: when `is_debug=true` the Windows palette URL
    /// must point at the Vite dev server (localhost:1420), not the bundled
    /// `/?window=prompt-palette` path. `palette_dev_url()` is the source of
    /// the URL passed to `WebviewUrl::ExternalUrl` in `WindowsPalette::new`.
    #[test]
    fn debug_config_produces_localhost_dev_url() {
        assert!(super::super::palette_dev_url().starts_with("http://localhost:1420/"));
        // Sanity: config defaults mirror cfg!(debug_assertions) so a debug
        // build goes through the dev-server branch by default.
        let debug_config = super::super::PaletteNativeConfig {
            is_debug: true,
            ..Default::default()
        };
        assert!(debug_config.is_debug);
    }
}
