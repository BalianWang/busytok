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
}: ProviderCardProps) {
  const [showCreateModel, setShowCreateModel] = useState(false);
  const [newModelDraft, setNewModelDraft] = useState<NewModelDraft>({ modelId: "", tags: "" });
  const [editingModelDbId, setEditingModelDbId] = useState<string | null>(null);
  const [editDraft, setEditDraft] = useState<ModelEditDraft | null>(null);
  const [providerEditDraft, setProviderEditDraft] = useState<ProviderEditDraft | null>(null);
  const [modelFormError, setModelFormError] = useState<string | null>(null);

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
    } else {
      setProviderEditDraft(null);
    }
  }, [isEditing, provider.id, provider.name, provider.base_url, provider.provider_kind]);

  const handleSaveProviderEdit = () => {
    if (!providerEditDraft) return;
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
      },
    });
  };

  const handleProviderDelete = () => {
    const ok = globalThis.confirm(
      "确定删除此 provider 及其关联的所有 models？\n注意：已绑定此 provider/model 的 subagents 将在下次 delegate 时失败，需要手动重新绑定。",
    );
    if (ok) onDelete(provider);
  };

  const handleModelDelete = (model: ModelCatalogEntryDto) => {
    const ok = globalThis.confirm(
      "确定删除此 model？\n注意：已绑定此 model 的 subagents 将在下次 delegate 时失败。",
    );
    if (ok) onModelDelete(model);
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

  // ─── Edit mode render ────────────────────────────────────────────────
  // Header fields become editable inputs; models section stays visible but
  // all model operations are disabled with a notice (per spec §4).
  if (isEditing && providerEditDraft) {
    const draft = providerEditDraft;
    return (
      <div className="provider-card">
        <div className="provider-card__header">
          <input
            type="text"
            value={draft.name}
            onChange={(e) => setProviderEditDraft({ ...draft, name: e.target.value })}
          />
          <select
            value={draft.provider_kind}
            onChange={(e) => setProviderEditDraft({ ...draft, provider_kind: e.target.value as ProviderKind })}
          >
            <option value="openai_compatible">openai_compatible</option>
            <option value="anthropic_compatible">anthropic_compatible</option>
          </select>
          <div style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
            <button type="button" onClick={handleSaveProviderEdit}>保存</button>
            <button type="button" onClick={onCancelEdit}>取消</button>
          </div>
        </div>
        <div style={{ padding: 16, display: "flex", flexDirection: "column", gap: 12 }}>
          <label>
            Base URL
            <input
              type="text"
              value={draft.base_url}
              onChange={(e) => setProviderEditDraft({ ...draft, base_url: e.target.value })}
            />
          </label>
          <label>
            New API Key (leave empty to keep current)
            <input
              type="password"
              placeholder="new api key (optional)"
              value={draft.api_key}
              onChange={(e) => setProviderEditDraft({ ...draft, api_key: e.target.value })}
            />
          </label>
          <div style={{ fontFamily: "monospace", fontSize: "0.85rem", color: "var(--color-text-muted)" }}>
            ID: {provider.id}
          </div>
        </div>
        <div className="provider-card__notice">正在编辑 Provider 信息，Models 操作暂不可用</div>
        <div className="provider-card__models provider-card__models--disabled">
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
            <strong>Models</strong>
            <button type="button" disabled>+ Add Model</button>
          </div>
          {models.map((m) => (
            <div key={m.model_db_id} className="model-row">
              <span className="model-row__name">{m.model_id}</span>
              <span>{m.model_enabled ? "●enabled" : "○disabled"}</span>
              <div style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
                <button type="button" disabled>编辑</button>
                <button type="button" disabled>删除</button>
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
        <div style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
          <button type="button" onClick={onEdit}>编辑</button>
          <button type="button" onClick={() => onTestConnection(provider.id)}>测试连接</button>
          <button type="button" onClick={handleProviderDelete}>删除</button>
        </div>
      </div>
      <div className="provider-card__info">
        <div>{provider.base_url}</div>
        <div style={{ fontFamily: "monospace" }}>ID: {provider.id}</div>
      </div>
      <div className="provider-card__models">
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
          <strong>Models</strong>
          <button type="button" onClick={() => setShowCreateModel((v) => !v)}>+ Add Model</button>
        </div>
        {showCreateModel && (
          <div className="model-row" style={{ flexDirection: "column", alignItems: "stretch", gap: 6 }}>
            <input
              type="text"
              placeholder="model name (e.g. deepseek-chat)"
              value={newModelDraft.modelId}
              onChange={(e) => setNewModelDraft((d) => ({ ...d, modelId: e.target.value }))}
            />
            <input
              type="text"
              placeholder="tags (comma-separated, optional)"
              value={newModelDraft.tags}
              onChange={(e) => setNewModelDraft((d) => ({ ...d, tags: e.target.value }))}
            />
            <div style={{ display: "flex", gap: 8 }}>
              <button type="button" onClick={handleCreateSubmit}>保存</button>
              <button type="button" onClick={() => { setShowCreateModel(false); setNewModelDraft({ modelId: "", tags: "" }); setModelFormError(null); }}>取消</button>
            </div>
            {modelFormError && (
              <div role="alert" style={{ color: "var(--color-status-danger)", fontSize: "0.85rem" }}>{modelFormError}</div>
            )}
          </div>
        )}
        {isModelsLoading ? (
          <div>加载中…</div>
        ) : models.length === 0 && !showCreateModel ? (
          <div style={{ color: "var(--color-text-muted)", fontSize: "0.85rem" }}>暂无 model</div>
        ) : (
          models.map((m) => (
            <div key={m.model_db_id} className="model-row">
              {editingModelDbId === m.model_db_id && editDraft ? (
                <div style={{ width: "100%", flexDirection: "column", alignItems: "stretch", gap: 6, display: "flex" }}>
                  <label>
                    Display Name
                    <input
                      type="text"
                      value={editDraft.display_name}
                      onChange={(e) => setEditDraft({ ...editDraft, display_name: e.target.value })}
                    />
                  </label>
                  <label>
                    Tags
                    <input
                      type="text"
                      placeholder="tags (comma-separated)"
                      value={editDraft.tags}
                      onChange={(e) => setEditDraft({ ...editDraft, tags: e.target.value })}
                    />
                  </label>
                  <label>
                    Context Window
                    <input
                      type="number"
                      value={editDraft.context_window}
                      onChange={(e) => setEditDraft({ ...editDraft, context_window: Number(e.target.value) })}
                    />
                  </label>
                  <label>
                    Max Tokens
                    <input
                      type="number"
                      value={editDraft.max_tokens}
                      onChange={(e) => setEditDraft({ ...editDraft, max_tokens: Number(e.target.value) })}
                    />
                  </label>
                  <label>
                    <input
                      type="checkbox"
                      checked={editDraft.reasoning}
                      onChange={(e) => setEditDraft({ ...editDraft, reasoning: e.target.checked })}
                    />
                    Reasoning
                  </label>
                  <label>
                    <input
                      type="checkbox"
                      checked={editDraft.enabled}
                      onChange={(e) => setEditDraft({ ...editDraft, enabled: e.target.checked })}
                    />
                    Enabled
                  </label>
                  <div style={{ display: "flex", gap: 8 }}>
                    <button type="button" onClick={() => handleEditSubmit(m)}>保存</button>
                    <button type="button" onClick={() => { cancelModelEdit(); setModelFormError(null); }}>取消</button>
                  </div>
                  {modelFormError && (
                    <div role="alert" style={{ color: "var(--color-status-danger)", fontSize: "0.85rem" }}>{modelFormError}</div>
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
                  <div style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
                    <button type="button" onClick={() => startModelEdit(m)}>编辑</button>
                    <button type="button" onClick={() => handleModelDelete(m)}>删除</button>
                  </div>
                </>
              )}
            </div>
          ))
        )}
      </div>
    </div>
  );
}
