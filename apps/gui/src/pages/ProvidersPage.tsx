import { useCallback, useMemo, useState } from "react";
import type {
  ProviderCreateRequestDto,
  ProviderDto,
  ProviderTestConnectionResponseDto,
  ProviderUpdateRequestDto,
} from "@busytok/protocol-types";
import { useProviderMutations, useProviders } from "../api/useBusytokData";
import { ModelsSection } from "../components/ModelsSection";
import { PageState } from "../components/PageState";
import { ProfilesSection } from "../components/ProfilesSection";
import { SettingsActionGroup } from "../components/desktop/SettingsActionGroup";
import { SettingsRow } from "../components/desktop/SettingsRow";
import { SettingsValue } from "../components/desktop/SettingsValue";
import { ToggleSwitch } from "../components/desktop/ToggleSwitch";
import { reportFrontendEventSafely } from "../logging/safeReporter";

// ── Provider form (inline add + inline edit) ────────────────────────
//
// Create fields (Spec §3.2): name, base_url, api_key (password).
// `provider_kind` is hardcoded to "openai_compatible" (the only variant)
// in the create payload — no UI input (YAGNI).
// `id` is system-generated (UUID v4) by the backend; the form never
// collects it.
//
// Edit fields: name, base_url only. `enabled` has its own toggle on the
// row; `api_key` has its own Update Key flow; `provider_kind` / `id`
// are immutable.

interface ProviderFormState {
  name: string;
  base_url: string;
  api_key: string;
  enabled: boolean;
}

const EMPTY_FORM: ProviderFormState = {
  name: "",
  base_url: "",
  api_key: "",
  enabled: true,
};

interface ProviderFormProps {
  form: ProviderFormState;
  /**
   * `create` shows the api_key field (initial key set). `edit` hides it —
   * the api_key has its own Update Key flow on the provider row, and per
   * the three-state `Option<Option<String>>` contract omitting it from
   * the patch payload means "no change".
   */
  mode: "create" | "edit";
  onChange: (patch: Partial<ProviderFormState>) => void;
  onSubmit: () => void;
  onCancel: () => void;
  isSubmitting: boolean;
}

function ProviderForm({
  form,
  mode,
  onChange,
  onSubmit,
  onCancel,
  isSubmitting,
}: ProviderFormProps) {
  return (
    <div className="settings-panel">
      <SettingsRow
        layout="vertical"
        label="Name"
        description="Display name for this provider."
        control={
          <input
            type="text"
            className="input"
            aria-label="Name"
            placeholder="DeepSeek"
            value={form.name}
            onChange={(e) => onChange({ name: e.currentTarget.value })}
          />
        }
      />
      <SettingsRow
        layout="vertical"
        label="Base URL"
        description="OpenAI-compatible API base URL."
        control={
          <input
            type="text"
            className="input"
            aria-label="Base URL"
            placeholder="https://api.deepseek.com/v1"
            value={form.base_url}
            onChange={(e) => onChange({ base_url: e.currentTarget.value })}
          />
        }
      />
      {mode === "create" && (
        <SettingsRow
          layout="vertical"
          label="API Key"
          description="The actual API key. Stored in the provider catalog database, never written to settings.toml."
          control={
            <input
              type="password"
              className="input"
              aria-label="API Key"
              placeholder="Enter API key"
              value={form.api_key}
              onChange={(e) => onChange({ api_key: e.currentTarget.value })}
            />
          }
        />
      )}
      {mode === "create" && (
        <SettingsRow
          label="Enabled"
          description="Disabled providers are excluded from the model catalog and sidecar routing."
          control={
            <ToggleSwitch
              checked={form.enabled}
              onChange={(checked) => onChange({ enabled: checked })}
            />
          }
        />
      )}
      <SettingsRow
        label="Actions"
        control={
          <SettingsActionGroup direction="row">
            <button
              type="button"
              className="btn btn--primary btn--sm"
              onClick={onSubmit}
              disabled={isSubmitting}
            >
              {isSubmitting ? "Saving..." : "Save"}
            </button>
            <button
              type="button"
              className="btn btn--secondary btn--sm"
              onClick={onCancel}
              disabled={isSubmitting}
            >
              Cancel
            </button>
          </SettingsActionGroup>
        }
      />
    </div>
  );
}

// ── Provider row ────────────────────────────────────────────────────

interface ProviderRowProps {
  provider: ProviderDto;
  isTestPending: boolean;
  testResult: { ok: boolean; error: string | null } | null;
  onTestConnection: (id: string) => void;
  onToggleEnabled: (provider: ProviderDto) => void;
  onDelete: (provider: ProviderDto) => void;
  onUpdateApiKey: (id: string, apiKey: string) => void;
  // Inline edit mode. `id` is immutable and shown read-only in edit mode.
  isEditing: boolean;
  editForm: ProviderFormState;
  isEditPending: boolean;
  onEditProvider: (provider: ProviderDto) => void;
  onEditChange: (patch: Partial<ProviderFormState>) => void;
  onEditSubmit: () => void;
  onEditCancel: () => void;
}

function ProviderRow({
  provider,
  isTestPending,
  testResult,
  onTestConnection,
  onToggleEnabled,
  onDelete,
  onUpdateApiKey,
  isEditing,
  editForm,
  isEditPending,
  onEditProvider,
  onEditChange,
  onEditSubmit,
  onEditCancel,
}: ProviderRowProps) {
  const [apiKeyInput, setApiKeyInput] = useState("");
  const apiKeyStatus = provider.has_api_key ? "API key stored" : "API key not set";
  const apiKeyTone: "default" | "warning" = provider.has_api_key
    ? "default"
    : "warning";

  const handleSubmitApiKey = () => {
    const value = apiKeyInput.trim();
    if (value.length === 0) return;
    onUpdateApiKey(provider.id, value);
    setApiKeyInput("");
  };

  if (isEditing) {
    return (
      <div className="settings-panel">
        <div className="settings-row">
          <div>
            <h3>{`Editing ${provider.name}`}</h3>
            <p>Update provider metadata. Provider ID is immutable. API key has its own Update Key flow when not editing.</p>
          </div>
        </div>
        <SettingsRow
          label="Provider ID"
          description="System-generated UUID; cannot be changed."
          control={<SettingsValue value={provider.id} tone="muted" />}
        />
        <ProviderForm
          form={editForm}
          mode="edit"
          onChange={onEditChange}
          onSubmit={onEditSubmit}
          onCancel={onEditCancel}
          isSubmitting={isEditPending}
        />
      </div>
    );
  }

  return (
    <div className="settings-panel">
      <SettingsRow
        label={provider.name}
        description={
          provider.enabled
            ? undefined
            : "This provider is disabled and will not be used."
        }
        dangerous={!provider.enabled}
        control={
          <ToggleSwitch
            checked={provider.enabled}
            onChange={() => onToggleEnabled(provider)}
            aria-label={`Enable ${provider.name}`}
          />
        }
      />
      <SettingsRow
        label="Provider ID"
        control={<SettingsValue value={provider.id} tone="muted" />}
      />
      <SettingsRow
        label="Kind"
        control={<SettingsValue value={provider.provider_kind} tone="muted" />}
      />
      <SettingsRow
        label="Base URL"
        control={<SettingsValue value={provider.base_url} tone="muted" />}
      />
      <SettingsRow
        label="API Key"
        control={
          <SettingsActionGroup direction="col">
            <SettingsValue value={apiKeyStatus} tone={apiKeyTone} />
            <SettingsActionGroup direction="row">
              <input
                type="password"
                className="input"
                aria-label={`Update API key for ${provider.name}`}
                placeholder={
                  provider.has_api_key ? "•••• (stored)" : "Enter API key"
                }
                value={apiKeyInput}
                onChange={(e) => setApiKeyInput(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    handleSubmitApiKey();
                  }
                }}
              />
              <button
                type="button"
                className="btn btn--secondary btn--sm"
                onClick={handleSubmitApiKey}
              >
                Update Key
              </button>
            </SettingsActionGroup>
          </SettingsActionGroup>
        }
      />
      <SettingsRow
        label="Actions"
        control={
          <SettingsActionGroup direction="row">
            <button
              type="button"
              className="btn btn--secondary btn--sm"
              onClick={() => onEditProvider(provider)}
            >
              Edit
            </button>
            <button
              type="button"
              className="btn btn--secondary btn--sm"
              onClick={() => onTestConnection(provider.id)}
              disabled={isTestPending}
            >
              {isTestPending ? "Testing..." : "Test Connection"}
            </button>
            <button
              type="button"
              className="btn btn--danger btn--sm"
              onClick={() => onDelete(provider)}
            >
              Delete
            </button>
          </SettingsActionGroup>
        }
      />
      {testResult && (
        <SettingsRow
          label="Connection Test"
          control={
            <SettingsValue
              value={
                testResult.ok
                  ? "✓ Connected"
                  : `✗ Failed${testResult.error ? `: ${testResult.error}` : ""}`
              }
              tone={testResult.ok ? "default" : "danger"}
            />
          }
        />
      )}
    </div>
  );
}

// ── Main page ───────────────────────────────────────────────────────

export function ProvidersPage() {
  const { data, isLoading, isError, isFetching } = useProviders();
  const { createProvider, updateProvider, deleteProvider, testConnection } =
    useProviderMutations();

  const [showForm, setShowForm] = useState(false);
  const [form, setForm] = useState<ProviderFormState>(EMPTY_FORM);
  const [testResults, setTestResults] = useState<
    Record<string, { ok: boolean; error: string | null } | null>
  >({});
  const [mutationError, setMutationError] = useState<string | null>(null);
  // Inline edit state. `editingId` is null in view mode; setting it to a
  // provider id swaps that row into the edit form (id is immutable and
  // shown read-only in edit mode).
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editForm, setEditForm] = useState<ProviderFormState>(EMPTY_FORM);

  const providers = useMemo<ProviderDto[]>(
    () => data?.providers ?? [],
    [data?.providers],
  );

  const handleFormChange = useCallback((patch: Partial<ProviderFormState>) => {
    setForm((prev) => ({ ...prev, ...patch }));
  }, []);

  const handleFormSubmit = useCallback(() => {
    // Create payload (Spec §3.2): name + base_url + api_key. provider_kind
    // is hardcoded to the only variant; id is system-generated. Empty
    // api_key is sent as null (Some(None) = "no key on create").
    const payload: ProviderCreateRequestDto = {
      name: form.name.trim(),
      provider_kind: "openai_compatible",
      base_url: form.base_url.trim(),
      enabled: form.enabled,
      api_key: form.api_key.length > 0 ? form.api_key : null,
    };
    setMutationError(null);
    createProvider.mutate(payload, {
      onSuccess: () => {
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "provider.added",
          message: "Provider added",
          details: { name: payload.name },
        });
        setForm(EMPTY_FORM);
        setShowForm(false);
      },
      onError: (err: unknown) => {
        setMutationError((err as Error)?.message ?? String(err));
      },
    });
  }, [form, createProvider]);

  const handleFormCancel = useCallback(() => {
    setForm(EMPTY_FORM);
    setShowForm(false);
  }, []);

  // ── Inline edit handlers ───────────────────────────────────────────
  const handleStartEdit = useCallback((provider: ProviderDto) => {
    setMutationError(null);
    setEditForm({
      name: provider.name,
      base_url: provider.base_url,
      api_key: "", // unused in edit mode (separate Update Key flow)
    });
    setEditingId(provider.id);
  }, []);

  const handleEditChange = useCallback((patch: Partial<ProviderFormState>) => {
    setEditForm((prev) => ({ ...prev, ...patch }));
  }, []);

  const handleEditSubmit = useCallback(() => {
    if (editingId === null) return;
    const id = editingId;
    setMutationError(null);
    // Three-state api_key contract: OMIT api_key/enabled from the patch
    // so the backend treats them as "no change". Only name + base_url are
    // editable in the inline form. Cast `as ProviderUpdateRequestDto` —
    // the generated TS type marks the omitted fields as required (`|
    // null`), but the wire protocol treats absent keys as `None`.
    const payload = {
      id,
      name: editForm.name.trim(),
      base_url: editForm.base_url.trim(),
    } as ProviderUpdateRequestDto;
    updateProvider.mutate(payload, {
      onSuccess: () => {
        setEditingId(null);
      },
      onError: (err: unknown) => {
        setMutationError((err as Error)?.message ?? String(err));
      },
    });
  }, [editForm, editingId, updateProvider]);

  const handleEditCancel = useCallback(() => {
    setEditingId(null);
    setEditForm(EMPTY_FORM);
  }, []);

  const handleToggleEnabled = useCallback(
    (provider: ProviderDto) => {
      setMutationError(null);
      // Omit name/base_url/api_key so the backend preserves them.
      const payload = {
        id: provider.id,
        enabled: !provider.enabled,
      } as ProviderUpdateRequestDto;
      updateProvider.mutate(payload, {
        onError: (err: unknown) => {
          setMutationError((err as Error)?.message ?? String(err));
        },
      });
    },
    [updateProvider],
  );

  const handleUpdateApiKey = useCallback(
    (id: string, apiKey: string) => {
      setMutationError(null);
      // Omit name/base_url/enabled so the backend preserves them. Sending
      // api_key as a string = Some(Some("...")) = update the key.
      const payload = {
        id,
        api_key: apiKey,
      } as ProviderUpdateRequestDto;
      updateProvider.mutate(payload, {
        onError: (err: unknown) => {
          setMutationError((err as Error)?.message ?? String(err));
        },
      });
    },
    [updateProvider],
  );

  const handleDelete = useCallback(
    (provider: ProviderDto) => {
      const confirmed = globalThis.confirm(
        `Delete provider "${provider.name}" (${provider.id})? This cannot be undone.`,
      );
      if (!confirmed) return;
      setMutationError(null);
      deleteProvider.mutate(provider.id, {
        onSuccess: () => {
          reportFrontendEventSafely({
            level: "INFO",
            event_code: "provider.deleted",
            message: "Provider deleted",
            details: { id: provider.id, name: provider.name },
          });
          setTestResults((prev) => {
            const next = { ...prev };
            delete next[provider.id];
            return next;
          });
        },
        onError: (err: unknown) => {
          setMutationError((err as Error)?.message ?? String(err));
        },
      });
    },
    [deleteProvider],
  );

  const handleTestConnection = useCallback(
    (id: string) => {
      // Clear previous result for an immediate visual reset.
      setTestResults((prev) => ({ ...prev, [id]: null }));
      testConnection.mutate(id, {
        onSuccess: (response: ProviderTestConnectionResponseDto) => {
          setTestResults((prev) => ({
            ...prev,
            [id]: { ok: response.ok, error: response.error },
          }));
          reportFrontendEventSafely({
            level: "INFO",
            event_code: "provider.tested",
            message: "Provider connection test completed",
            details: {
              id,
              ok: response.ok,
              error: response.error,
            },
          });
        },
        onError: (err: unknown) => {
          const msg = (err as Error)?.message ?? String(err);
          setTestResults((prev) => ({
            ...prev,
            [id]: { ok: false, error: msg },
          }));
          reportFrontendEventSafely({
            level: "INFO",
            event_code: "provider.tested",
            message: "Provider connection test failed",
            details: { id, ok: false, error: msg },
          });
        },
      });
    },
    [testConnection],
  );

  // Track which provider is currently being tested (for spinner state).
  const testingId: string | null = testConnection.isPending
    ? Object.keys(testResults).find((id) => testResults[id] === null) ?? null
    : null;

  // ── Loading state ──────────────────────────────────────────────────

  if (isLoading && !data) {
    return (
      <div className="settings-page">
        <PageState
          kind="loading"
          title="Providers"
          message="Loading providers..."
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
          title="Providers unavailable"
          message="Could not load providers."
        />
      </div>
    );
  }

  // ── Render ─────────────────────────────────────────────────────────

  return (
    <div className="settings-page" data-fetching={isFetching ? "true" : "false"}>
      <div className="settings-pane">
        {mutationError && (
          <section className="settings-section">
            <div className="settings-panel">
              <SettingsRow
                label="Error"
                control={
                  <SettingsValue value={mutationError} tone="danger" />
                }
              />
            </div>
          </section>
        )}
        <section className="settings-section">
          <h2>Providers</h2>
          <div className="settings-panel">
            <SettingsRow
              label="Add Provider"
              description="Configure a new OpenAI-compatible provider."
              control={
                <button
                  type="button"
                  className="btn btn--primary btn--sm"
                  onClick={() => setShowForm((v) => !v)}
                  disabled={showForm}
                >
                  Add Provider
                </button>
              }
            />
          </div>
          {showForm && (
            <ProviderForm
              form={form}
              mode="create"
              onChange={handleFormChange}
              onSubmit={handleFormSubmit}
              onCancel={handleFormCancel}
              isSubmitting={createProvider.isPending}
            />
          )}
        </section>

        {providers.length === 0 && !showForm ? (
          <section className="settings-section">
            <div className="settings-panel">
              <p>No providers configured. Add one to get started.</p>
            </div>
          </section>
        ) : (
          <section className="settings-section">
            <h2>Configured providers</h2>
            {providers.map((provider) => (
              <ProviderRow
                key={provider.id}
                provider={provider}
                isTestPending={testingId === provider.id}
                testResult={testResults[provider.id] ?? null}
                onTestConnection={handleTestConnection}
                onToggleEnabled={handleToggleEnabled}
                onDelete={handleDelete}
                onUpdateApiKey={handleUpdateApiKey}
                isEditing={editingId === provider.id}
                editForm={editForm}
                isEditPending={updateProvider.isPending && editingId === provider.id}
                onEditProvider={handleStartEdit}
                onEditChange={handleEditChange}
                onEditSubmit={handleEditSubmit}
                onEditCancel={handleEditCancel}
              />
            ))}
          </section>
        )}
        <ModelsSection />
        <ProfilesSection />
      </div>
    </div>
  );
}
