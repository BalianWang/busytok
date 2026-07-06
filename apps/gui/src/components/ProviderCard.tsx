import { useState } from "react";
import type {
  ModelCatalogEntryDto,
  ModelCreateRequestDto,
  ModelUpdateRequestDto,
  ProviderDto,
} from "@busytok/protocol-types";
import type { useProviderMutations, useModelMutations } from "../api/useBusytokData";
import { parseTags } from "../pages/providerFormUtils";

interface ProviderCardProps {
  provider: ProviderDto;
  models: ModelCatalogEntryDto[];
  isModelsLoading: boolean;
  providerMutations: ReturnType<typeof useProviderMutations>;
  modelMutations: ReturnType<typeof useModelMutations>;
  onEdit: () => void;
  onTestConnection: (id: string) => void;
  onDelete: (provider: ProviderDto) => void;
  onModelCreate: (payload: ModelCreateRequestDto) => void;
  onModelUpdate: (model: ModelCatalogEntryDto, patch: ModelUpdateRequestDto) => void;
  onModelTagsUpdate: (modelId: string, tags: string[]) => void;
  onModelDelete: (model: ModelCatalogEntryDto) => void;
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
  onEdit,
  onTestConnection,
  onDelete,
  onModelCreate,
  onModelUpdate,
  onModelTagsUpdate,
  onModelDelete,
}: ProviderCardProps) {
  const [showCreateModel, setShowCreateModel] = useState(false);
  const [newModelDraft, setNewModelDraft] = useState<NewModelDraft>({ modelId: "", tags: "" });
  const [editingModelDbId, setEditingModelDbId] = useState<string | null>(null);
  const [editDraft, setEditDraft] = useState<ModelEditDraft | null>(null);

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

  const handleCreateSubmit = () => {
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
    onModelCreate(payload);
    setNewModelDraft({ modelId: "", tags: "" });
    setShowCreateModel(false);
  };

  const startModelEdit = (m: ModelCatalogEntryDto) => {
    setEditingModelDbId(m.model_db_id);
    setEditDraft(toEditDraft(m));
  };

  const cancelModelEdit = () => {
    setEditingModelDbId(null);
    setEditDraft(null);
  };

  const handleEditSubmit = (m: ModelCatalogEntryDto) => {
    if (!editDraft) return;
    // Single-state Option<T>: only include fields that changed. Omit = no change
    // (serde deserializes both missing and null to None, so this is wire-compatible
    // with the existing ModelsSection pattern that sends null for unchanged fields).
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
    if (Object.keys(patch).length > 1) {
      // More than just `id` → there are field updates.
      onModelUpdate(m, patch as ModelUpdateRequestDto);
    }
    // Tags are updated via a separate RPC. Compare parsed arrays to avoid
    // false positives from whitespace differences in the comma-separated string.
    const newTags = parseTags(editDraft.tags);
    const sameTags =
      m.tags.length === newTags.length && m.tags.every((t, i) => t === newTags[i]);
    if (!sameTags) {
      onModelTagsUpdate(m.model_id, newTags);
    }
    cancelModelEdit();
  };

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
              <button type="button" onClick={() => { setShowCreateModel(false); setNewModelDraft({ modelId: "", tags: "" }); }}>取消</button>
            </div>
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
                    <button type="button" onClick={cancelModelEdit}>取消</button>
                  </div>
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
