import { invoke } from "@tauri-apps/api/core";

export async function desktopHostShowGui(): Promise<void> {
  await invoke("desktop_host_show_gui");
}

export async function desktopHostShortcutDiagnostics() {
  return invoke<{
    state: string;
    shortcut: string;
    failure_reason: string | null;
    retry_count: number;
  }>("desktop_host_shortcut_diagnostics");
}

export async function desktopHostRetryShortcutRegistration(): Promise<void> {
  await invoke("desktop_host_retry_shortcut_registration");
}
