//! macOS ServiceManagement (`SMAppService`) bridge.
//!
//! This module is the single place in the GUI binary that talks to the
//! ServiceManagement framework. Higher-level lifecycle code (Task 3's
//! `SmAppServiceLifecycle` and Task 5's coordinator) calls into this bridge
//! instead of reaching for `SMAppService` directly.
//!
//! ## Threading contract
//!
//! `SMAppService` is main-thread-only. Every ObjC call here
//! (`status`, `register`, `unregister`) runtime-asserts that it is running on
//! the main thread via `pthread_main_np()`. Callers that are not already on
//! the main thread must route through the [`MainThreadExecutor`] trait using
//! the `_with_executor` method variants, which defer execution to whatever
//! executor the caller supplies (production = Tauri's
//! `AppHandle::run_on_main_thread`; tests = a recording fake).
//!
//! ## Stale-bundle detection
//!
//! `SMAppService.status` does NOT expose the registered executable path, so
//! detecting a stale registration after an app move requires parsing the
//! launchd state. That parser lives in the sibling
//! [`crate::service_lifecycle::launchd_job_snapshot`] module — it is a pure
//! text parser with no FFI surface.

use anyhow::{anyhow, Result};
#[cfg(target_os = "macos")]
use anyhow::Context;

use super::LifecycleStatus;

/// Native-facing status enum that mirrors `SMAppService.status` without
/// pulling the framework's enum into the rest of the codebase.
///
/// Variants follow the meaning documented at
/// <https://developer.apple.com/documentation/servicemanagement/smappservice/status>.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SMServiceStatus {
    /// `SMAppServiceStatus.notRegistered` (value 0). The agent is not
    /// registered with ServiceManagement.
    NotRegistered,
    /// `SMAppServiceStatus.enabled` (value 1). Registered and (the framework
    /// believes) running.
    Enabled,
    /// `SMAppServiceStatus.requiresApproval` (value 2). Registered but
    /// requires user approval in System Settings > General > Login Items.
    RequiresApproval,
    /// `SMAppServiceStatus.notFound` (value 3). The registered plist was
    /// removed from disk out from under ServiceManagement.
    NotFound,
    /// `SMAppServiceStatus.enabledNotRunning` (value 4). Registered and
    /// approved but the helper is currently not running (it crashed or was
    /// killed).
    EnabledNotRunning,
}

impl SMServiceStatus {
    /// Map the native ServiceManagement status into the platform-agnostic
    /// [`LifecycleStatus`] used by the rest of the lifecycle port.
    ///
    /// Mapping policy:
    /// - `NotRegistered` / `NotFound` -> `NotRegistered` (caller should call
    ///   `register`).
    /// - `EnabledNotRunning` -> `RegisteredInactive`.
    /// - `Enabled` -> `Running` (ServiceManagement claims the helper is up).
    /// - `RequiresApproval` -> `NeedsAttention` (user must enable it).
    pub fn to_lifecycle_status(self) -> LifecycleStatus {
        match self {
            SMServiceStatus::NotRegistered | SMServiceStatus::NotFound => {
                LifecycleStatus::NotRegistered
            }
            SMServiceStatus::EnabledNotRunning => LifecycleStatus::RegisteredInactive,
            SMServiceStatus::Enabled => LifecycleStatus::Running,
            SMServiceStatus::RequiresApproval => LifecycleStatus::NeedsAttention,
        }
    }
}

impl From<SMServiceStatus> for LifecycleStatus {
    fn from(value: SMServiceStatus) -> Self {
        value.to_lifecycle_status()
    }
}

/// Executor abstraction that lets callers route ServiceManagement calls onto
/// the main thread without this module depending on Tauri directly.
///
/// Production implementations wrap
/// `tauri::AppHandle::run_on_main_thread`. Test implementations record the
/// closure and (optionally) execute it synchronously.
///
/// The closure handed to [`MainThreadExecutor::run_on_main_thread`] may
/// either fire synchronously (tests) or be queued for the framework's run
/// loop (production). Callers must therefore not rely on side effects being
/// visible immediately after the call returns — the `_with_executor` bridge
/// methods hide this by blocking on a oneshot channel.
pub trait MainThreadExecutor: Send + Sync {
    /// Schedule `f` to run on the main thread.
    ///
    /// Implementations must eventually invoke `f` exactly once. Whether that
    /// happens before this method returns is implementation-defined; the
    /// bridge's `_with_executor` helpers internally synchronise on the
    /// result.
    fn run_on_main_thread(&self, f: Box<dyn FnOnce() + Send>);
}

/// Identifies which kind of `SMAppService` a handle refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SMAppServiceKind {
    /// Registered via `+agentWithPlistName:` against the launch-agent plist
    /// bundled inside the app.
    Agent { plist_name: &'static str },
    /// The `+mainApp` service: the GUI app itself registered as a login item.
    MainApp,
}

/// Thin value type holding the inputs the FFI layer needs. Construct via
/// [`SMAppServiceHandle::agent`] / [`SMAppServiceHandle::main_app`].
///
/// All ObjC-touching methods are documented and runtime-asserted as
/// main-thread-only.
#[derive(Debug, Clone, Copy)]
pub struct SMAppServiceHandle {
    kind: SMAppServiceKind,
}

impl SMAppServiceHandle {
    /// Build a handle for the bundled LaunchAgent service identified by its
    /// plist filename, e.g. `com.busytok.service.plist`.
    pub fn agent(plist_name: &'static str) -> Self {
        Self {
            kind: SMAppServiceKind::Agent { plist_name },
        }
    }

    /// Build a handle for `+mainApp` (the GUI registered as a login item).
    pub fn main_app() -> Self {
        Self {
            kind: SMAppServiceKind::MainApp,
        }
    }

    /// Introspection helper: which kind of service is this handle bound to?
    pub fn kind(&self) -> SMAppServiceKind {
        self.kind
    }

    /// Constant flag advertised so the test layer (and future callers) can
    /// branch on the threading model without resorting to `cfg`.
    ///
    /// Always `true` on macOS (where the FFI is real), always `false`
    /// elsewhere (where the bridge is stubbed out).
    pub fn main_thread_only() -> bool {
        cfg!(target_os = "macos")
    }

    /// Query the current ServiceManagement status.
    ///
    /// # Main-thread-only
    ///
    /// `SMAppService.status` must be called from the main thread. This
    /// method runtime-asserts that condition. Off-main-thread callers must
    /// use [`SMAppServiceHandle::status_with_executor`].
    pub fn status(&self) -> Result<SMServiceStatus> {
        #[cfg(target_os = "macos")]
        {
            assert_main_thread("status");
            self.status_unchecked()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = self;
            bail_non_macos("status")
        }
    }

    /// Register the service with ServiceManagement.
    ///
    /// # Main-thread-only
    ///
    /// `registerAndReturnError:` must be called from the main thread. This
    /// method runtime-asserts that condition. Off-main-thread callers must
    /// use [`SMAppServiceHandle::register_with_executor`].
    pub fn register(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            assert_main_thread("register");
            self.register_unchecked()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = self;
            bail_non_macos("register")
        }
    }

    /// Unregister the service from ServiceManagement.
    ///
    /// # Main-thread-only
    ///
    /// `unregisterAndReturnError:` must be called from the main thread. This
    /// method runtime-asserts that condition. Off-main-thread callers must
    /// use [`SMAppServiceHandle::unregister_with_executor`].
    pub fn unregister(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            assert_main_thread("unregister");
            self.unregister_unchecked()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = self;
            bail_non_macos("unregister")
        }
    }

    /// Executor-routed variant of [`Self::status`].
    ///
    /// Schedules the call on `executor`'s main thread and blocks until the
    /// result is available. Safe to call from any thread.
    pub fn status_with_executor(&self, executor: &dyn MainThreadExecutor) -> Result<SMServiceStatus> {
        // `SMAppServiceHandle` is `Copy`, so capturing `*self` by value keeps
        // the closure `'static` even though `executor` is borrowed.
        let this = *self;
        run_via_executor(executor, move || this.status())
    }

    /// Executor-routed variant of [`Self::register`].
    pub fn register_with_executor(&self, executor: &dyn MainThreadExecutor) -> Result<()> {
        let this = *self;
        run_via_executor(executor, move || this.register())
    }

    /// Executor-routed variant of [`Self::unregister`].
    pub fn unregister_with_executor(&self, executor: &dyn MainThreadExecutor) -> Result<()> {
        let this = *self;
        run_via_executor(executor, move || this.unregister())
    }

    // ── FFI layer ──────────────────────────────────────────────────

    #[cfg(target_os = "macos")]
    fn status_unchecked(&self) -> Result<SMServiceStatus> {
        ffi::status(self.kind).context("querying SMAppService.status")
    }

    #[cfg(target_os = "macos")]
    fn register_unchecked(&self) -> Result<()> {
        ffi::register(self.kind).context("calling SMAppService.registerAndReturnError:")
    }

    #[cfg(target_os = "macos")]
    fn unregister_unchecked(&self) -> Result<()> {
        ffi::unregister(self.kind).context("calling SMAppService.unregisterAndReturnError:")
    }
}

/// Helper for the `_with_executor` variants: run `op` on the main thread
/// via `executor` and return its result through a oneshot channel so the
/// caller can synchronise.
fn run_via_executor<T: Send + 'static>(
    executor: &dyn MainThreadExecutor,
    op: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    use std::sync::mpsc::channel;

    // If we are already on the main thread, dispatching through the
    // executor (run_on_main_thread) and blocking on rx.recv() would
    // deadlock: the queued closure can never execute because the main
    // thread is blocked. During did_finish_launching and other macOS
    // app-delegate callbacks this is always true. Run synchronously.
    #[cfg(target_os = "macos")]
    {
        if (unsafe { libc::pthread_main_np() }) != 0 {
            return op();
        }
    }

    let (tx, rx) = channel();
    executor.run_on_main_thread(Box::new(move || {
        let result = op();
        // Best-effort: ignore send errors if the receiver was dropped.
        let _ = tx.send(result);
    }));

    rx.recv()
        .map_err(|_| anyhow!("main-thread executor dropped result channel before completion"))?
}

#[cfg(target_os = "macos")]
fn assert_main_thread(api: &'static str) {
    let on_main_thread = unsafe { libc::pthread_main_np() } != 0;
    assert!(
        on_main_thread,
        "SMAppServiceHandle::{api} must be called on the main thread; \
         route through the _with_executor variants from worker threads",
    );
}

#[cfg(not(target_os = "macos"))]
fn bail_non_macos<T>(api: &'static str) -> Result<T> {
    Err(anyhow!(
        "SMAppServiceHandle::{api} is only implemented on macOS; \
         the service-management bridge is a no-op on this target"
    ))
}

// ── macOS FFI ──────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
mod ffi {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int};

    use anyhow::{anyhow, Result};
    use objc::{class, msg_send, sel, sel_impl};

    use super::{SMAppServiceKind, SMServiceStatus};

    // SMAppServiceStatus raw values, per Apple's documentation.
    const STATUS_NOT_REGISTERED: c_int = 0;
    const STATUS_ENABLED: c_int = 1;
    const STATUS_REQUIRES_APPROVAL: c_int = 2;
    const STATUS_NOT_FOUND: c_int = 3;
    const STATUS_ENABLED_NOT_RUNNING: c_int = 4;

    #[link(name = "ServiceManagement", kind = "framework")]
    extern "C" {}

    /// Heuristic: is the running bundle likely to satisfy SMAppService's
    /// signing/code-seal requirements?
    ///
    /// SMAppService throws NSException (not NSError) when called on
    /// bundles that are unsigned, ad-hoc-signed, or have a broken seal.
    /// There is no public API to ask "will my status/register call throw?"
    /// without actually entering the FFI, so this check uses a bundle-
    /// state heuristic.
    ///
    /// We look for `Contents/_CodeSignature/CodeResources` — a properly
    /// sealed bundle MUST include this manifest. A linker-signed dev
    /// bundle typically has no `_CodeSignature` directory at all; an
    /// incompletely sealed bundle may have the directory but a missing
    /// or stale `CodeResources`.
    ///
    /// This is NOT a cryptographic validation — a maliciously crafted
    /// `_CodeSignature` would pass this check and still throw at the FFI
    /// boundary. The real validation belongs in a `codesign -dv` shell-out
    /// or an `SecCode` API path, both of which are deferred to future
    /// hardening.
    fn bundle_is_properly_signed() -> bool {
        let bundle = unsafe {
            let cls = class!(NSBundle);
            let bundle: *mut objc::runtime::Object = msg_send![cls, mainBundle];
            bundle
        };
        if bundle.is_null() {
            return false;
        }
        let path: *mut objc::runtime::Object =
            unsafe { msg_send![bundle, bundlePath] };
        if path.is_null() {
            return false;
        }
        let utf8: *const c_char = unsafe { msg_send![path, UTF8String] };
        if utf8.is_null() {
            return false;
        }
        let bundle_path = unsafe { std::ffi::CStr::from_ptr(utf8) }
            .to_string_lossy()
            .into_owned();
        let code_resources = std::path::Path::new(&bundle_path)
            .join("Contents")
            .join("_CodeSignature")
            .join("CodeResources");
        code_resources.exists()
    }

    /// Turn the `SMAppServiceKind` enum into a concrete `SMAppService*`
    /// instance via the appropriate class method. Returned pointer is
    /// autoreleased; the caller does not need to release.
    fn acquire_service(kind: SMAppServiceKind) -> Result<*mut objc::runtime::Object> {
        unsafe {
            // `class!` panics if the class is missing; ServiceManagement's
            // SMAppService is present on every supported macOS version.
            let cls = class!(SMAppService);
            let service: *mut objc::runtime::Object = match kind {
                SMAppServiceKind::MainApp => msg_send![cls, mainApp],
                SMAppServiceKind::Agent { plist_name } => {
                    // `stringWithUTF8String:` returns an autoreleased instance;
                    // no manual release is required and no transient `alloc`
                    // is leaked. The surrounding `with_autorelease_pool` keeps
                    // it alive for the duration of `agentWithPlistName:`.
                    let ns_name = nsstring(plist_name)?;
                    msg_send![cls, agentWithPlistName: ns_name]
                }
            };
            if service.is_null() {
                Err(anyhow!(
                    "SMAppService factory returned nil for {:?}",
                    kind
                ))
            } else {
                Ok(service)
            }
        }
    }

    /// Allocate an autorelease pool, run `body`, drain the pool, and return
    /// `body`'s result. Keeps transient NSStrings from leaking.
    fn with_autorelease_pool<T>(body: impl FnOnce() -> T) -> T {
        unsafe {
            let pool: *mut objc::runtime::Object = msg_send![class!(NSAutoreleasePool), new];
            let result = body();
            let _: () = msg_send![pool, drain];
            result
        }
    }

    /// Allocate an autoreleased `NSString` from a UTF-8 Rust slice. Returned
    /// pointer is owned by the current autorelease pool — callers must hold a
    /// pool alive (via [`with_autorelease_pool`]) for the lifetime they need
    /// the string.
    fn nsstring(s: &str) -> Result<*mut objc::runtime::Object> {
        let c_string = cstring_for_nsstring_input(s)?;
        unsafe {
            let cls = class!(NSString);
            let ns_string: *mut objc::runtime::Object =
                msg_send![cls, stringWithUTF8String: c_string.as_ptr()];
            if ns_string.is_null() {
                Err(anyhow!("NSString conversion returned nil"))
            } else {
                Ok(ns_string)
            }
        }
    }

    fn cstring_for_nsstring_input(s: &str) -> Result<CString> {
        CString::new(s).map_err(|_| anyhow!("NSString input contains an interior NUL byte"))
    }

    #[cfg(test)]
    pub(super) fn cstring_for_nsstring_input_for_test(s: &str) -> Result<CString> {
        cstring_for_nsstring_input(s)
    }

    /// Human-readable label for an `SMAppServiceKind`, used to enrich error
    /// messages with which registration failed. For an Agent we include the
    /// plist filename (e.g. `agent com.busytok.service.plist`) so logs identify
    /// the specific job; for `+mainApp` we say `mainApp`.
    fn kind_label(kind: SMAppServiceKind) -> String {
        match kind {
            SMAppServiceKind::MainApp => "mainApp".to_string(),
            SMAppServiceKind::Agent { plist_name } => format!("agent {plist_name}"),
        }
    }

    pub(super) fn status(kind: SMAppServiceKind) -> Result<SMServiceStatus> {
        if !bundle_is_properly_signed() {
            return Err(anyhow!(
                "SMAppService status skipped: bundle is not properly signed. Kind: {}",
                kind_label(kind)
            ));
        }
        with_autorelease_pool(|| unsafe {
            let service = acquire_service(kind)?;
            let raw: c_int = msg_send![service, status];
            Ok(match raw {
                STATUS_NOT_REGISTERED => SMServiceStatus::NotRegistered,
                STATUS_ENABLED => SMServiceStatus::Enabled,
                STATUS_REQUIRES_APPROVAL => SMServiceStatus::RequiresApproval,
                STATUS_NOT_FOUND => SMServiceStatus::NotFound,
                STATUS_ENABLED_NOT_RUNNING => SMServiceStatus::EnabledNotRunning,
                other => {
                    return Err(anyhow!(
                        "unrecognised SMAppService.status raw value {other}"
                    ))
                }
            })
        })
    }

    pub(super) fn register(kind: SMAppServiceKind) -> Result<()> {
        if !bundle_is_properly_signed() {
            return Err(anyhow!(
                "SMAppService register skipped: bundle is not properly signed (debug or ad-hoc). Kind: {}",
                kind_label(kind)
            ));
        }
        with_autorelease_pool(|| unsafe {
            let service = acquire_service(kind)?;
            let mut error_ptr: *mut objc::runtime::Object = std::ptr::null_mut();
            let ok: bool = msg_send![service, registerAndReturnError: &mut error_ptr];
            if ok {
                Ok(())
            } else {
                Err(nserror_to_anyhow(error_ptr, kind, "registerAndReturnError:"))
            }
        })
    }

    pub(super) fn unregister(kind: SMAppServiceKind) -> Result<()> {
        if !bundle_is_properly_signed() {
            return Err(anyhow!(
                "SMAppService unregister skipped: bundle is not properly signed (debug or ad-hoc). Kind: {}",
                kind_label(kind)
            ));
        }
        with_autorelease_pool(|| unsafe {
            let service = acquire_service(kind)?;
            let mut error_ptr: *mut objc::runtime::Object = std::ptr::null_mut();
            let ok: bool = msg_send![service, unregisterAndReturnError: &mut error_ptr];
            if ok {
                Ok(())
            } else {
                Err(nserror_to_anyhow(error_ptr, kind, "unregisterAndReturnError:"))
            }
        })
    }

    /// Convert the out-param `NSError` from a failed
    /// `registerAndReturnError:` / `unregisterAndReturnError:` call into an
    /// `anyhow::Error` carrying the kind of service and the OS-provided
    /// localized description.
    ///
    /// Per the Objective-C memory contract the caller of `*-AndReturnError:`
    /// owns the returned `NSError` and must release it. We extract the
    /// `localizedDescription` first (the description itself is autoreleased
    /// and will be drained by the surrounding `with_autorelease_pool`), then
    /// release `error_ptr` before constructing the error.
    unsafe fn nserror_to_anyhow(
        error_ptr: *mut objc::runtime::Object,
        kind: SMAppServiceKind,
        api: &'static str,
    ) -> anyhow::Error {
        let label = kind_label(kind);
        if error_ptr.is_null() {
            return anyhow!(
                "SMAppService {api} failed for {label}: returned NO without an NSError"
            );
        }
        let localized: *mut objc::runtime::Object = msg_send![error_ptr, localizedDescription];
        let description = if localized.is_null() {
            None
        } else {
            let utf8: *const c_char = msg_send![localized, UTF8String];
            if utf8.is_null() {
                None
            } else {
                Some(
                    std::ffi::CStr::from_ptr(utf8)
                        .to_string_lossy()
                        .into_owned(),
                )
            }
        };
        // Honour the ownership contract for the out-param NSError: we own it,
        // so we release it now that we have copied out the description we
        // needed.
        let _: () = msg_send![error_ptr, release];
        match description {
            Some(description) => {
                anyhow!("SMAppService {api} failed for {label}: {description}")
            }
            None => anyhow!(
                "SMAppService {api} failed for {label}: returned an NSError with no description"
            ),
        }
    }
}

// ── tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_management_status_mapping_is_lossless() {
        assert_eq!(
            LifecycleStatus::from(SMServiceStatus::EnabledNotRunning),
            LifecycleStatus::RegisteredInactive
        );
        assert_eq!(
            SMServiceStatus::NotRegistered.to_lifecycle_status(),
            LifecycleStatus::NotRegistered
        );
        assert_eq!(
            SMServiceStatus::NotFound.to_lifecycle_status(),
            LifecycleStatus::NotRegistered
        );
        assert_eq!(
            SMServiceStatus::Enabled.to_lifecycle_status(),
            LifecycleStatus::Running
        );
        assert_eq!(
            SMServiceStatus::RequiresApproval.to_lifecycle_status(),
            LifecycleStatus::NeedsAttention
        );
    }

    #[test]
    fn service_management_calls_require_main_thread() {
        // Advertised constant: the bridge says it requires the main thread on
        // macOS and does not on other platforms. We don't actually invoke
        // `status()` here because that would either require running on the
        // main thread or hit the runtime assert.
        let expected = cfg!(target_os = "macos");
        assert_eq!(SMAppServiceHandle::main_thread_only(), expected);
    }

    #[test]
    fn handle_agent_carries_plist_name() {
        let h = SMAppServiceHandle::agent("com.busytok.service.plist");
        assert_eq!(
            h.kind(),
            SMAppServiceKind::Agent {
                plist_name: "com.busytok.service.plist"
            }
        );
    }

    #[test]
    fn handle_main_app_round_trip() {
        let h = SMAppServiceHandle::main_app();
        assert_eq!(h.kind(), SMAppServiceKind::MainApp);
    }

    #[test]
    fn ffi_nsstring_input_is_null_terminated_for_objc() {
        #[cfg(target_os = "macos")]
        {
            let c_string =
                ffi::cstring_for_nsstring_input_for_test("com.busytok.service.plist")
                    .expect("valid plist names should convert to CString");

            assert_eq!(
                c_string.as_bytes_with_nul(),
                b"com.busytok.service.plist\0",
                "NSString::stringWithUTF8String requires a null-terminated C string"
            );
        }
    }

    #[test]
    fn ffi_nsstring_input_rejects_embedded_nul() {
        #[cfg(target_os = "macos")]
        {
            let result = ffi::cstring_for_nsstring_input_for_test(
                "com.busytok.service.plist\0garbage",
            );

            assert!(
                result.is_err(),
                "embedded NUL would truncate or corrupt the Objective-C plist name"
            );
        }
    }

    /// Test executor that runs the closure synchronously on the calling
    /// thread. Real production executors defer onto the framework's main
    /// run loop; this is enough to exercise the bridge's channel plumbing.
    struct InlineExecutor;

    impl MainThreadExecutor for InlineExecutor {
        fn run_on_main_thread(&self, f: Box<dyn FnOnce() + Send>) {
            f();
        }
    }

    #[test]
    fn executor_plumbing_returns_inner_result() {
        let exec = InlineExecutor;
        let exec_ref: &dyn MainThreadExecutor = &exec;
        // We cannot actually invoke the FFI from a unit test on a worker
        // thread without elaborate main-thread gymnastics. Instead exercise
        // the helper directly so the channel/result plumbing is covered.
        let result: Result<u32> =
            run_via_executor(exec_ref, || Ok(7u32));
        assert_eq!(result.unwrap(), 7);

        let failing: Result<()> =
            run_via_executor(exec_ref, || Err(anyhow!("boom")));
        assert!(failing.is_err());
    }

    /// Purpose-built executor: runs op synchronously (like InlineExecutor)
    /// but records whether it was invoked so we can verify the fast-path.
    struct RecordingExecutor {
        invoked: std::sync::atomic::AtomicBool,
    }
    impl MainThreadExecutor for RecordingExecutor {
        fn run_on_main_thread(&self, f: Box<dyn FnOnce() + Send>) {
            self.invoked.store(true, std::sync::atomic::Ordering::SeqCst);
            f();
        }
    }

    #[test]
    fn run_via_executor_fast_path_skips_dispatch_on_main_thread() {
        #[cfg(target_os = "macos")]
        {
            let exec = RecordingExecutor {
                invoked: std::sync::atomic::AtomicBool::new(false),
            };
            let exec_ref: &dyn MainThreadExecutor = &exec;
            let _: Result<u32> = run_via_executor(exec_ref, || Ok(1));
            let on_main = (unsafe { libc::pthread_main_np() }) != 0;
            if on_main {
                assert!(
                    !exec.invoked.load(std::sync::atomic::Ordering::SeqCst),
                    "run_via_executor should NOT dispatch when already on main thread"
                );
            }
            // If not on main thread, dispatch IS expected — that's the
            // normal off-main-thread path and is covered by
            // executor_plumbing_returns_inner_result.
        }
    }

    #[test]
    fn ffi_status_returns_err_not_ok_when_bundle_is_unsigned() {
        #[cfg(target_os = "macos")]
        {
            // In a test binary there is no real app bundle, so
            // bundle_is_properly_signed returns false. The gate in
            // ffi::status must return Err, NOT Ok(NotRegistered).
            let result = ffi::status(SMAppServiceKind::MainApp);
            assert!(
                result.is_err(),
                "ffi::status on an unsigned bundle must return Err, \
                 not Ok(NotRegistered) — Ok(NotRegistered) tricks \
                 upstream logic into trying register() which also fails"
            );
        }
    }

    #[test]
    fn ffi_register_returns_err_when_bundle_is_unsigned() {
        #[cfg(target_os = "macos")]
        {
            let result = ffi::register(SMAppServiceKind::MainApp);
            assert!(
                result.is_err(),
                "ffi::register on an unsigned bundle must return Err"
            );
        }
    }

    #[test]
    fn ffi_unregister_returns_err_when_bundle_is_unsigned() {
        #[cfg(target_os = "macos")]
        {
            let result = ffi::unregister(SMAppServiceKind::MainApp);
            assert!(
                result.is_err(),
                "ffi::unregister on an unsigned bundle must return Err"
            );
        }
    }
}
