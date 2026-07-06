import type {
  ModelCatalogEntryDto,
  ProviderDto,
} from "@busytok/protocol-types";
import type { useProviderMutations, useModelMutations } from "../api/useBusytokData";

interface ProviderCardProps {
  provider: ProviderDto;
  models: ModelCatalogEntryDto[];
  isModelsLoading: boolean;
  providerMutations: ReturnType<typeof useProviderMutations>;
  modelMutations: ReturnType<typeof useModelMutations>;
  onEdit: () => void;
  onTestConnection: (id: string) => void;
  onDelete: (provider: ProviderDto) => void;
}

const KIND_LABEL: Record<string, string> = {
  openai_compatible: "openai",
  anthropic_compatible: "anthropic",
};

export function ProviderCard({
  provider,
  models,
  isModelsLoading,
  onEdit,
  onTestConnection,
  onDelete,
}: ProviderCardProps) {
  const handleDelete = () => {
    const ok = globalThis.confirm(
      "确定删除此 provider 及其关联的所有 models？\n注意：已绑定此 provider/model 的 subagents 将在下次 delegate 时失败，需要手动重新绑定。",
    );
    if (ok) onDelete(provider);
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
          <button type="button" onClick={handleDelete}>删除</button>
        </div>
      </div>
      <div className="provider-card__info">
        <div>{provider.base_url}</div>
        <div style={{ fontFamily: "monospace" }}>ID: {provider.id}</div>
      </div>
      <div className="provider-card__models">
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
          <strong>Models</strong>
          <button type="button">+ Add Model</button>
        </div>
        {isModelsLoading ? (
          <div>加载中…</div>
        ) : models.length === 0 ? (
          <div style={{ color: "var(--color-text-muted)", fontSize: "0.85rem" }}>暂无 model</div>
        ) : (
          models.map((m) => (
            <div key={m.model_db_id} className="model-row">
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
                <button type="button">编辑</button>
                <button type="button">删除</button>
              </div>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
