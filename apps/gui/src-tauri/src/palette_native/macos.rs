//! macOS-native NSPanel + WKWebView creation via objc FFI.
//!
//! This module provides a floating NSPanel hosting a WKWebView that serves as
//! the Prompt Palette UI. A custom ObjC class `BusytokPanelMessageHandler`
//! implements the WKScriptMessageHandler protocol to relay JavaScript messages
//! back to Rust via a stored callback.

use std::ffi::c_void;
use std::os::raw::c_long;
use std::ptr;
use std::sync::Once;

use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

// ---------------------------------------------------------------------------
// macOS-specific constants
// ---------------------------------------------------------------------------

pub const NS_TITLED_WINDOW_MASK: c_long = 1 << 0;
pub const NS_FULL_SIZE_CONTENT_VIEW_MASK: c_long = 1 << 15;
pub const NS_NONACTIVATING_PANEL_MASK: c_long = 1 << 7;

// Exposed for the "not present in collection behavior" assertion in tests.
pub const CAN_JOIN_ALL_SPACES: u64 = 1 << 0;
pub const MOVE_TO_ACTIVE_SPACE: u64 = 1 << 1;
pub const IGNORES_CYCLE: u64 = 1 << 6;
pub const FULL_SCREEN_AUXILIARY: u64 = 1 << 8;

pub fn palette_style_mask() -> c_long {
    NS_TITLED_WINDOW_MASK | NS_FULL_SIZE_CONTENT_VIEW_MASK | NS_NONACTIVATING_PANEL_MASK
}

pub fn palette_collection_behavior() -> u64 {
    MOVE_TO_ACTIVE_SPACE | FULL_SCREEN_AUXILIARY | IGNORES_CYCLE
}

/// NSBackingStoreBuffered = 2
const NS_BACKING_STORE_BUFFERED: c_long = 2;
/// NSFloatingWindowLevel = 3
const NS_FLOATING_WINDOW_LEVEL: i32 = 3;
/// NSWindowTitleHidden = 1
const NS_WINDOW_TITLE_HIDDEN: c_long = 1;
/// NSWindowCloseButton / MiniaturizeButton / ZoomButton
const NS_WINDOW_CLOSE_BUTTON: u64 = 0;
const NS_WINDOW_MINIATURIZE_BUTTON: u64 = 1;
const NS_WINDOW_ZOOM_BUTTON: u64 = 2;
/// NSViewWidthSizable | NSViewHeightSizable = 2 | 16 = 18
const AUTO_RESIZING_MASK: u64 = 18;
const PALETTE_WEBVIEW_DRAWS_BACKGROUND: bool = false;

pub fn palette_webview_draws_background() -> bool {
    PALETTE_WEBVIEW_DRAWS_BACKGROUND
}

// -----------------------------------------------------------------------
// Bridge bootstrap script
// -----------------------------------------------------------------------

/// JavaScript injected into the WKWebView at document-start.
///
/// Defines `window.busytokPanelBridge` (invoke/subscribe) and
/// `window.__busytokPanelBridgeDispatch` (native->JS event router).
/// This closes the bridge chain: JS calls invoke() -> postMessage ->
/// ObjC handler -> Rust -> evaluateJavaScript -> __busytokPanelBridgeDispatch
/// -> Promise resolve / subscriber callback.
///
/// The source is minified inline to minimise the injection payload.  When
/// modifying, expand it into a readable form first, edit, then recompress.
/// A human-readable version lives alongside this file as
/// `bridge_bootstrap.js` (not loaded at runtime).
const BRIDGE_BOOTSTRAP_JS: &str = r#"(function(){
var _r={},_s={},_n=0;
function _g(){return "r"+(++_n)+"_"+Date.now();}
function _post(m,d){
  try{
    window.webkit.messageHandlers.busytokPanelBridge.postMessage(
      JSON.stringify({id:_g(),type:"invoke",method:m,payload:d||null}));
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
        window.webkit.messageHandlers.busytokPanelBridge.postMessage(
          JSON.stringify({id:id,type:"invoke",method:m,payload:d||null}));
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

// -----------------------------------------------------------------------
// NSGeometry helpers
// -----------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct NSPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct NSSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct NSRect {
    pub origin: NSPoint,
    pub size: NSSize,
}

// -----------------------------------------------------------------------
// Placement policy (pure — unit-tested without FFI)
// -----------------------------------------------------------------------

/// One display's geometry, in AppKit's global bottom-left coordinate space.
///
/// `frame` is the full screen rectangle; `visible_frame` excludes the menu
/// bar and Dock. Both matter for palette placement: screen SELECTION tests
/// `frame` (a mouse over the menu bar still belongs to that screen), but
/// CENTERING uses `visible_frame` so the palette never lands under the menu
/// bar or Dock.
#[derive(Clone, Copy, Debug)]
pub struct ScreenGeometry {
    pub frame: NSRect,
    pub visible_frame: NSRect,
}

/// Index of the screen whose `frame` contains `mouse`, or `None` if the
/// mouse is not over any screen (e.g. it moved off-host between events).
///
/// Boundary semantics: inclusive on the low edge, exclusive on the high
/// edge, so two abutting screens never both claim the shared edge.
pub fn screen_index_containing(mouse: NSPoint, screens: &[ScreenGeometry]) -> Option<usize> {
    screens.iter().position(|s| point_in_rect(s.frame, mouse))
}

/// Resolve the panel's target frame for a mouse location and set of screens.
///
/// Policy: place on the screen whose `frame` contains the mouse; if none,
/// fall back to the first screen (`NSScreen.screens[0]` is the main screen
/// per Apple) so the palette still appears somewhere sensible. The panel is
/// centered within the chosen screen's `visible_frame`. Pure — no FFI — so
/// the entire "follow the mouse" placement policy is unit-testable in
/// isolation, independent of the AppKit calls that feed it data.
pub fn resolve_panel_frame(
    mouse: NSPoint,
    screens: &[ScreenGeometry],
    panel_size: NSSize,
) -> NSRect {
    let visible = match screen_index_containing(mouse, screens) {
        Some(i) => screens[i].visible_frame,
        None => screens
            .first()
            .map(|s| s.visible_frame)
            .unwrap_or_default(),
    };
    NSRect {
        origin: centered_origin(visible, panel_size),
        size: panel_size,
    }
}

/// Bottom-left-origin containment test.
fn point_in_rect(rect: NSRect, p: NSPoint) -> bool {
    p.x >= rect.origin.x
        && p.x < rect.origin.x + rect.size.width
        && p.y >= rect.origin.y
        && p.y < rect.origin.y + rect.size.height
}

/// Center `panel_size` within `visible`. Clamps so a panel larger than the
/// visible frame never starts above/left of the visible origin.
fn centered_origin(visible: NSRect, panel_size: NSSize) -> NSPoint {
    NSPoint {
        x: visible.origin.x + ((visible.size.width - panel_size.width) / 2.0).max(0.0),
        y: visible.origin.y + ((visible.size.height - panel_size.height) / 2.0).max(0.0),
    }
}

// -----------------------------------------------------------------------
// Callback type
// -----------------------------------------------------------------------

/// Concrete wrapper so we can store `MessageCallback` (which is a fat
/// `Box<dyn Fn>` pointer) in an ObjC ivar that only accepts thin pointers.
struct CallbackStorage {
    callback: super::MessageCallback,
}

// -----------------------------------------------------------------------
// ObjC class registration
// -----------------------------------------------------------------------

static REGISTER_HANDLER: Once = Once::new();

/// Register the `BusytokPanelMessageHandler` ObjC class exactly once.
///
/// The class is a subclass of `NSObject` with one ivar (`_callback`) that
/// holds a raw pointer to a boxed `MessageCallback`. It implements the
/// `WKScriptMessageHandler` protocol method
/// `userContentController:didReceiveScriptMessage:`.
pub fn register_handler_class() {
    REGISTER_HANDLER.call_once(|| {
        let mut decl = ClassDecl::new("BusytokPanelMessageHandler", class!(NSObject))
            .expect("failed to declare BusytokPanelMessageHandler class");

        // Ivar: raw pointer to the boxed callback
        decl.add_ivar::<*mut c_void>("_callback");

        // WKScriptMessageHandler protocol method
        unsafe {
            decl.add_method(
                sel!(userContentController:didReceiveScriptMessage:),
                handle_script_message as extern "C" fn(&Object, Sel, *mut Object, *mut Object),
            );
        }

        decl.register();
    });
}

/// The ObjC method implementation for `userContentController:didReceiveScriptMessage:`.
extern "C" fn handle_script_message(
    this: &Object,
    _sel: Sel,
    _controller: *mut Object,
    message: *mut Object,
) {
    unsafe {
        let body: *mut Object = msg_send![message, body];
        if body.is_null() {
            return;
        }

        // Extract the NSString contents
        let utf8_len: usize = msg_send![body, lengthOfBytesUsingEncoding: 4u64]; // NSUTF8StringEncoding = 4
        if utf8_len == 0 {
            // Could be an empty string or a non-string object. For empty
            // strings we still dispatch an empty string to the callback.
            let is_str: bool = msg_send![body, isKindOfClass: class!(NSString)];
            if !is_str {
                return;
            }
        }

        let c_str: *const i8 = msg_send![body, UTF8String];
        if c_str.is_null() {
            return;
        }
        let s = std::ffi::CStr::from_ptr(c_str);
        let message_text = s.to_string_lossy();

        // Retrieve the callback. MessageCallback = Box<dyn Fn> is a
        // fat pointer; ObjC ivars store thin pointers. We wrap it in
        // a concrete CallbackStorage struct so the ivar pointer is
        // always thin.
        let storage_ptr: *mut c_void = *this.get_ivar("_callback");
        if storage_ptr.is_null() {
            return;
        }
        let storage: &CallbackStorage = &*(storage_ptr as *const CallbackStorage);
        (storage.callback)(&message_text);
    }
}

// -----------------------------------------------------------------------
// Factory helpers
// -----------------------------------------------------------------------

/// Create an autoreleased NSString from a Rust `&str`.
pub fn ns_string(s: &str) -> *mut Object {
    unsafe {
        let bytes = s.as_ptr();
        let len = s.len();
        let ns_string: *mut Object = msg_send![class!(NSString), alloc];
        let ns_string: *mut Object = msg_send![ns_string,
            initWithBytes: bytes
            length: len
            encoding: 4u64 // NSUTF8StringEncoding
        ];
        let _: () = msg_send![ns_string, autorelease];
        ns_string
    }
}

/// Set a boolean KVC value on an ObjC object.
///
/// WebKit exposes some behavior used by wry/Tauri through KVC-only private
/// keys rather than public Objective-C setters. Calling those setters
/// directly raises NSInvalidArgumentException and aborts when it crosses
/// Rust FFI, so keep these calls on the KVC path.
unsafe fn set_bool_value_for_key(object: *mut Object, key: &str, value: bool) {
    let ns_value: *mut Object = msg_send![class!(NSNumber), numberWithBool: value];
    let ns_key = ns_string(key);
    let _: () = msg_send![object, setValue: ns_value forKey: ns_key];
}

unsafe fn responds_to_selector(object: *mut Object, selector: Sel) -> bool {
    msg_send![object, respondsToSelector: selector]
}

struct AutoreleasePool(*mut Object);

impl AutoreleasePool {
    unsafe fn new() -> Self {
        let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
        Self(pool)
    }
}

impl Drop for AutoreleasePool {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![self.0, drain];
        }
    }
}

/// Allocate and initialise a `BusytokPanelMessageHandler` instance that
/// stores the given callback.
pub fn create_message_handler(callback: super::MessageCallback) -> *mut Object {
    register_handler_class();

    unsafe {
        let cls = Class::get("BusytokPanelMessageHandler")
            .expect("BusytokPanelMessageHandler class not found");
        let obj: *mut Object = msg_send![cls, alloc];
        let obj: *mut Object = msg_send![obj, init];

        // MessageCallback = Box<dyn Fn> is a fat pointer. ObjC ivars
        // only hold thin pointers. Wrap in CallbackStorage so the ivar
        // stores a thin *mut CallbackStorage.
        let storage = CallbackStorage { callback };
        let boxed: *mut c_void = Box::into_raw(Box::new(storage)) as *mut c_void;
        (*obj).set_ivar("_callback", boxed);

        obj
    }
}

/// Gather the current mouse location and every screen's geometry from AppKit.
///
/// Main-thread only — reads `NSEvent.mouseLocation`. Returns `(mouse, screens)`
/// as plain data so the placement decision is delegated to the pure
/// [`resolve_panel_frame`], keeping AppKit calls here and policy testable.
unsafe fn gather_placement_context() -> (NSPoint, Vec<ScreenGeometry>) {
    let mouse: NSPoint = msg_send![class!(NSEvent), mouseLocation];
    let screens_array: *mut Object = msg_send![class!(NSScreen), screens];
    let count: usize = msg_send![screens_array, count];
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let screen: *mut Object = msg_send![screens_array, objectAtIndex: i];
        let frame: NSRect = msg_send![screen, frame];
        let visible_frame: NSRect = msg_send![screen, visibleFrame];
        out.push(ScreenGeometry { frame, visible_frame });
    }
    (mouse, out)
}

/// Create the NSPanel + WKWebView pair.
///
/// The panel is configured as a floating, non-activating, transparent
/// window. The WKWebView loads `url` and registers the
/// `busytokPanelBridge` script message handler.
pub fn create_panel(
    config: &super::PaletteNativeConfig,
    message_handler: *mut Object,
) -> (*mut Object, *mut Object) {
    unsafe {
        let _autorelease_pool = AutoreleasePool::new();
        tracing::debug!(
            event_code = "prompt_palette.native.create_panel.begin",
            width = config.width,
            height = config.height,
            is_debug = config.is_debug,
            webview_draws_background = PALETTE_WEBVIEW_DRAWS_BACKGROUND,
            "creating native prompt palette panel"
        );
        // ── NSPanel ──────────────────────────────────────────────

        // Place on the screen containing the mouse (launcher semantics),
        // centered in that screen's visible frame. This initial rect feeds
        // initWithContentRect; show_panel re-resolves on EVERY open so the
        // palette follows the current mouse screen rather than this one.
        let panel_size = NSSize {
            width: config.width as f64,
            height: config.height as f64,
        };
        let (mouse, screens) = gather_placement_context();
        let frame = resolve_panel_frame(mouse, &screens, panel_size);

        let style_mask: c_long = palette_style_mask();
        let panel: *mut Object = msg_send![class!(NSPanel), alloc];
        let panel: *mut Object = msg_send![panel,
            initWithContentRect: frame
            styleMask: style_mask
            backing: NS_BACKING_STORE_BUFFERED
            defer: false
        ];

        // Floating window level
        let _: () = msg_send![panel, setLevel: NS_FLOATING_WINDOW_LEVEL];

        // Collection behavior: appear in the active Space, remain usable
        // over fullscreen apps, and stay out of Cmd+` window cycling.
        let behavior: u64 = palette_collection_behavior();
        let _: () = msg_send![panel, setCollectionBehavior: behavior];

        // Use a titled/full-size-content panel with an invisible titlebar.
        // AppKit documents that FullSizeContentView opts into layer backing,
        // which gives WKWebView a more stable rendering host than a purely
        // borderless non-activating panel.
        let _: () = msg_send![panel, setTitleVisibility: NS_WINDOW_TITLE_HIDDEN];
        let _: () = msg_send![panel, setTitlebarAppearsTransparent: true];
        let _: () = msg_send![panel, setMovableByWindowBackground: true];
        hide_standard_window_button(panel, NS_WINDOW_CLOSE_BUTTON);
        hide_standard_window_button(panel, NS_WINDOW_MINIATURIZE_BUTTON);
        hide_standard_window_button(panel, NS_WINDOW_ZOOM_BUTTON);

        // Keep the panel itself visually neutral; the full-size WKWebView
        // provides the visible backing. A fully transparent panel +
        // transparent webview can successfully run JS while remaining
        // invisible to the user.
        let clear_color: *mut Object = msg_send![class!(NSColor), clearColor];
        let _: () = msg_send![panel, setBackgroundColor: clear_color];

        // Panel-specific configuration
        let _: () = msg_send![panel, setHidesOnDeactivate: false];
        let _: () = msg_send![panel, setBecomesKeyOnlyIfNeeded: false];
        let _: () = msg_send![panel, setHasShadow: true];

        // Opaque = false for transparency
        let _: () = msg_send![panel, setOpaque: false];

        // ── WKWebView ────────────────────────────────────────────
        let wkwebview_config: *mut Object = msg_send![class!(WKWebViewConfiguration), new];
        set_bool_value_for_key(
            wkwebview_config,
            "drawsBackground",
            PALETTE_WEBVIEW_DRAWS_BACKGROUND,
        );

        // WKWebView blocks JS modules loaded from file:// base URLs
        // by default. The native palette uses loadHTMLString:baseURL:
        // with a file:// base URL; enable file access so the React
        // bundle's <script type="module"> can execute.
        let prefs: *mut Object = msg_send![wkwebview_config, preferences];
        set_bool_value_for_key(prefs, "allowFileAccessFromFileURLs", true);

        // Register script message handler BEFORE creating the webview
        let user_content: *mut Object = msg_send![wkwebview_config, userContentController];
        let handler_name = ns_string("busytokPanelBridge");
        let _: () = msg_send![user_content,
            addScriptMessageHandler: message_handler
            name: handler_name
        ];

        // Inject bridge bootstrap JS at document-start so
        // window.busytokPanelBridge and __busytokPanelBridgeDispatch
        // are available before any page JS runs.
        let bootstrap_script = ns_string(BRIDGE_BOOTSTRAP_JS);
        // WKUserScriptInjectionTimeAtDocumentStart = 0
        let user_script: *mut Object = msg_send![class!(WKUserScript), alloc];
        let user_script: *mut Object = msg_send![user_script,
            initWithSource: bootstrap_script
            injectionTime: 0u64
            forMainFrameOnly: true
        ];
        let _: () = msg_send![user_content, addUserScript: user_script];

        // Create the webview
        let webview_frame = NSRect {
            origin: NSPoint { x: 0.0, y: 0.0 },
            size: NSSize {
                width: config.width as f64,
                height: config.height as f64,
            },
        };
        let webview: *mut Object = msg_send![class!(WKWebView), alloc];
        let webview: *mut Object = msg_send![webview,
            initWithFrame: webview_frame
            configuration: wkwebview_config
        ];

        // Keep WebKit transparent; the React surface draws the visible
        // card. If WKWebView draws its own backing, users see a second
        // full-window white rectangle behind the palette content.
        set_bool_value_for_key(webview, "drawsBackground", PALETTE_WEBVIEW_DRAWS_BACKGROUND);

        // Developer extras in debug mode. `setInspectable:` is public on
        // newer macOS versions; `developerExtrasEnabled` is KVC-only.
        #[cfg(debug_assertions)]
        {
            if responds_to_selector(webview, sel!(setInspectable:)) {
                let _: () = msg_send![webview, setInspectable: true];
            } else {
                tracing::debug!(
                    event_code = "prompt_palette.native.inspectable_unavailable",
                    "WKWebView setInspectable selector is unavailable"
                );
            }

            let prefs: *mut Object = msg_send![wkwebview_config, preferences];
            set_bool_value_for_key(prefs, "developerExtrasEnabled", true);
        }

        // Auto-resize with panel
        let _: () = msg_send![webview, setAutoresizingMask: AUTO_RESIZING_MASK];

        // Set webview as panel content view
        let _: () = msg_send![panel, setContentView: webview];

        // ── Load URL ─────────────────────────────────────────────
        match super::palette_load_strategy(config.is_debug) {
            // Dev: load from dev server via HTTP
            super::PaletteLoadStrategy::DevServerRequest => {
                let palette_url = super::palette_dev_url();
                tracing::info!(
                    event_code = "prompt_palette.native.load_url",
                    mode = "debug",
                    url = %palette_url,
                    style_mask = style_mask,
                    collection_behavior = behavior,
                    "loading native prompt palette webview URL"
                );
                let url_str = ns_string(&palette_url);
                let ns_url: *mut Object = msg_send![class!(NSURL), URLWithString: url_str];
                let request: *mut Object =
                    msg_send![class!(NSURLRequest), requestWithURL: ns_url];
                let _: () = msg_send![webview, loadRequest: request];
            }
            super::PaletteLoadStrategy::LocalHtmlStringWithBaseUrl => {
                // Local bundle: read index.html and load it with a file://
                // base URL. Direct file URL loading can leave module scripts
                // blocked in WKWebView, producing a white panel with an empty
                // #root while document-start bridge scripts still run.
                let bundle_dir = config.bundle_resource_dir.trim_end_matches('/');
                let html_path = super::palette_index_html_path(bundle_dir);

                let html = match std::fs::read_to_string(&html_path) {
                    Ok(html) => html,
                    Err(error) => {
                        tracing::error!(
                            event_code = "prompt_palette.native.index_html_read_failed",
                            path = %html_path.display(),
                            error = %error,
                            "failed to read prompt palette index.html"
                        );
                        "<!doctype html><html><body>Prompt Palette failed to load.</body></html>"
                        .to_string()
                    }
                };
                let base_url_str = format!("file://{bundle_dir}/");
                tracing::info!(
                    event_code = "prompt_palette.native.load_url",
                    mode = "local_html_string",
                    index_html = %html_path.display(),
                    base_url = %base_url_str,
                    html_bytes = html.len(),
                    style_mask = style_mask,
                    collection_behavior = behavior,
                    "loading native prompt palette webview HTML string with base URL"
                );

                let html_ns = ns_string(&html);
                let base_url_ns = ns_string(&base_url_str);
                let base_url: *mut Object =
                    msg_send![class!(NSURL), URLWithString: base_url_ns];
                let _: () = msg_send![webview,
                    loadHTMLString: html_ns
                    baseURL: base_url
                ];
            }
        }

        tracing::debug!(
            event_code = "prompt_palette.native.create_panel.end",
            "native prompt palette panel created"
        );
        (panel, webview)
    }
}

fn hide_standard_window_button(panel: *mut Object, button_kind: u64) {
    unsafe {
        let button: *mut Object = msg_send![panel, standardWindowButton: button_kind];
        if !button.is_null() {
            let _: () = msg_send![button, setHidden: true];
        }
    }
}

/// Show the panel on screen.
///
/// Repositions to the current mouse screen on EVERY show — not just the
/// first create — so a launcher palette follows where the user actually is,
/// matching Raycast/Alfred/Spotlight expectations. Without this, the panel
/// would forever reopen on whatever screen it was first created on, because
/// `orderFront` alone reuses the baked-in frame.
pub fn show_panel(panel: *mut Object, webview: *mut Object) {
    unsafe {
        let _pool = AutoreleasePool::new();

        // Re-resolve placement against the live mouse location, then move the
        // panel (origin only — never resize). Reads the panel's own size so a
        // future size change is never clobbered by this reposition.
        let current: NSRect = msg_send![panel, frame];
        let (mouse, screens) = gather_placement_context();
        let target = resolve_panel_frame(mouse, &screens, current.size);
        let _: () = msg_send![panel, setFrameOrigin: target.origin];

        let source = match screen_index_containing(mouse, &screens) {
            Some(i) => format!("mouse_screen[{i}]"),
            None => "fallback_main".to_string(),
        };
        tracing::info!(
            event_code = "prompt_palette.native.reposition",
            source = %source,
            mouse_x = mouse.x,
            mouse_y = mouse.y,
            screen_count = screens.len(),
            origin_x = target.origin.x,
            origin_y = target.origin.y,
            "repositioned palette panel to current mouse screen"
        );

        let _: () = msg_send![panel, orderFrontRegardless];
        let _: () = msg_send![panel, makeKeyWindow];
        let _: () = msg_send![panel, makeFirstResponder: webview];
    }
}

/// Hide the panel off screen.
pub fn hide_panel(panel: *mut Object) {
    unsafe {
        let _: () = msg_send![panel, orderOut: ptr::null::<c_void>()];
    }
}

/// Close and release the panel.
pub fn destroy_panel(panel: *mut Object) {
    unsafe {
        let _: () = msg_send![panel, close];
    }
}

/// Free the boxed `MessageCallback` stored inside a handler object.
///
/// Call this before or after `destroy_panel` to reclaim the callback's
/// heap allocation. Passing a null pointer is safe (no-op).
///
/// Acceptable leak: if the process crashes or is force-killed (SIGTERM,
/// SIGKILL), `destroy()` is never called and the Box is leaked. This is
/// acceptable because the OS reclaims all memory on process exit and the
/// alternative (at-exit handlers) adds complexity for no practical gain.
pub fn cleanup_handler(handler: *mut Object) {
    if handler.is_null() {
        return;
    }
    unsafe {
        let callback_ptr: *mut c_void = *(*handler).get_ivar::<*mut c_void>("_callback");
        if !callback_ptr.is_null() {
            // Free the CallbackStorage (thin pointer), which drops the
            // inner MessageCallback closure.
            let _: Box<CallbackStorage> = Box::from_raw(callback_ptr as *mut CallbackStorage);
        }
    }
}

/// Evaluate a JavaScript string in the webview.
pub fn eval_js(webview: *mut Object, script: &str) {
    unsafe {
        let _pool = AutoreleasePool::new();
        let ns_script = ns_string(script);
        let _: () = msg_send![webview,
            evaluateJavaScript: ns_script
            completionHandler: ptr::null::<c_void>()
        ];
    }
}

// ---------------------------------------------------------------------------
// Framework link hints
// ---------------------------------------------------------------------------

#[link(name = "WebKit", kind = "framework")]
#[link(name = "AppKit", kind = "framework")]
extern "C" {}
