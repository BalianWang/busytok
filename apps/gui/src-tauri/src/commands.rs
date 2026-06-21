//! Tauri command wrapper — thin adapter that proxies GUI invokes to the
//! transport-agnostic service layer in `host_application_services`.

use std::sync::Arc;

use serde_json::Value as JsonValue;
use tauri::{AppHandle, Manager, State};

use busytok_protocol::dto::RequestMeta;

use crate::host_application_services::{
    invoke_busytok_via_socket_with_bootstrap, BusytokState, InvokeMeta,
};
use crate::lifecycle_coordinator::LifecycleCause;

/// Invoke a Busytok control method by name, returning the JSON payload.
///
/// This is the single Tauri command the GUI calls. The frontend passes
/// `{ method, params, meta? }` and gets back the `Ok` payload or an error.
///
/// Socket-recovery bootstrap routes through the coordinator-owned lifecycle
/// (when present in Tauri state) so the session-suppression and ensure-
/// coalescing contracts are honored on every invoke path.
#[tauri::command]
pub async fn invoke_busytok(
    method: String,
    params: JsonValue,
    meta: Option<InvokeMeta>,
    state: State<'_, BusytokState>,
    app: AppHandle,
) -> Result<JsonValue, String> {
    let meta = RequestMeta {
        session_id: meta
            .as_ref()
            .and_then(|m| m.session_id().map(|s| s.to_string())),
        correlation_id: meta
            .as_ref()
            .and_then(|m| m.correlation_id().map(|s| s.to_string())),
    };
    tracing::info!(
        event_code = "tauri.invoke_busytok",
        source = "tauri",
        method = %method,
        session_id = meta.session_id.as_deref().unwrap_or(""),
        correlation_id = meta.correlation_id.as_deref().unwrap_or(""),
        "forwarding GUI invoke to busytok-service"
    );
    let socket = state.control_endpoint.clone();
    let coordinator = app
        .try_state::<Arc<crate::lifecycle_coordinator::LifecycleCoordinator>>()
        .map(|s| s.inner().clone());
    invoke_busytok_via_socket_with_bootstrap(
        &method,
        params,
        &socket,
        meta,
        move || async move {
            if let Some(coordinator) = coordinator.as_ref() {
                coordinator
                    .ensure_running(LifecycleCause::CliInvocation)
                    .await
                    .map_err(|e| format!("service bootstrap failed: {e}"))?;
                Ok(())
            } else {
                Err("service bootstrap unavailable (coordinator not initialized)".into())
            }
        },
    )
    .await
}

// ── Desktop lifecycle settings commands ────────────────────────────────

use crate::desktop_lifecycle_settings::{
    DesktopLifecycleSettings, DesktopLifecycleSettingsStore,
};
use crate::desktop_service_status::{
    DesktopBackgroundServiceDiagnostics, DesktopBackgroundServiceState,
};
use crate::lifecycle_coordinator::LifecycleCoordinator;

/// Return the current snapshot of the local desktop lifecycle settings.
#[tauri::command]
pub fn desktop_lifecycle_settings_snapshot(
    settings_store: State<'_, Arc<DesktopLifecycleSettingsStore>>,
) -> Result<DesktopLifecycleSettings, String> {
    let s = settings_store.load();
    tracing::info!(
        event_code = "tauri.desktop_lifecycle_settings_snapshot",
        launch_at_login = s.launch_busytok_desktop_at_login,
    );
    Ok(s)
}

/// Update the desktop lifecycle settings, persist them, and trigger
/// reconciliation (enable/disable login-start as appropriate).
#[tauri::command]
pub async fn desktop_lifecycle_settings_update(
    settings: DesktopLifecycleSettings,
    settings_store: State<'_, Arc<DesktopLifecycleSettingsStore>>,
    coordinator: State<'_, Arc<LifecycleCoordinator>>,
) -> Result<(), String> {
    tracing::info!(
        event_code = "tauri.desktop_lifecycle_settings_update",
        launch_at_login = settings.launch_busytok_desktop_at_login,
    );

    let current = settings_store.load();

    // Reconcile login-start registration BEFORE persisting, so a
    // failed OS reconcile does not leave the on-disk toggle ahead of
    // reality (P2-6). The login_start methods already persist the
    // toggle themselves after successful reconcile.
    if settings.launch_busytok_desktop_at_login != current.launch_busytok_desktop_at_login {
        if settings.launch_busytok_desktop_at_login {
            coordinator
                .login_start()
                .enable_for_current_session()
                .map_err(|e| format!("failed to enable login start: {e}"))?;
        } else {
            coordinator
                .login_start()
                .disable()
                .map_err(|e| format!("failed to disable login start: {e}"))?;
        }
    } else {
        // Toggle unchanged; still persist in case suppression fields
        // or other fields changed.
        settings_store.save(settings);
    }

    // Always trigger an ensure via the coordinator so the lifecycle phase
    // reflects the new settings.
    let _ = coordinator
        .ensure_running(LifecycleCause::SettingsToggle)
        .await;

    Ok(())
}

// ── Background service diagnostics commands ────────────────────────────

/// Return comprehensive diagnostics about the desktop background service.
///
/// Reads the lifecycle coordinator phase and the underlying service lifecycle
/// status to produce a user-facing [`DesktopBackgroundServiceDiagnostics`].
#[tauri::command]
pub async fn desktop_background_service_diagnostics(
    coordinator: State<'_, Arc<LifecycleCoordinator>>,
) -> Result<DesktopBackgroundServiceDiagnostics, String> {
    let gui_build_identity = env!("CARGO_PKG_VERSION").to_string();

    let phase = coordinator.phase_snapshot();

    // Determine the service state from the coordinator phase + lifecycle status.
    let lifecycle_status = coordinator.lifecycle().status();

    let state = match (phase, &lifecycle_status) {
        (crate::lifecycle_coordinator::LifecyclePhase::SuppressedForSession, _) => {
            DesktopBackgroundServiceState::StoppedForThisSession
        }
        (_, Ok(crate::service_lifecycle::LifecycleStatus::Running)) => {
            DesktopBackgroundServiceState::Running
        }
        (_, Ok(crate::service_lifecycle::LifecycleStatus::NotRegistered)) => {
            DesktopBackgroundServiceState::NotRegistered
        }
        (_, Ok(crate::service_lifecycle::LifecycleStatus::NeedsAttention)) => {
            DesktopBackgroundServiceState::NeedsAttention
        }
        (_, Ok(crate::service_lifecycle::LifecycleStatus::Disabled))
        | (_, Ok(crate::service_lifecycle::LifecycleStatus::RegisteredInactive)) => {
            DesktopBackgroundServiceState::NeedsAttention
        }
        (_, Err(_)) => {
            // Status probe failed entirely — service or OS layer is
            // unreachable, most likely needs attention.
            DesktopBackgroundServiceState::NeedsAttention
        }
    };

    let actionable = state.is_actionable();

    // Probe the running service's build identity to populate version-skew
    // diagnostics. Falls back to `None` / `false` when the probe fails
    // (e.g. service not yet reachable, or non-macOS where the probe is
    // unavailable) — those cases are handled by the state classification
    // above.
    let (service_build_identity, version_skew) = match coordinator
        .lifecycle()
        .probe_service_identity()
    {
        Ok(Some(ident)) => {
            let skew = ident != gui_build_identity;
            (Some(ident), skew)
        }
        Ok(None) => (None, false),
        Err(e) => {
            tracing::debug!(
                event_code = "tauri.desktop_background_service_diagnostics.identity_probe_failed",
                error = %e,
                "service identity probe failed; leaving build identity as None"
            );
            (None, false)
        }
    };

    tracing::info!(
        event_code = "tauri.desktop_background_service_diagnostics",
        state = state.as_str(),
        phase = phase.as_str(),
        actionable = actionable,
        gui_build = %gui_build_identity,
        service_build = ?service_build_identity,
        version_skew = version_skew,
    );

    let host_mode_active = coordinator.login_start().host_mode_active();

    Ok(DesktopBackgroundServiceDiagnostics {
        state,
        actionable,
        gui_build_identity,
        service_build_identity,
        version_skew,
        host_mode_active,
    })
}

/// Trigger a best-effort repair of the background service.
///
/// Delegates to [`crate::service_recovery::run_service_recovery`] with the
/// lifecycle instance held by the coordinator.
#[tauri::command]
pub async fn desktop_background_service_repair(
    coordinator: State<'_, Arc<LifecycleCoordinator>>,
) -> Result<(), String> {
    tracing::info!(
        event_code = "tauri.desktop_background_service_repair",
    );

    // Run repair through the coordinator so suppression +
    // quit-priority + coalescing are honored.
    coordinator
        .repair(LifecycleCause::Repair)
        .await
        .map_err(|e| format!("service repair failed: {e}"))
}
