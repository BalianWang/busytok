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
import { parseTags } from "../pages/providerFormUtils";
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
  onDelete: (provider: ProviderDto) => void;
  onModelCreate: (payload: ModelCreateRequestDto) => Promise<void>;
  onModelUpdate: (model: ModelCatalogEntryDto, patch: ModelUpdateRequestDto) => Promise<void>;
  onModelTagsUpdate: (model: ModelCatalogEntryDto, tags: string[]) => Promise<void>;
  onModelDelete: (model: ModelCatalogEntryDto) => void;
  isEditing?: boolean;
  onCancelEdit?: () => void;
  /** Latest test-connection result for this provider (undefined = not tested yet). */
  testResult?: TestConnectionResult;
}

const KIND_LABEL: Record<string, string> = {
  openai_compatible: "openai",
  anthropic_compatible: "anthropic",
};

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
  | { kind: "model-delete"; model: ModelCatalogEntryDto };

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
  const [confirmState, setConfirmState] = useState<ConfirmState>({ kind: "none" });

  // Initialize/clear provider edit draft via useEffect — never setState during
  // render (React 19 + StrictMode render phase must be pure).
  useEffect(() => {
    if (isEditing) {
      setProviderEditDraft({
        name: provider.name,
        base_url: provider.base_url,
        api_key: "",
        provider_kind: provider.provider_kind,
      });
      setProviderSaveError(null);
    } else {
      setProviderEditDraft(null);
      setProviderSaveError(null);
    }
  }, [isEditing, provider.id, provider.name, provider.base_url, provider.provider_kind]);

  // Any provider mutation in-flight → disable action buttons (f4).
  const isProviderMutationPending =
    providerMutations.updateProvider.isPending || providerMutations.deleteProvider.isPending;

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
        onCancelEdit?.();
      },
      onError: (err: Error) => {
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "provider.update.failed",
          message: "Provider update failed",
          details: { id: provider.id, error: err.message },
        });
        setProviderSaveError(err.message ?? "保存失败");
      },
    });
  };

  const handleProviderDelete = () => {
    setConfirmState({ kind: "provider-delete" });
  };

  const handleModelDelete = (model: ModelCatalogEntryDto) => {
    setConfirmState({ kind: "model-delete", model });
  };

  const handleConfirmDelete = () => {
    if (confirmState.kind === "provider-delete") {
      onDelete(provider);
    } else if (confirmState.kind === "model-delete") {
      onModelDelete(confirmState.model);
    }
    setConfirmState({ kind: "none" });
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
    try {
      await onModelCreate(payload);
      setNewModelDraft({ modelId: "", tags: "" });
      setShowCreateModel(false);
      setModelFormError(null);
    } catch (err: any) {
      setModelFormError(err.message ?? "创建失败");
    }
  };

  const startModelEdit = (m: ModelCatalogEntryDto) => {
    setEditingModelDbId(m.model_db_id);
    setEditDraft(toEditDraft(m));
  };

  const cancelModelEdit = () => {
    setEditingModelDbId(null);
    setEditDraft(null);
  };

  const handleEditSubmit = async (m: ModelCatalogEntryDto) => {
    if (!editDraft) return;
    // Single-state Option<T>: only include fields that changed. Omit = no change
    // (serde deserializes both missing and null to None, so omitting is wire-
    // compatible with the DTO's Option<T> contract).
    const patch: Partial<ModelUpdateRequestDto> & { id: string } = { id: m.model_db_id };
    if (editDraft.display_name && editDraft.display_name !== (m.display_name ?? "")) {
      patch.display_name = editDraft.display_name;
    }
    if (editDraft.context_window !== (m.context_window ?? 200000)) {
      patch.context_window = editDraft.context_window;
    }
    if (editDraft.max_tokens !== (m.max_tokens ?? 8192)) {
      patch.max_tokens = editDraft.max_tokens;
    }
    if (editDraft.reasoning !== (m.reasoning ?? false)) {
      patch.reasoning = editDraft.reasoning;
    }
    if (editDraft.enabled !== m.model_enabled) {
      patch.enabled = editDraft.enabled;
    }
    try {
      if (Object.keys(patch).length > 1) {
        // More than just `id` → there are field updates.
        await onModelUpdate(m, patch as ModelUpdateRequestDto);
      }
      // Tags are updated via a separate RPC. Compare parsed arrays to avoid
      // false positives from whitespace differences in the comma-separated string.
      const newTags = parseTags(editDraft.tags);
      const sameTags =
        m.tags.length === newTags.length && m.tags.every((t, i) => t === newTags[i]);
      if (!sameTags) {
        await onModelTagsUpdate(m, newTags);
      }
      cancelModelEdit();
      setModelFormError(null);
    } catch (err: any) {
      setModelFormError(err.message ?? "保存失败");
    }
  };

  // ─── Confirm dialog content (f1) ─────────────────────────────────────
  const confirmDialog =
    confirmState.kind === "provider-delete" ? {
      title: "删除 Provider",
      body: "确定删除此 provider 及其关联的所有 models？",
      detail: "注意：已绑定此 provider/model 的 subagents 将在下次 delegate 时失败，需要手动重新绑定。",
      confirmLabel: "删除",
    } : confirmState.kind === "model-delete" ? {
      title: "删除 Model",
      body: "确定删除此 model？",
      detail: "注意：已绑定此 model 的 subagents 将在下次 delegate 时失败。",
      confirmLabel: "删除",
    } : null;

  // ─── Edit mode render ────────────────────────────────────────────────
  // Header fields become editable inputs; models section stays visible but
  // all model operations are disabled with a notice (per spec §4).
  if (isEditing && providerEditDraft) {
    const draft = providerEditDraft;
    return (
      <div className="provider-card">
        <div className="provider-card__header">
          <div className="field-group">
            <label className="field-label" htmlFor={`prov-name-${provider.id}`}>名称</label>
            <input
              id={`prov-name-${provider.id}`}
              className="field-input"
              type="text"
              value={draft.name}
              onChange={(e) => setProviderEditDraft({ ...draft, name: e.target.value })}
            />
          </div>
          <div className="field-group">
            <label className="field-label" htmlFor={`prov-kind-${provider.id}`}>类型</label>
            <select
              id={`prov-kind-${provider.id}`}
              className="field-select"
              value={draft.provider_kind}
              onChange={(e) => setProviderEditDraft({ ...draft, provider_kind: e.target.value as ProviderKind })}
            >
              <option value="openai_compatible">openai_compatible</option>
              <option value="anthropic_compatible">anthropic_compatible</option>
            </select>
          </div>
          <div className="provider-card__actions provider-card__actions--end">
            <button type="button" className="btn btn--primary" onClick={handleSaveProviderEdit} disabled={isProviderMutationPending}>保存</button>
            <button type="button" className="btn btn--secondary" onClick={onCancelEdit} disabled={isProviderMutationPending}>取消</button>
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
              onChange={(e) => setProviderEditDraft({ ...draft, base_url: e.target.value })}
            />
          </div>
          <div className="field-group">
            <label className="field-label" htmlFor={`prov-key-${provider.id}`}>New API Key (leave empty to keep current)</label>
            <input
              id={`prov-key-${provider.id}`}
              className="field-input"
              type="password"
              placeholder="new api key (optional)"
              value={draft.api_key}
              onChange={(e) => setProviderEditDraft({ ...draft, api_key: e.target.value })}
            />
          </div>
          <div className="provider-card__id">ID: {provider.id}</div>
          {providerSaveError && (
            <div className="provider-card__test-result provider-card__test-result--fail" role="alert">
              {providerSaveError}
            </div>
          )}
        </div>
        <div className="provider-card__notice">正在编辑 Provider 信息，Models 操作暂不可用</div>
        <div className="provider-card__models provider-card__models--disabled">
          <div className="provider-card__models-header">
            <strong>Models</strong>
            <button type="button" className="btn btn--secondary" disabled>+ Add Model</button>
          </div>
          {models.map((m) => (
            <div key={m.model_db_id} className="model-row">
              <span className="model-row__name">{m.model_id}</span>
              <span>{m.model_enabled ? "●enabled" : "○disabled"}</span>
              <div className="provider-card__actions provider-card__actions--end">
                <button type="button" className="btn btn--secondary" disabled>编辑</button>
                <button type="button" className="btn btn--danger" disabled>删除</button>
              </div>
            </div>
          ))}
        </div>
      </div>
    );
  }

  // ─── View mode render ────────────────────────────────────────────────
  return (
    <div className="provider-card">
      <div className="provider-card__header">
        <span className="provider-card__name">{provider.name}</span>
        <span className="chip chip--kind">{KIND_LABEL[provider.provider_kind] ?? provider.provider_kind}</span>
        <span>{provider.enabled ? "● enabled" : "○ disabled"}</span>
        <div className="provider-card__actions provider-card__actions--end">
          <button type="button" className="btn btn--secondary" onClick={onEdit} disabled={isProviderMutationPending}>编辑</button>
          <button type="button" className="btn btn--secondary" onClick={() => onTestConnection(provider.id)} disabled={providerMutations.testConnection.isPending}>测试连接</button>
          <button type="button" className="btn btn--danger" onClick={handleProviderDelete} disabled={isProviderMutationPending}>删除</button>
        </div>
      </div>
      <div className="provider-card__info">
        <div className="provider-card__url">{provider.base_url}</div>
        <div className="provider-card__id">ID: {provider.id}</div>
      </div>
      {testResult && (
        <div
          className={`provider-card__test-result ${testResult.ok ? "provider-card__test-result--ok" : "provider-card__test-result--fail"}`}
          role={testResult.ok ? "status" : "alert"}
        >
          {testResult.ok ? "连接成功" : `连接失败：${testResult.error ?? "未知错误"}`}
        </div>
      )}
      <div className="provider-card__models">
        <div className="provider-card__models-header">
          <strong>Models</strong>
          <button type="button" className="btn btn--secondary" onClick={() => setShowCreateModel((v) => !v)}>+ Add Model</button>
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
              <button type="button" className="btn btn--primary" onClick={handleCreateSubmit}>保存</button>
              <button type="button" className="btn btn--secondary" onClick={() => { setShowCreateModel(false); setNewModelDraft({ modelId: "", tags: "" }); setModelFormError(null); }}>取消</button>
            </div>
            {modelFormError && (
              <div className="field-error" role="alert">{modelFormError}</div>
            )}
          </div>
        )}
        {isModelsLoading ? (
          <div>加载中…</div>
        ) : models.length === 0 && !showCreateModel ? (
          <div className="provider-card__empty">暂无 model</div>
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
                    <button type="button" className="btn btn--primary" onClick={() => handleEditSubmit(m)}>保存</button>
                    <button type="button" className="btn btn--secondary" onClick={() => { cancelModelEdit(); setModelFormError(null); }}>取消</button>
                  </div>
                  {modelFormError && (
                    <div className="field-error" role="alert">{modelFormError}</div>
                  )}
                </div>
              ) : (
                <>
                  <span className="model-row__name">{m.model_id}</span>
                  <span>{m.model_enabled ? "●enabled" : "○disabled"}</span>
                  {m.tags.length > 0 && (
                    <span className="model-row__tags">
                      {m.tags.map((t) => (
                        <span key={t} className="chip">{t}</span>
                      ))}
                    </span>
                  )}
                  <div className="provider-card__actions provider-card__actions--end">
                    <button type="button" className="btn btn--secondary" onClick={() => startModelEdit(m)}>编辑</button>
                    <button type="button" className="btn btn--danger" onClick={() => handleModelDelete(m)}>删除</button>
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
          onConfirm={handleConfirmDelete}
          onCancel={() => setConfirmState({ kind: "none" })}
        />
      )}
    </div>
  );
}
