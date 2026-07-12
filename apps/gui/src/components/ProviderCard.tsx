import { useEffect, useState } from "react";
import type {
  ModelCatalogEntryDto,
  ModelCreateRequestDto,
  ModelUpdateRequestDto,
  ProviderDto,
  ProviderKind,
  ProviderUpdateRequestDto,
} from "@busytok/protocol-types";
import type { useProviderMutations } from "../api/useBusytokData";
import { errorMessage, parseTags, validateBaseUrl, KIND_LABELS, KIND_OPTIONS } from "../pages/providerFormUtils";
import { reportFrontendEventSafely } from "../logging/safeReporter";
import { ConfirmDialog } from "./ConfirmDialog";

/** Test-connection result surfaced from the page to the card (f3). */
export interface TestConnectionResult {
  ok: boolean;
  error: string | null;
}

interface ProviderCardProps {
  provider: ProviderDto;
  models: ModelCatalogEntryDto[];
  isModelsLoading: boolean;
  providerMutations: ReturnType<typeof useProviderMutations>;
  onEdit: () => void;
  onTestConnection: (id: string) => void;
  onDelete: (provider: ProviderDto) => Promise<void>;
  onModelCreate: (payload: ModelCreateRequestDto) => Promise<void>;
  onModelUpdate: (model: ModelCatalogEntryDto, patch: ModelUpdateRequestDto) => Promise<void>;
  onModelTagsUpdate: (model: ModelCatalogEntryDto, tags: string[]) => Promise<void>;
  onModelDelete: (model: ModelCatalogEntryDto) => Promise<void>;
  isEditing?: boolean;
  onCancelEdit?: () => void;
  /** Latest test-connection result for this provider (undefined = not tested yet). */
  testResult?: TestConnectionResult;
}

interface NewModelDraft {
  modelId: string;
  tags: string;
}

interface ModelEditDraft {
  display_name: string;
  tags: string;
  context_window: number;
  max_tokens: number;
  reasoning: boolean;
  enabled: boolean;
}

interface ProviderEditDraft {
  name: string;
  base_url: string;
  api_key: string;
  provider_kind: ProviderKind;
}

/** Which confirm dialog (if any) is currently open. */
type ConfirmState =
  | { kind: "none" }
  | { kind: "provider-delete" }
  | { kind: "model-delete"; model: ModelCatalogEntryDto }
  | { kind: "cancel-provider-edit" }
  | { kind: "cancel-model-edit" };

function toEditDraft(m: ModelCatalogEntryDto): ModelEditDraft {
  return {
    display_name: m.display_name ?? "",
    tags: m.tags.join(", "),
    context_window: m.context_window ?? 200000,
    max_tokens: m.max_tokens ?? 8192,
    reasoning: m.reasoning ?? false,
    enabled: m.model_enabled,
  };
}

export function ProviderCard({
  provider,
  models,
  isModelsLoading,
  providerMutations,
  onEdit,
  onTestConnection,
  onDelete,
  onModelCreate,
  onModelUpdate,
  onModelTagsUpdate,
  onModelDelete,
  isEditing = false,
  onCancelEdit,
  testResult,
}: ProviderCardProps) {
  const [showCreateModel, setShowCreateModel] = useState(false);
  const [newModelDraft, setNewModelDraft] = useState<NewModelDraft>({ modelId: "", tags: "" });
  const [editingModelDbId, setEditingModelDbId] = useState<string | null>(null);
  const [editDraft, setEditDraft] = useState<ModelEditDraft | null>(null);
  const [providerEditDraft, setProviderEditDraft] = useState<ProviderEditDraft | null>(null);
  const [modelFormError, setModelFormError] = useState<string | null>(null);
  const [providerSaveError, setProviderSaveError] = useState<string | null>(null);
  const [editUrlError, setEditUrlError] = useState<string | null>(null);
  const [confirmState, setConfirmState] = useState<ConfirmState>({ kind: "none" });
  const [deleteInFlight, setDeleteInFlight] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [modelFormInFlight, setModelFormInFlight] = useState(false);
  // Brief success notice shown in view mode after provider save succeeds.
  // Consistent with test-connection result feedback. Auto-dismisses after 3s.
  const [providerSaveSuccess, setProviderSaveSuccess] = useState(false);

  useEffect(() => {
    if (!providerSaveSuccess) return;
    const timer = setTimeout(() => setProviderSaveSuccess(false), 3000);
    return () => clearTimeout(timer);
  }, [providerSaveSuccess]);

  // Synchronously initialize/clear provider edit draft when isEditing changes.
  // Uses the React-recommended "adjusting state during render" pattern to
  // avoid a view-mode flash on the first render after entering edit mode.
  // See: https://react.dev/reference/react/useState#storing-information-from-previous-renders
  const [prevEditing, setPrevEditing] = useState(false);
  if (isEditing !== prevEditing) {
    setPrevEditing(isEditing);
    if (isEditing) {
      setProviderEditDraft({
        name: provider.name,
        base_url: provider.base_url,
        api_key: "",
        provider_kind: provider.provider_kind,
      });
      setProviderSaveError(null);
      setEditUrlError(null);
      setProviderSaveSuccess(false);
    } else {
      setProviderEditDraft(null);
      setProviderSaveError(null);
      setEditUrlError(null);
    }
  }

  // Any provider or model mutation in-flight → disable action buttons (f4).
  const isProviderMutationPending =
    providerMutations.updateProvider.isPending || providerMutations.deleteProvider.isPending;
  const isAnyMutationInFlight =
    isProviderMutationPending || modelFormInFlight || deleteInFlight;

  // Dirty-form checks: detect unsaved changes before allowing cancel.
  const isProviderEditDirty = providerEditDraft !== null && (
    providerEditDraft.name !== provider.name ||
    providerEditDraft.base_url !== provider.base_url ||
    providerEditDraft.api_key !== "" ||
    providerEditDraft.provider_kind !== provider.provider_kind
  );
  const isModelEditDirty = editDraft !== null && editingModelDbId !== null && (() => {
    const model = models.find((m) => m.model_db_id === editingModelDbId);
    if (!model) return false;
    const orig = toEditDraft(model);
    return (
      editDraft.display_name !== orig.display_name ||
      editDraft.tags !== orig.tags ||
      editDraft.context_window !== orig.context_window ||
      editDraft.max_tokens !== orig.max_tokens ||
      editDraft.reasoning !== orig.reasoning ||
      editDraft.enabled !== orig.enabled
    );
  })();

  const handleSaveProviderEdit = () => {
    if (!providerEditDraft) return;
    setProviderSaveError(null);
    // Three-state api_key: empty string = omit (undefined → no change).
    // The "clear key" flow (api_key = null) is out of scope for v1.
    // Typing a new value = update.
    const patch: ProviderUpdateRequestDto = {
      id: provider.id,
      name: providerEditDraft.name,
      base_url: providerEditDraft.base_url,
      enabled: null, // not editable in v1 edit form → no change
      provider_kind: providerEditDraft.provider_kind,
      api_key: providerEditDraft.api_key.length > 0 ? providerEditDraft.api_key : undefined,
    };
    providerMutations.updateProvider.mutate(patch, {
      onSuccess: () => {
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "provider.updated",
          message: "Provider updated",
          details: { id: provider.id, name: providerEditDraft.name },
        });
        setProviderSaveSuccess(true);
        onCancelEdit?.();
      },
      onError: (err: Error) => {
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "provider.update.failed",
          message: "Provider update failed",
          details: { id: provider.id, error: err.message },
        });
        setProviderSaveError(err.message ?? "Save failed");
      },
    });
  };

  const handleProviderDelete = () => {
    setDeleteError(null);
    setConfirmState({ kind: "provider-delete" });
  };

  const handleModelDelete = (model: ModelCatalogEntryDto) => {
    setDeleteError(null);
    setConfirmState({ kind: "model-delete", model });
  };

  // Cancel buttons: check dirty state before discarding.
  // Also clear stale deleteError so it doesn't leak into cancel-confirm dialogs.
  const handleProviderEditCancel = () => {
    if (isProviderEditDirty) {
      setDeleteError(null);
      setConfirmState({ kind: "cancel-provider-edit" });
    } else {
      onCancelEdit?.();
    }
  };

  const handleModelEditCancel = () => {
    if (isModelEditDirty) {
      setDeleteError(null);
      setConfirmState({ kind: "cancel-model-edit" });
    } else {
      cancelModelEdit();
      setModelFormError(null);
    }
  };

  const handleConfirm = async () => {
    // Cancel-confirm cases: synchronous, no loading/error needed.
    if (confirmState.kind === "cancel-provider-edit") {
      setConfirmState({ kind: "none" });
      onCancelEdit?.();
      return;
    }
    if (confirmState.kind === "cancel-model-edit") {
      setConfirmState({ kind: "none" });
      cancelModelEdit();
      setModelFormError(null);
      return;
    }
    // Delete cases: async with loading/error.
    setDeleteError(null);
    setDeleteInFlight(true);
    try {
      if (confirmState.kind === "provider-delete") {
        await onDelete(provider);
      } else if (confirmState.kind === "model-delete") {
        await onModelDelete(confirmState.model);
      }
      setConfirmState({ kind: "none" });
    } catch (err) {
      setDeleteError(errorMessage(err, "Delete failed"));
    } finally {
      setDeleteInFlight(false);
    }
  };

  const handleCreateSubmit = async () => {
    if (!newModelDraft.modelId.trim()) return;
    const payload: ModelCreateRequestDto = {
      provider_id: provider.id,
      model_id: newModelDraft.modelId.trim(),
      display_name: newModelDraft.modelId.trim(),
      context_window: 200000,
      max_tokens: 8192,
      reasoning: true,
      enabled: true,
      tags: parseTags(newModelDraft.tags),
    };
    setModelFormInFlight(true);
    try {
      await onModelCreate(payload);
      setNewModelDraft({ modelId: "", tags: "" });
      setShowCreateModel(false);
      setModelFormError(null);
    } catch (err) {
      setModelFormError(errorMessage(err, "Create failed"));
    } finally {
      setModelFormInFlight(false);
    }
  };

  const startModelEdit = (m: ModelCatalogEntryDto) => {
    setEditingModelDbId(m.model_db_id);
    setEditDraft(toEditDraft(m));
    setModelFormError(null);
  };

  const cancelModelEdit = () => {
    setEditingModelDbId(null);
    setEditDraft(null);
  };

  const handleEditSubmit = async (m: ModelCatalogEntryDto) => {
    if (!editDraft) return;
    // Build the full ModelUpdateRequestDto with null for unchanged fields.
    // The DTO uses `T | null` (not optional `?`), and serde deserializes
    // null to None — wire-compatible with omitting, but type-safe.
    const patch: ModelUpdateRequestDto = {
      id: m.model_db_id,
      // Empty display_name = no change (prevents accidental clear).
      display_name: editDraft.display_name && editDraft.display_name !== (m.display_name ?? "") ? editDraft.display_name : null,
      context_window: editDraft.context_window !== (m.context_window ?? 200000) ? editDraft.context_window : null,
      max_tokens: editDraft.max_tokens !== (m.max_tokens ?? 8192) ? editDraft.max_tokens : null,
      reasoning: editDraft.reasoning !== (m.reasoning ?? false) ? editDraft.reasoning : null,
      enabled: editDraft.enabled !== m.model_enabled ? editDraft.enabled : null,
    };
    const hasFieldChanges =
      patch.display_name !== null ||
      patch.context_window !== null ||
      patch.max_tokens !== null ||
      patch.reasoning !== null ||
      patch.enabled !== null;
    // Tags are updated via a separate RPC. Compare parsed arrays to avoid
    // false positives from whitespace differences in the comma-separated string.
    const newTags = parseTags(editDraft.tags);
    const sameTags =
      m.tags.length === newTags.length && m.tags.every((t, i) => t === newTags[i]);
    const hasTagChanges = !sameTags;

    if (!hasFieldChanges && !hasTagChanges) {
      cancelModelEdit();
      return;
    }
    setModelFormInFlight(true);
    try {
      if (hasFieldChanges) {
        await onModelUpdate(m, patch);
      }
      if (hasTagChanges) {
        await onModelTagsUpdate(m, newTags);
      }
      cancelModelEdit();
      setModelFormError(null);
    } catch (err) {
      setModelFormError(errorMessage(err, "Save failed"));
    } finally {
      setModelFormInFlight(false);
    }
  };

  // ─── Confirm dialog content (f1) ─────────────────────────────────────
  const confirmDialog =
    confirmState.kind === "provider-delete" ? {
      title: "Delete Provider",
      body: `Delete provider "${provider.name}" and all associated models?`,
      detail: "Subagents bound to this provider or its models will fail on next delegate and must be rebound manually.",
      confirmLabel: "Delete",
    } : confirmState.kind === "model-delete" ? {
      title: "Delete Model",
      body: `Delete model "${confirmState.model.model_id}"?`,
      detail: "Subagents bound to this model will fail on next delegate.",
      confirmLabel: "Delete",
    } : confirmState.kind === "cancel-provider-edit" ? {
      title: "Discard Changes",
      body: "Provider has unsaved changes. Discard them?",
      confirmLabel: "Discard",
    } : confirmState.kind === "cancel-model-edit" ? {
      title: "Discard Changes",
      body: "Model has unsaved changes. Discard them?",
      confirmLabel: "Discard",
    } : null;

  // ─── Edit mode render ────────────────────────────────────────────────
  // Header fields become editable inputs; models section stays visible but
  // all model operations are disabled with a notice (per spec §4).
  if (isEditing && providerEditDraft) {
    const draft = providerEditDraft;
    return (
      <div className="provider-card">
        <div className="provider-card__header">
          <div className="provider-card__identity">
            <div className="field-group">
              <label className="field-label" htmlFor={`prov-name-${provider.id}`}>Name</label>
              <input
                id={`prov-name-${provider.id}`}
                className="field-input"
                type="text"
                value={draft.name}
                onChange={(e) => setProviderEditDraft({ ...draft, name: e.target.value })}
              />
            </div>
            <div className="field-group">
              <label className="field-label" htmlFor={`prov-kind-${provider.id}`}>Kind</label>
              <select
                id={`prov-kind-${provider.id}`}
                className="field-select"
                value={draft.provider_kind}
                onChange={(e) => setProviderEditDraft({ ...draft, provider_kind: e.target.value as ProviderKind })}
              >
                {KIND_OPTIONS.map((k) => (
                  <option key={k} value={k}>{KIND_LABELS[k] ?? k}</option>
                ))}
              </select>
            </div>
          </div>
          <div className="provider-card__actions provider-card__actions--end">
            <button type="button" className="btn btn--primary btn--sm" onClick={handleSaveProviderEdit} disabled={isProviderMutationPending || editUrlError !== null}>Save</button>
            <button type="button" className="btn btn--secondary btn--sm" onClick={handleProviderEditCancel} disabled={isProviderMutationPending}>Cancel</button>
          </div>
        </div>
        <div className="provider-card__body">
          <div className="field-group">
            <label className="field-label" htmlFor={`prov-url-${provider.id}`}>Base URL</label>
            <input
              id={`prov-url-${provider.id}`}
              className="field-input"
              type="text"
              value={draft.base_url}
              aria-invalid={editUrlError !== null}
              aria-describedby={editUrlError ? `prov-url-error-${provider.id}` : undefined}
              onChange={(e) => setProviderEditDraft({ ...draft, base_url: e.target.value })}
              onBlur={() => setEditUrlError(validateBaseUrl(draft.base_url))}
            />
            {editUrlError && (
              <div id={`prov-url-error-${provider.id}`} className="field-error" role="alert">{editUrlError}</div>
            )}
          </div>
          <div className="field-group">
            <label className="field-label" htmlFor={`prov-key-${provider.id}`}>New API Key (leave empty to keep current)</label>
            <input
              id={`prov-key-${provider.id}`}
              className="field-input"
              type="password"
              autoComplete="off"
              placeholder="new api key (optional)"
              value={draft.api_key}
              onChange={(e) => setProviderEditDraft({ ...draft, api_key: e.target.value })}
            />
          </div>
          <dl className="provider-card__metadata">
            <div className="provider-card__metadata-row">
              <dt className="provider-card__metadata-label">ID</dt>
              <dd className="provider-card__metadata-value provider-card__metadata-value--mono">{provider.id}</dd>
            </div>
          </dl>
          {providerSaveError && (
            <div className="provider-card__test-result provider-card__test-result--fail" role="alert">
              {providerSaveError}
            </div>
          )}
        </div>
        <div className="provider-card__notice" role="status">Editing provider details. Model operations are temporarily unavailable.</div>
        <div className="provider-card__models provider-card__models--disabled">
          <div className="provider-card__models-header">
            <span className="provider-card__section-label">Models</span>
            <button type="button" className="btn btn--secondary btn--sm" disabled>+ Add Model</button>
          </div>
          {models.map((m) => (
            <div key={m.model_db_id} className="model-row">
              <span className="model-row__name">{m.model_id}</span>
              <span className={`status-indicator status-indicator--inline ${m.model_enabled ? "status-indicator--ok" : "status-indicator--muted"}`}>
                <span className="status-indicator__dot" />
                {m.model_enabled ? "Enabled" : "Disabled"}
              </span>
              <div className="model-row__actions">
                <button type="button" className="btn btn--secondary btn--sm" disabled>Edit</button>
                <button type="button" className="btn btn--danger-quiet btn--sm" disabled>Delete</button>
              </div>
            </div>
          ))}
        </div>
        {confirmDialog && (
          <ConfirmDialog
            open
            title={confirmDialog.title}
            body={confirmDialog.body}
            detail={confirmDialog.detail}
            confirmLabel={confirmDialog.confirmLabel}
            loading={deleteInFlight}
            error={confirmState.kind === "provider-delete" || confirmState.kind === "model-delete" ? deleteError : null}
            onConfirm={handleConfirm}
            onCancel={() => setConfirmState({ kind: "none" })}
          />
        )}
      </div>
    );
  }

  // ─── View mode render ────────────────────────────────────────────────
  return (
    <div className="provider-card">
      <div className="provider-card__header">
        <div className="provider-card__identity">
          <span className="provider-card__name">{provider.name}</span>
          <span className="chip chip--kind">{KIND_LABELS[provider.provider_kind] ?? provider.provider_kind}</span>
        </div>
        <span className={`status-indicator ${provider.enabled ? "status-indicator--ok" : "status-indicator--muted"}`}>
          <span className="status-indicator__dot" />
          {provider.enabled ? "Enabled" : "Disabled"}
        </span>
        <div className="provider-card__actions provider-card__actions--end">
          <button type="button" className="btn btn--secondary btn--sm" onClick={onEdit} disabled={isProviderMutationPending}>Edit</button>
          <button type="button" className="btn btn--secondary btn--sm" onClick={() => onTestConnection(provider.id)} disabled={providerMutations.testConnection.isPending}>Test Connection</button>
          <button type="button" className="btn btn--danger-quiet btn--sm" onClick={handleProviderDelete} disabled={isProviderMutationPending}>Delete</button>
        </div>
      </div>
      <div className="provider-card__body">
        <dl className="provider-card__metadata">
          <div className="provider-card__metadata-row">
            <dt className="provider-card__metadata-label">Base URL</dt>
            <dd className="provider-card__metadata-value">{provider.base_url}</dd>
          </div>
          <div className="provider-card__metadata-row">
            <dt className="provider-card__metadata-label">ID</dt>
            <dd className="provider-card__metadata-value provider-card__metadata-value--mono">{provider.id}</dd>
          </div>
        </dl>
        {testResult && (
          <div
            className={`provider-card__test-result ${testResult.ok ? "provider-card__test-result--ok" : "provider-card__test-result--fail"}`}
            role={testResult.ok ? "status" : "alert"}
          >
            {testResult.ok ? "Connection successful" : `Connection failed: ${testResult.error ?? "Unknown error"}`}
          </div>
        )}
        {providerSaveSuccess && (
          <div className="provider-card__test-result provider-card__test-result--ok" role="status">
            Saved
          </div>
        )}
      </div>
      <div className="provider-card__models">
        <div className="provider-card__models-header">
          <span className="provider-card__section-label">Models</span>
          <button type="button" className="btn btn--secondary btn--sm" onClick={() => setShowCreateModel((v) => !v)} disabled={isAnyMutationInFlight}>+ Add Model</button>
        </div>
        {showCreateModel && (
          <div className="model-row model-row__edit-form">
            <div className="field-group">
              <label className="field-label" htmlFor={`new-model-id-${provider.id}`}>Model Name</label>
              <input
                id={`new-model-id-${provider.id}`}
                className="field-input"
                type="text"
                placeholder="model name (e.g. deepseek-chat)"
                value={newModelDraft.modelId}
                onChange={(e) => setNewModelDraft((d) => ({ ...d, modelId: e.target.value }))}
              />
            </div>
            <div className="field-group">
              <label className="field-label" htmlFor={`new-model-tags-${provider.id}`}>Tags</label>
              <input
                id={`new-model-tags-${provider.id}`}
                className="field-input"
                type="text"
                placeholder="tags (comma-separated, optional)"
                value={newModelDraft.tags}
                onChange={(e) => setNewModelDraft((d) => ({ ...d, tags: e.target.value }))}
              />
            </div>
            <div className="provider-card__actions">
              <button type="button" className="btn btn--primary btn--sm" onClick={handleCreateSubmit} disabled={isAnyMutationInFlight}>Save</button>
              <button type="button" className="btn btn--secondary btn--sm" onClick={() => { setShowCreateModel(false); setNewModelDraft({ modelId: "", tags: "" }); setModelFormError(null); }}>Cancel</button>
            </div>
            {modelFormError && (
              <div className="field-error" role="alert">{modelFormError}</div>
            )}
          </div>
        )}
        {isModelsLoading ? (
          <div className="provider-card__empty">Loading…</div>
        ) : models.length === 0 && !showCreateModel ? (
          <div className="provider-card__empty">No models</div>
        ) : (
          models.map((m) => (
            <div key={m.model_db_id} className="model-row">
              {editingModelDbId === m.model_db_id && editDraft ? (
                <div className="model-row__edit-form">
                  <div className="field-group">
                    <label className="field-label" htmlFor={`edit-name-${m.model_db_id}`}>Display Name</label>
                    <input
                      id={`edit-name-${m.model_db_id}`}
                      className="field-input"
                      type="text"
                      value={editDraft.display_name}
                      onChange={(e) => setEditDraft({ ...editDraft, display_name: e.target.value })}
                    />
                  </div>
                  <div className="field-group">
                    <label className="field-label" htmlFor={`edit-tags-${m.model_db_id}`}>Tags</label>
                    <input
                      id={`edit-tags-${m.model_db_id}`}
                      className="field-input"
                      type="text"
                      placeholder="tags (comma-separated)"
                      value={editDraft.tags}
                      onChange={(e) => setEditDraft({ ...editDraft, tags: e.target.value })}
                    />
                  </div>
                  <div className="field-group">
                    <label className="field-label" htmlFor={`edit-ctx-${m.model_db_id}`}>Context Window</label>
                    <input
                      id={`edit-ctx-${m.model_db_id}`}
                      className="field-input"
                      type="number"
                      value={editDraft.context_window}
                      onChange={(e) => setEditDraft({ ...editDraft, context_window: Number(e.target.value) })}
                    />
                  </div>
                  <div className="field-group">
                    <label className="field-label" htmlFor={`edit-max-${m.model_db_id}`}>Max Tokens</label>
                    <input
                      id={`edit-max-${m.model_db_id}`}
                      className="field-input"
                      type="number"
                      value={editDraft.max_tokens}
                      onChange={(e) => setEditDraft({ ...editDraft, max_tokens: Number(e.target.value) })}
                    />
                  </div>
                  <label className="field-label">
                    <input
                      type="checkbox"
                      checked={editDraft.reasoning}
                      onChange={(e) => setEditDraft({ ...editDraft, reasoning: e.target.checked })}
                    />
                    Reasoning
                  </label>
                  <label className="field-label">
                    <input
                      type="checkbox"
                      checked={editDraft.enabled}
                      onChange={(e) => setEditDraft({ ...editDraft, enabled: e.target.checked })}
                    />
                    Enabled
                  </label>
                  <div className="provider-card__actions">
                    <button type="button" className="btn btn--primary btn--sm" onClick={() => handleEditSubmit(m)} disabled={isAnyMutationInFlight}>Save</button>
                    <button type="button" className="btn btn--secondary btn--sm" onClick={handleModelEditCancel} disabled={isAnyMutationInFlight}>Cancel</button>
                  </div>
                  {modelFormError && (
                    <div className="field-error" role="alert">{modelFormError}</div>
                  )}
                </div>
              ) : (
                <>
                  <span className="model-row__name">{m.model_id}</span>
                  {m.tags.length > 0 && (
                    <span className="model-row__tags">
                      {m.tags.map((t) => (
                        <span key={t} className="chip">{t}</span>
                      ))}
                    </span>
                  )}
                  <span className={`status-indicator status-indicator--inline ${m.model_enabled ? "status-indicator--ok" : "status-indicator--muted"}`}>
                    <span className="status-indicator__dot" />
                    {m.model_enabled ? "Enabled" : "Disabled"}
                  </span>
                  <div className="model-row__actions">
                    <button type="button" className="btn btn--secondary btn--sm" onClick={() => startModelEdit(m)} disabled={isAnyMutationInFlight}>Edit</button>
                    <button type="button" className="btn btn--danger-quiet btn--sm" onClick={() => handleModelDelete(m)} disabled={isAnyMutationInFlight}>Delete</button>
                  </div>
                </>
              )}
            </div>
          ))
        )}
      </div>
      {confirmDialog && (
        <ConfirmDialog
          open
          title={confirmDialog.title}
          body={confirmDialog.body}
          detail={confirmDialog.detail}
          confirmLabel={confirmDialog.confirmLabel}
          loading={deleteInFlight}
          error={deleteError}
          onConfirm={handleConfirm}
          onCancel={() => setConfirmState({ kind: "none" })}
        />
      )}
    </div>
  );
}
