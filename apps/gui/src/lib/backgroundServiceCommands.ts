//! Background service diagnostic commands — typed `invoke` wrappers
//! for the Tauri commands registered by the desktop lifecycle subsystem.
//!
//! These are independent of the `busytokClient` IPC path; they talk
//! directly to the GUI process via Tauri's built-in command invoke.

import { invoke } from "@tauri-apps/api/core";

/** Mirrors the Rust `DesktopLifecycleSettings` struct. */
export interface DesktopLifecycleSettings {
  launch_busytok_desktop_at_login: boolean;
}

/** Mirrors the Rust `DesktopBackgroundServiceState` enum. */
export type DesktopBackgroundServiceState =
  | "running"
  | "starting"
  | "not_registered"
  | "stopped_for_this_session"
  | "needs_attention";

/** Mirrors the Rust `DesktopBackgroundServiceDiagnostics` struct. */
export interface DesktopBackgroundServiceDiagnostics {
  state: DesktopBackgroundServiceState;
  actionable: boolean;
  gui_build_identity: string;
  service_build_identity: string | null;
  version_skew: boolean;
}

/**
 * Fetch the current local desktop lifecycle settings snapshot.
 */
export async function getDesktopLifecycleSettings(): Promise<DesktopLifecycleSettings> {
  return invoke<DesktopLifecycleSettings>("desktop_lifecycle_settings_snapshot");
}

/**
 * Update the desktop lifecycle settings and trigger reconciliation.
 */
export async function updateDesktopLifecycleSettings(
  settings: DesktopLifecycleSettings,
): Promise<void> {
  await invoke("desktop_lifecycle_settings_update", { settings });
}

/**
 * Fetch comprehensive diagnostics about the desktop background service.
 */
export async function getBackgroundServiceDiagnostics(): Promise<DesktopBackgroundServiceDiagnostics> {
  return invoke<DesktopBackgroundServiceDiagnostics>("desktop_background_service_diagnostics");
}

/**
 * Trigger a best-effort repair of the desktop background service.
 */
export async function repairBackgroundService(): Promise<void> {
  await invoke("desktop_background_service_repair");
}
