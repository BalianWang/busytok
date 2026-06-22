import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  ManualRootDto,
  PromptActionDto,
  SettingsDiscoveryDto,
  SettingsPrivacyDto,
  SettingsUpdateRequestDto,
  WeekdayIndexDto,
} from "@busytok/protocol-types";
import {
  useSettingsDiagnostics,
  useSettingsSnapshot,
  useSettingsUpdate,
} from "../api/useBusytokData";
import { BusytokControlError } from "../api/busytokClient";
import { AppSelect, AppSelectItem } from "../components/Select";
import { PageState } from "../components/PageState";
import { SettingsRow } from "../components/desktop/SettingsRow";
import { SegmentedControl } from "../components/desktop/SegmentedControl";
import { useRefreshToolbar } from "../components/desktop/useRefreshToolbar";
import { usePreferences } from "../hooks/usePreferences";
import type { ThemePreference } from "../lib/preferencesStorage";
import { formatBytes } from "../lib/formatters";
import {
  getPromptPaletteShortcutStatus,
  subscribePromptPaletteShortcutStatus,
  type PromptPaletteShortcutStatus,
} from "../lib/promptPaletteShortcutState";
import {
  getPromptPaletteAccessibilityStatus,
  openPromptPaletteAccessibilitySettings,
  type PromptPalettePasteResult,
} from "../lib/promptPaletteActions";
import {
  desktopHostShortcutDiagnostics,
  desktopHostRetryShortcutRegistration,
} from "../lib/desktopHostCommands";
import {
  getDesktopLifecycleSettings,
  updateDesktopLifecycleSettings,
  getBackgroundServiceDiagnostics,
  repairBackgroundService,
  type DesktopLifecycleSettings,
  type DesktopBackgroundServiceDiagnostics,
} from "../lib/backgroundServiceCommands";
import { useUpdater } from "../hooks/useUpdater";
import { reportFrontendEvent } from "../logging/reporter";

const COMMIT_TIMEOUT_MS = 10_000; // 10s bounded timeout for settings writes

// ── Helpers ──────────────────────────────────────────────────────────

function toWeekdayIndex(v: number): WeekdayIndexDto {
  return (v === 0 || v === 1 ? v : 0) as WeekdayIndexDto;
}

function shortcutStatusText(status: PromptPaletteShortcutStatus): string {
  if (status.state === "registered") {
    return "Registered";
  }
  if (status.state === "failed") {
    return "Unavailable";
  }
  return "Using in-app fallback";
}

function isMacPlatform(): boolean {
  return /Mac/i.test(globalThis.navigator?.platform ?? "");
}

function pasteStatusText(status: PromptPalettePasteResult): string {
  if (status.ok) {
    return "Ready";
  }
  if (status.failure_reason === "permission_missing") {
    return "Permission needed";
  }
  return "Copy fallback";
}

// ── Main page ────────────────────────────────────────────────────────

export function SettingsPage() {
  const { data, isLoading, isError, isFetching, refetch } = useSettingsSnapshot();
  const diagQuery = useSettingsDiagnostics();
  const updateMutation = useSettingsUpdate();

  const [localWeekStart, setLocalWeekStart] = useState<number | null>(null);
  const [localDiscovery, setLocalDiscovery] = useState<SettingsDiscoveryDto | null>(null);
  const [localPrivacy, setLocalPrivacy] = useState<SettingsPrivacyDto | null>(null);
  const [localDefaultAction, setLocalDefaultAction] = useState<PromptActionDto | null>(null);
  const [validationErrors, setValidationErrors] = useState<Record<string, string>>({});
  const [commitTimedOut, setCommitTimedOut] = useState(false);
  const [shortcutStatus, setShortcutStatus] = useState(getPromptPaletteShortcutStatus);
  const [hostShortcutDiagnostics, setHostShortcutDiagnostics] = useState<{
    state: string;
    shortcut: string;
    failure_reason: string | null;
    retry_count: number;
  } | null>(null);
  const [pasteStatus, setPasteStatus] = useState<PromptPalettePasteResult>({
    ok: false,
    failure_reason: "unsupported_platform",
  });

  // ── Lifecycle settings state ─────────────────────────────────────────
  const [lifecycleSettings, setLifecycleSettings] =
    useState<DesktopLifecycleSettings | null>(null);
  const [lifecycleSettingsLoading, setLifecycleSettingsLoading] = useState(false);

  // ── Background service diagnostics state ─────────────────────────────
  const [bgDiag, setBgDiag] =
    useState<DesktopBackgroundServiceDiagnostics | null>(null);
  const [bgDiagLoading, setBgDiagLoading] = useState(false);
  const [showBgDiagnostics, setShowBgDiagnostics] = useState(false);
  const [bgRepairing, setBgRepairing] = useState(false);
  const [bgDiagError, setBgDiagError] = useState<string | null>(null);

  const lastCommitPatchRef = useRef<Partial<SettingsUpdateRequestDto> | null>(null);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Sync local state when snapshot data loads / refreshes
  // We hold local state for each editable section so the user can edit
  // without waiting for round-trips.
  const snapshot = data?.data;
  const timezone = snapshot?.timezone ?? "";
  const weekStart = localWeekStart ?? snapshot?.week_starts_on ?? 0;
  const discovery = localDiscovery ?? snapshot?.discovery;
  const privacy = localPrivacy ?? snapshot?.privacy;
  const defaultAction = localDefaultAction ?? snapshot?.prompt_palette_default_action ?? "paste";

  const handleMutate = useCallback(
    (patch: Partial<SettingsUpdateRequestDto>) => {
      setValidationErrors({});
      setCommitTimedOut(false);
      lastCommitPatchRef.current = patch;

      const body: SettingsUpdateRequestDto = {
        timezone: patch.timezone !== undefined ? patch.timezone : null,
        week_starts_on: patch.week_starts_on !== undefined ? patch.week_starts_on : null,
        discovery: patch.discovery !== undefined ? patch.discovery : null,
        privacy: patch.privacy !== undefined ? patch.privacy : null,
        prompt_palette_default_action: patch.prompt_palette_default_action !== undefined ? patch.prompt_palette_default_action : null,
      };

      // Clear any previous timeout
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
      }

      // Bounded timeout: if the mutation doesn't settle within COMMIT_TIMEOUT_MS,
      // mark it as timed out and let the user retry.
      timeoutRef.current = setTimeout(() => {
        setCommitTimedOut(true);
      }, COMMIT_TIMEOUT_MS);

      updateMutation.mutate(body, {
        onSettled: () => {
          if (timeoutRef.current) {
            clearTimeout(timeoutRef.current);
            timeoutRef.current = null;
          }
        },
        onError: (err: unknown) => {
          if (err instanceof BusytokControlError && err.code === 'settings_validation_failed') {
            const payload = err.payload as { errors?: Array<{ code: string; field_path: string; message: string }> } | null;
            if (payload?.errors) {
              const errors: Record<string, string> = {};
              for (const item of payload.errors) {
                errors[item.field_path] = item.message;
              }
              setValidationErrors(errors);
              return;
            }
          }
          const msg = (err as any)?.message ?? String(err);
          setValidationErrors({ _general: msg });
        },
      });
    },
    [updateMutation],
  );

  // Cleanup timeout on unmount
  useEffect(() => {
    return () => {
      if (timeoutRef.current) clearTimeout(timeoutRef.current);
    };
  }, []);

  useEffect(() => subscribePromptPaletteShortcutStatus(setShortcutStatus), []);

  useEffect(() => {
    let cancelled = false;
    desktopHostShortcutDiagnostics()
      .then((diag) => {
        if (!cancelled) {
          setHostShortcutDiagnostics(diag);
        }
      })
      .catch(() => {
        // Host diagnostics not available; fallback to in-app status
        reportFrontendEvent({ level: "WARN", event_code: "gui.settings.shortcut_diagnostics_failed", message: "Failed to fetch shortcut diagnostics" });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const fetchPasteStatus = useCallback(
    async () =>
      getPromptPaletteAccessibilityStatus().catch(() => ({
        ok: false as const,
        failure_reason: "unsupported_platform" as const,
      })),
    [],
  );

  useEffect(() => {
    let cancelled = false;
    fetchPasteStatus().then((status) => {
      if (!cancelled) {
        setPasteStatus(status);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [fetchPasteStatus]);

  // Fetch desktop lifecycle settings on mount.
  const fetchLifecycleSettings = useCallback(async () => {
    setLifecycleSettingsLoading(true);
    try {
      const s = await getDesktopLifecycleSettings();
      setLifecycleSettings(s);
    } catch {
      // Lifecycle settings unavailable (non-macOS or Tauri bridge issue).
      // The UI gracefully hides the toggle.
    } finally {
      setLifecycleSettingsLoading(false);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    fetchLifecycleSettings().then(() => {
      // no-op; state is already set
    });
    return () => {
      cancelled = true;
    };
  }, [fetchLifecycleSettings]);

  // Fetch background service diagnostics on mount.
  const fetchBgDiagnostics = useCallback(async () => {
    setBgDiagLoading(true);
    setBgDiagError(null);
    try {
      const d = await getBackgroundServiceDiagnostics();
      setBgDiag(d);
    } catch (e) {
      setBgDiagError((e as Error).message ?? String(e));
    } finally {
      setBgDiagLoading(false);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    fetchBgDiagnostics().then(() => {
      // no-op
    });
    return () => {
      cancelled = true;
    };
  }, [fetchBgDiagnostics]);

  const handleRefresh = useCallback(async () => {
    await refetch();
    setPasteStatus(await fetchPasteStatus());
    void fetchLifecycleSettings();
    void fetchBgDiagnostics();
  }, [fetchPasteStatus, refetch, fetchLifecycleSettings, fetchBgDiagnostics]);

  useRefreshToolbar({
    surface: "settings",
    onRefresh: handleRefresh,
    isFetching,
  });

  // ── Theme preference (frontend-local) ──────────────────────────────
  // Appearance is a local concern: it lives in preferencesStorage and is
  // applied live by themeRuntime via PREFERENCES_UPDATED_EVENT. It never
  // touches the server-backed settings mutation flow.
  const { preferences, updatePreference } = usePreferences();
  const { status: updateStatus, checkNow: checkForUpdates } = useUpdater();

  const handleThemePreferenceChange = useCallback(
    (next: ThemePreference) => {
      if (next === preferences.themePreference) return;
      updatePreference("themePreference", next);
      reportFrontendEvent({
        level: "INFO",
        event_code: "gui.theme.preference_changed",
        message: "User changed theme preference",
        details: { preference: next },
      });
    },
    [preferences.themePreference, updatePreference],
  );

  // Retry last commit
  const handleRetryCommit = useCallback(() => {
    if (lastCommitPatchRef.current) {
      handleMutate(lastCommitPatchRef.current);
    }
  }, [handleMutate]);

  // ── Week starts on ─────────────────────────────────────────────────

  const handleWeekStartChange = useCallback(
    (value: number) => {
      if (value !== snapshot?.week_starts_on) {
        setLocalWeekStart(value);
        handleMutate({ week_starts_on: toWeekdayIndex(value) });
      }
    },
    [handleMutate, snapshot?.week_starts_on],
  );

  // ── Discovery toggles ─────────────────────────────────────────────

  const handleDiscoveryToggle = useCallback(
    (key: "claude_code_default_paths" | "codex_default_paths") => {
      if (!discovery) return;
      const next: SettingsDiscoveryDto = {
        ...discovery,
        [key]: !discovery[key],
      };
      setLocalDiscovery(next);
      handleMutate({ discovery: next });
    },
    [discovery, handleMutate],
  );

  // ── Manual roots ───────────────────────────────────────────────────

  const [localManualRoots, setLocalManualRoots] = useState<ManualRootDto[] | null>(null);
  const manualRoots = localManualRoots ?? discovery?.manual_roots ?? [];

  const syncManualRoots = useCallback(
    (roots: ManualRootDto[]) => {
      if (!discovery) return;
      setLocalManualRoots(roots);
      const next: SettingsDiscoveryDto = {
        ...discovery,
        manual_roots: roots,
      };
      setLocalDiscovery(next);
      handleMutate({ discovery: next });
    },
    [discovery, handleMutate],
  );

  const handleAddRoot = useCallback(() => {
    syncManualRoots([
      ...manualRoots,
      { id: "", client_id: "", root_path: "", source_type: "manual_root" },
    ]);
  }, [manualRoots, syncManualRoots]);

  const handleRemoveRoot = useCallback(
    (index: number) => {
      const next = manualRoots.filter((_, i) => i !== index);
      syncManualRoots(next);
    },
    [manualRoots, syncManualRoots],
  );

  const handleRootChange = useCallback(
    (index: number, field: "client_id" | "root_path", value: string) => {
      const next = manualRoots.map((r, i) =>
        i === index ? { ...r, [field]: value } : r,
      );
      syncManualRoots(next);
    },
    [manualRoots, syncManualRoots],
  );

  // ── Privacy toggles ────────────────────────────────────────────────

  const handlePrivacyToggle = useCallback(
    (key: "local_only" | "redact_sensitive_values") => {
      if (!privacy) return;
      const next: SettingsPrivacyDto = {
        ...privacy,
        [key]: !privacy[key],
      };
      setLocalPrivacy(next);
      handleMutate({ privacy: next });
    },
    [privacy, handleMutate],
  );

  // ── Prompt Palette default action ──────────────────────────────────

  const handleDefaultActionChange = useCallback(
    (value: string) => {
      if (value !== snapshot?.prompt_palette_default_action) {
        setLocalDefaultAction(value as PromptActionDto);
        handleMutate({ prompt_palette_default_action: value as PromptActionDto });
      }
    },
    [handleMutate, snapshot?.prompt_palette_default_action],
  );

  // ── Diagnostics data ───────────────────────────────────────────────

  const diagnostics = diagQuery.data?.data ?? snapshot?.diagnostics;

  // ── Loading state ──────────────────────────────────────────────────

  if (isLoading && !data) {
    return (
      <div className="settings-page">
        <PageState
          kind="loading"
          title="Settings"
          message="Loading settings data..."
        />
      </div>
    );
  }

  // ── Error state ────────────────────────────────────────────────────

  if (isError && !data) {
    return (
      <div className="settings-page">
        <PageState
          kind="error"
          title="Settings unavailable"
          message="Could not load settings data."
          actionLabel="Retry"
          onAction={() => void handleRefresh()}
        />
      </div>
    );
  }

  // ── Helpers: get validation error for a field path ─────────────────

  const fieldError = (path: string): string | null =>
    validationErrors[path] ?? null;

  // ── Render ─────────────────────────────────────────────────────────

  return (
    <div className="settings-page">
      <div className="settings-pane">
        {/* ── Commit timeout banner ───────────────────────────────── */}
        {commitTimedOut && (
          <div className="settings-panel settings-panel--warning" role="alert">
            <p className="settings-panel__warning-text">
              Settings commit is taking longer than expected. The change may still
              be applied — check the diagnostics panel for status.
            </p>
            <button
              type="button"
              className="btn btn--secondary"
              onClick={handleRetryCommit}
              disabled={updateMutation.isPending}
            >
              Retry commit
            </button>
          </div>
        )}

        {/* ── Appearance (frontend-local) ──────────────────────────── */}
        <section className="settings-section">
          <h2>Appearance</h2>
          <div className="settings-panel">
            <SettingsRow
              label="Theme"
              description="Choose light, dark, or follow the system. Applies immediately and stays in sync if you switch back to System."
              control={
                <SegmentedControl
                  label="Theme"
                  value={preferences.themePreference}
                  options={[
                    { value: "system", label: "System" },
                    { value: "light", label: "Light" },
                    { value: "dark", label: "Dark" },
                  ]}
                  onChange={handleThemePreferenceChange}
                />
              }
            />
          </div>
        </section>

        {/* ── Prompt Palette shortcut ────────────────────────────── */}
        <section className="settings-section">
          <h2>Keyboard</h2>
          <div className="settings-panel">
            <SettingsRow
              label="Prompt Palette Shortcut"
              description={
                hostShortcutDiagnostics?.state === "failed"
                  ? `Global shortcut could not be registered: ${hostShortcutDiagnostics.failure_reason ?? "unknown"}. Cmd/Ctrl+Shift+K still works while Busytok is focused.`
                  : shortcutStatus.state === "failed"
                    ? "Global shortcut could not be registered. Cmd/Ctrl+Shift+K still works while Busytok is focused."
                    : undefined
              }
              control={
                hostShortcutDiagnostics?.state === "failed" ? (
                  <div className="manual-root-controls">
                    <span className="diag-value">
                      Unavailable
                    </span>
                    <button
                      type="button"
                      className="btn btn--secondary btn--sm"
                      onClick={() => {
                        reportFrontendEvent({ level: "INFO", event_code: "gui.settings.shortcut_retry_clicked", message: "Shortcut registration retry requested" });
                        void desktopHostRetryShortcutRegistration().then(() => {
                          return desktopHostShortcutDiagnostics().then((diag) => {
                            setHostShortcutDiagnostics(diag);
                          });
                        }).catch(() => {});
                      }}
                    >
                      Retry
                    </button>
                  </div>
                ) : (
                  <span className="diag-value">
                    {shortcutStatusText(shortcutStatus)}
                  </span>
                )
              }
            />
          </div>
        </section>

        {/* ── Prompt Palette default action ────────────────────────── */}
        <section className="settings-section">
          <h2>Prompt Palette</h2>
          <div className="settings-panel">
            <SettingsRow
              label="Default action"
              description="Choose whether pressing Enter on a prompt copies it or pastes it into the active app."
              error={fieldError("prompt_palette_default_action")}
              control={
                <AppSelect
                  label="Default action"
                  aria-label="Prompt palette default action"
                  value={defaultAction}
                  onValueChange={handleDefaultActionChange}
                >
                  <AppSelectItem value="paste">Paste</AppSelectItem>
                  <AppSelectItem value="copy">Copy</AppSelectItem>
                </AppSelect>
              }
            />
          </div>
        </section>

        {/* ── Reporting timezone ──────────────────────────────────────── */}
        <section className="settings-section">
          <h2>Reporting timezone</h2>
          <div className="settings-panel">
            <SettingsRow
              label="Reporting timezone"
              description={`Follows system: ${timezone}`}
              control={
                <span className="diag-value">{timezone}</span>
              }
            />
          </div>
        </section>

        {/* ── Week starts on ───────────────────────────────────────── */}
        <section className="settings-section">
          <h2>Week starts on</h2>
          <div className="settings-panel">
            <SettingsRow
              label="Week starts on"
              description="First day of the week for calendar views."
              error={fieldError("week_starts_on")}
              control={
                <fieldset className="segmented-group" aria-label="Week start day">
                  <label className="segmented-label">
                    <input
                      type="radio"
                      name="week_start"
                      value="0"
                      checked={weekStart === 0}
                      onChange={() => handleWeekStartChange(0)}
                      aria-label="Sunday"
                    />
                    <span>Sunday</span>
                  </label>
                  <label className="segmented-label">
                    <input
                      type="radio"
                      name="week_start"
                      value="1"
                      checked={weekStart === 1}
                      onChange={() => handleWeekStartChange(1)}
                      aria-label="Monday"
                    />
                    <span>Monday</span>
                  </label>
                </fieldset>
              }
            />
          </div>
        </section>

        {/* ── Source discovery ─────────────────────────────────────── */}
        <section className="settings-section">
          <h2>Source discovery</h2>
          <div className="settings-panel">
            {discovery && (
              <>
                <SettingsRow
                  label="Claude Code"
                  description="Scan default Claude Code config paths."
                  error={fieldError("discovery.claude_code_default_paths")}
                  control={
                    <label className="toggle-label">
                      <input
                        type="checkbox"
                        className="toggle"
                        checked={discovery.claude_code_default_paths}
                        onChange={() => handleDiscoveryToggle("claude_code_default_paths")}
                        aria-label="Claude Code"
                      />
                      <span className="toggle-track" />
                    </label>
                  }
                />
                <SettingsRow
                  label="Codex"
                  description="Scan default Codex config paths."
                  error={fieldError("discovery.codex_default_paths")}
                  control={
                    <label className="toggle-label">
                      <input
                        type="checkbox"
                        className="toggle"
                        checked={discovery.codex_default_paths}
                        onChange={() => handleDiscoveryToggle("codex_default_paths")}
                        aria-label="Codex"
                      />
                      <span className="toggle-track" />
                    </label>
                  }
                />
              </>
            )}
          </div>
        </section>

        {/* ── Manual roots ─────────────────────────────────────────── */}
        <section className="settings-section">
          <h2>Manual roots</h2>
          {manualRoots.map((root, i) => (
            <div className="settings-panel" key={i}>
              <SettingsRow
                label={`Root ${i + 1}`}
                description="Select client and enter root path."
                error={fieldError(`discovery.manual_roots[${i}].root_path`) || fieldError(`discovery.manual_roots[${i}].client_id`)}
                control={
                  <div className="manual-root-controls">
                    <input
                      type="text"
                      className="input"
                      placeholder="Client ID"
                      value={root.client_id}
                      onChange={(e) => handleRootChange(i, "client_id", e.currentTarget.value)}
                      aria-label={`Root ${i + 1} client`}
                    />
                    <input
                      type="text"
                      className="input"
                      placeholder="Root path"
                      value={root.root_path}
                      onChange={(e) => handleRootChange(i, "root_path", e.currentTarget.value)}
                      aria-label={`Root ${i + 1} path`}
                    />
                    <button
                      type="button"
                      className="btn btn--danger"
                      onClick={() => handleRemoveRoot(i)}
                      aria-label={`Remove root ${i + 1}`}
                    >
                      Remove
                    </button>
                  </div>
                }
              />
            </div>
          ))}
          <div className="settings-panel">
            <button
              type="button"
              className="btn btn--secondary"
              onClick={handleAddRoot}
              disabled={!discovery}
            >
              Add root
            </button>
          </div>
        </section>

        {/* ── Privacy ──────────────────────────────────────────────── */}
        <section className="settings-section">
          <h2>Privacy</h2>
          <div className="settings-panel">
            {privacy && (
              <>
                <SettingsRow
                  label="Local only"
                  description="Keep all data local, disable network features."
                  error={fieldError("privacy.local_only")}
                  control={
                    <label className="toggle-label">
                      <input
                        type="checkbox"
                        className="toggle"
                        checked={privacy.local_only}
                        onChange={() => handlePrivacyToggle("local_only")}
                        aria-label="Local only"
                      />
                      <span className="toggle-track" />
                    </label>
                  }
                />
                <SettingsRow
                  label="Redact sensitive values"
                  description="Mask sensitive information in logs and displays."
                  error={fieldError("privacy.redact_sensitive_values")}
                  control={
                    <label className="toggle-label">
                      <input
                        type="checkbox"
                        className="toggle"
                        checked={privacy.redact_sensitive_values}
                        onChange={() => handlePrivacyToggle("redact_sensitive_values")}
                        aria-label="Redact sensitive values"
                      />
                      <span className="toggle-track" />
                    </label>
                  }
                />
              </>
            )}
          </div>
        </section>

        {/* ── Launch Busytok Desktop at login ────────────────────────── */}
        {lifecycleSettings && !lifecycleSettingsLoading && (
          <section className="settings-section">
            <h2>Desktop</h2>
            <div className="settings-panel">
              <SettingsRow
                label="Launch Busytok Desktop at login"
                description="When enabled, Busytok starts automatically when you log in. The menu bar icon and global shortcut remain available."
                control={
                  <label className="toggle-label">
                    <input
                      type="checkbox"
                      className="toggle"
                      checked={lifecycleSettings.launch_busytok_desktop_at_login}
                      onChange={() => {
                        const next: DesktopLifecycleSettings = {
                          launch_busytok_desktop_at_login:
                            !lifecycleSettings.launch_busytok_desktop_at_login,
                        };
                        setLifecycleSettings(next);
                        void updateDesktopLifecycleSettings(next).catch(() => {
                          // Revert on failure.
                          setLifecycleSettings({
                            launch_busytok_desktop_at_login:
                              lifecycleSettings.launch_busytok_desktop_at_login,
                          });
                        });
                      }}
                      aria-label="Launch Busytok Desktop at login"
                    />
                    <span className="toggle-track" />
                  </label>
                }
              />
            </div>
          </section>
        )}

        {/* ── Background Service ────────────────────────────────────────── */}
        {!bgDiagError && (
          <section className="settings-section">
            <h2>Background Service</h2>
            <div className="settings-panel">
              {bgDiagLoading && !bgDiag && (
                <SettingsRow
                  label="Background Service"
                  control={<span className="diag-value">Checking...</span>}
                />
              )}
              {!bgDiagLoading && bgDiag && (
                <>
                  <SettingsRow
                    label="Status"
                    description={
                      bgDiag.state === "stopped_for_this_session"
                        ? "The background service has been stopped for the current session. Open Busytok.app to restart it."
                        : bgDiag.state === "needs_attention"
                          ? "The background service needs attention. Repairs may resolve the issue."
                          : bgDiag.state === "not_registered"
                            ? "The background service is not registered. Repair will attempt to register it."
                            : undefined
                    }
                    control={
                      <span
                        className={`diag-badge diag-badge--${
                          bgDiag.state === "running"
                            ? "ok"
                            : bgDiag.state === "starting"
                              ? "ok"
                              : "error"
                        }`}
                      >
                        {bgDiag.state === "stopped_for_this_session"
                          ? "Stopped for session"
                          : bgDiag.state === "needs_attention"
                            ? "Needs attention"
                            : bgDiag.state === "not_registered"
                              ? "Not registered"
                              : bgDiag.state === "starting"
                                ? "Starting"
                                : "Running"}
                      </span>
                    }
                  />
                  {bgDiag.actionable && (
                    <SettingsRow
                      label="Repair"
                      description="Attempt to repair the background service registration and restart it."
                      control={
                        <button
                          type="button"
                          className="btn btn--secondary btn--sm"
                          disabled={bgRepairing}
                          onClick={() => {
                            setBgRepairing(true);
                            void repairBackgroundService()
                              .then(() => fetchBgDiagnostics())
                              .catch(() => {
                                // Repair failed; diagnostics will reflect the state.
                              })
                              .finally(() => setBgRepairing(false));
                          }}
                        >
                          {bgRepairing ? "Repairing..." : "Repair Background Service"}
                        </button>
                      }
                    />
                  )}
                  <SettingsRow
                    label="Show Diagnostics"
                    description="View detailed background service diagnostics information."
                    control={
                      <label className="toggle-label">
                        <input
                          type="checkbox"
                          className="toggle"
                          checked={showBgDiagnostics}
                          onChange={() =>
                            setShowBgDiagnostics(!showBgDiagnostics)
                          }
                          aria-label="Show Diagnostics"
                        />
                        <span className="toggle-track" />
                      </label>
                    }
                  />
                  {showBgDiagnostics && (
                    <>
                      <SettingsRow
                        label="GUI build"
                        control={
                          <span className="diag-value">
                            {bgDiag.gui_build_identity}
                          </span>
                        }
                      />
                      <SettingsRow
                        label="Service build"
                        control={
                          <span className="diag-value">
                            {bgDiag.service_build_identity ?? "Unknown"}
                          </span>
                        }
                      />
                      <SettingsRow
                        label="Version skew"
                        control={
                          <span
                            className={`diag-badge diag-badge--${
                              bgDiag.version_skew ? "error" : "ok"
                            }`}
                          >
                            {bgDiag.version_skew ? "Yes" : "No"}
                          </span>
                        }
                      />
                    </>
                  )}
                </>
              )}
            </div>
          </section>
        )}

        {/* ── Updates ──────────────────────────────────────────────── */}
        <section className="settings-section">
          <h2>Updates</h2>
          <div className="settings-panel">
            <SettingsRow
              label="Software Update"
              description={
                updateStatus.state === "done" && updateStatus.result.kind === "updated"
                  ? `Updated to ${updateStatus.result.version}. Restarting...`
                  : updateStatus.state === "done" && updateStatus.result.kind === "error"
                    ? `Update check failed: ${updateStatus.result.message}`
                    : "Check for and install the latest version of Busytok."
              }
              control={
                <button
                  type="button"
                  className="btn btn--secondary btn--sm"
                  disabled={updateStatus.state === "checking"}
                  onClick={() => void checkForUpdates()}
                >
                  {updateStatus.state === "checking"
                    ? "Checking..."
                    : updateStatus.state === "done" && updateStatus.result.kind === "up-to-date"
                      ? "Up to date"
                      : updateStatus.state === "done" && updateStatus.result.kind === "updated"
                        ? `Updated to ${updateStatus.result.version}`
                        : updateStatus.state === "done" && updateStatus.result.kind === "error"
                          ? "Retry"
                          : "Check for updates"}
                </button>
              }
            />
          </div>
        </section>

        {/* ── Diagnostics ──────────────────────────────────────────── */}
        {diagnostics && (
          <section className="settings-section">
            <h2>Diagnostics</h2>
            <div className="settings-panel">
              <SettingsRow
                label="DB healthy"
                control={<span className={`diag-badge diag-badge--${diagnostics.db_healthy ? "ok" : "error"}`}>{diagnostics.db_healthy ? "Yes" : "No"}</span>}
              />
              <SettingsRow
                label="DB size"
                control={<span className="diag-value">{formatBytes(diagnostics.db_size_bytes)}</span>}
              />
              <SettingsRow
                label="Migration version"
                control={<span className="diag-value">{diagnostics.migration_version}</span>}
              />
              <SettingsRow
                label="Event count"
                control={<span className="diag-value">{diagnostics.usage_event_count.toLocaleString()}</span>}
              />
              <SettingsRow
                label="Last checkpoint"
                control={
                  <span className="diag-value">
                    {diagnostics.last_log_checkpoint_ms != null
                      ? new Date(diagnostics.last_log_checkpoint_ms).toLocaleString()
                      : "None"}
                  </span>
                }
              />
              <SettingsRow
                label="Prompt Palette Paste"
                description={
                  pasteStatus.failure_reason === "permission_missing"
                    ? "Accessibility permission is required for automatic paste."
                    : undefined
                }
                control={
                  pasteStatus.failure_reason === "permission_missing" && isMacPlatform() ? (
                    <div className="manual-root-controls">
                      <span className="diag-value">{pasteStatusText(pasteStatus)}</span>
                      <button
                        type="button"
                        className="btn btn--secondary btn--sm"
                        onClick={() => void openPromptPaletteAccessibilitySettings()}
                      >
                        Open System Settings
                      </button>
                    </div>
                  ) : (
                    <span className="diag-value">{pasteStatusText(pasteStatus)}</span>
                  )
                }
              />
            </div>
          </section>
        )}
      </div>
    </div>
  );
}
