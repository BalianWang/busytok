import { useCallback, useMemo, useState } from "react";
import type {
  ProviderCreateRequestDto,
  ProviderDto,
  ProviderTestConnectionResponseDto,
} from "@busytok/protocol-types";
import { useProviderMutations, useProviders } from "../api/useBusytokData";
import { PageState } from "../components/PageState";
import { SettingsActionGroup } from "../components/desktop/SettingsActionGroup";
import { SettingsRow } from "../components/desktop/SettingsRow";
import { SettingsValue } from "../components/desktop/SettingsValue";
import { ToggleSwitch } from "../components/desktop/ToggleSwitch";
import { reportFrontendEventSafely } from "../logging/safeReporter";

// ── Helpers ──────────────────────────────────────────────────────────

function parseModels(input: string): string[] {
  return input
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

function modelsLabel(models: string[]): string {
  if (models.length === 0) return "None";
  return models.join(", ");
}

// ── Provider form (inline add) ──────────────────────────────────────

interface ProviderFormState {
  id: string;
  name: string;
  base_url: string;
  api_key_env_name: string;
  models: string;
  api_key: string;
}

const EMPTY_FORM: ProviderFormState = {
  id: "",
  name: "",
  base_url: "",
  api_key_env_name: "",
  models: "",
  api_key: "",
};

interface ProviderFormProps {
  form: ProviderFormState;
  /**
   * `create` shows the full field set (id + api_key). `edit` hides the immutable
   * `id` (Spec §3.1: "editable only on create") and the `api_key` field, which
   * has its own dedicated Update Key flow on the provider row.
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
      {mode === "create" && (
        <SettingsRow
          layout="vertical"
          label="Provider ID"
          description="Unique identifier for this provider (e.g. deepseek-prod). Cannot be changed after creation."
          control={
            <input
              type="text"
              className="input"
              aria-label="Provider ID"
              placeholder="deepseek-prod"
              value={form.id}
              onChange={(e) => onChange({ id: e.currentTarget.value })}
            />
          }
        />
      )}
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
      <SettingsRow
        layout="vertical"
        label="API Key Env Name"
        description="Environment variable name that holds the API key at runtime."
        control={
          <input
            type="text"
            className="input"
            aria-label="API Key Env Name"
            placeholder="DEEPSEEK_API_KEY"
            value={form.api_key_env_name}
            onChange={(e) =>
              onChange({ api_key_env_name: e.currentTarget.value })
            }
          />
        }
      />
      <SettingsRow
        layout="vertical"
        label="Models"
        description="Comma-separated list of model IDs available through this provider."
        control={
          <input
            type="text"
            className="input"
            aria-label="Models"
            placeholder="deepseek-chat, deepseek-reasoner"
            value={form.models}
            onChange={(e) => onChange({ models: e.currentTarget.value })}
          />
        }
      />
      {mode === "create" && (
        <SettingsRow
          layout="vertical"
          label="API Key"
          description="The actual API key. Stored in the system keychain, never written to settings.toml."
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
  // Inline edit mode (Plan Task 6: editable on update, id immutable).
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

  // Edit mode replaces the read-only field view with the inline edit form.
  // The toggle / api-key-update / test / delete actions remain available in
  // view mode; the Edit button is the entry point into edit mode.
  if (isEditing) {
    return (
      <div className="settings-panel">
        <div className="settings-row">
          <div>
            <h3>{`Editing ${provider.name}`}</h3>
            <p>Update provider metadata. Provider ID is immutable. API key has its own Update Key flow when not editing.</p>
          </div>
        </div>
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
        label="Base URL"
        control={<SettingsValue value={provider.base_url} tone="muted" />}
      />
      <SettingsRow
        label="Models"
        control={
          <SettingsValue value={modelsLabel(provider.models)} tone="default" />
        }
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
  // provider id swaps that row into the edit form (Plan Task 6: editable on
  // update; id is immutable and omitted from the edit form).
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
    const payload: ProviderCreateRequestDto = {
      id: form.id.trim(),
      name: form.name.trim(),
      base_url: form.base_url.trim(),
      api_key_env_name: form.api_key_env_name.trim(),
      base_url_env_name: null,
      models: parseModels(form.models),
      api_key: form.api_key.length > 0 ? form.api_key : null,
    };
    setMutationError(null);
    createProvider.mutate(payload, {
      onSuccess: () => {
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "provider.added",
          message: "Provider added",
          details: { id: payload.id, name: payload.name },
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
      id: provider.id, // immutable; not rendered in edit mode
      name: provider.name,
      base_url: provider.base_url,
      api_key_env_name: provider.api_key_env_name,
      models: provider.models.join(", "),
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
    // Send all editable fields (simpler than per-field null patching; the
    // backend treats present fields as "set to this value"). enabled / api_key
    // / base_url_env_name are left null (unchanged) — enabled has its own
    // toggle and api_key has its own Update Key flow.
    updateProvider.mutate(
      {
        id,
        name: editForm.name.trim(),
        base_url: editForm.base_url.trim(),
        api_key_env_name: editForm.api_key_env_name.trim(),
        base_url_env_name: null,
        models: parseModels(editForm.models),
        enabled: null,
        api_key: null,
      },
      {
        onSuccess: () => {
          setEditingId(null);
        },
        onError: (err: unknown) => {
          setMutationError((err as Error)?.message ?? String(err));
        },
      },
    );
  }, [editForm, editingId, updateProvider]);

  const handleEditCancel = useCallback(() => {
    setEditingId(null);
    setEditForm(EMPTY_FORM);
  }, []);

  const handleToggleEnabled = useCallback(
    (provider: ProviderDto) => {
      setMutationError(null);
      updateProvider.mutate(
        {
          id: provider.id,
          enabled: !provider.enabled,
          name: null,
          base_url: null,
          api_key_env_name: null,
          base_url_env_name: null,
          models: null,
          api_key: null,
        },
        {
          onError: (err: unknown) => {
            setMutationError((err as Error)?.message ?? String(err));
          },
        },
      );
    },
    [updateProvider],
  );

  const handleUpdateApiKey = useCallback(
    (id: string, apiKey: string) => {
      setMutationError(null);
      updateProvider.mutate(
        {
          id,
          api_key: apiKey,
          enabled: null,
          name: null,
          base_url: null,
          api_key_env_name: null,
          base_url_env_name: null,
          models: null,
        },
        {
          onError: (err: unknown) => {
            setMutationError((err as Error)?.message ?? String(err));
          },
        },
      );
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
      </div>
    </div>
  );
}
