import { useMemo, useState } from "react";
import type {
  ModelCatalogEntryDto,
  ModelCreateRequestDto,
  ModelUpdateRequestDto,
  ProviderDto,
} from "@busytok/protocol-types";
import {
  useModelMutations,
  useModels,
  useProviderMutations,
  useProviders,
} from "../api/useBusytokData";
import { ProviderCard } from "../components/ProviderCard";
import { ProviderCreationForm } from "../components/ProviderCreationForm";
import { reportFrontendEventSafely } from "../logging/safeReporter";

/**
 * ProvidersPage orchestrates the provider/model catalog UI.
 *
 * Architecture (per spec §4 redesign):
 *   - A single `useModels({ includeDisabled: true })` query at the page
 *     level, grouped by `provider_id` into a Map. Each ProviderCard
 *     receives its slice of models via props (1 query vs N-per-card).
 *   - ProviderCard (view + edit modes) renders one provider and its
 *     inline models. Provider edit mode is tracked at the page level so
 *     only one card is editable at a time.
 *   - ProviderCreationForm (toggleable) handles create-provider (+ the
 *     optional sync-create-model partial-success flow). It emits its own
 *     `provider.added` / `provider.add.failed` / `model.added` /
 *     `model.add.failed` events for the sync-create flow.
 *   - The page emits the remaining observability events (spec §8) for
 *     provider delete/test, inline model create/update/tags/delete, and
 *     the symmetric `.failed` events on client exceptions. ProviderCard
 *     emits `provider.updated` / `provider.update.failed` directly
 *     inside its edit-mode save handler (it has the patch context).
 *
 * Event details payloads (spec §8):
 *   - Provider events: `{ id, ... }` (or `{ id, ok, error }` for test)
 *   - Model events: `{ provider_id, model_id, ... }` (NOT `model_db_id`)
 */
export function ProvidersPage() {
  const providersQuery = useProviders();
  const modelsQuery = useModels({ includeDisabled: true });
  const providerMutations = useProviderMutations();
  const modelMutations = useModelMutations();

  const [showCreateForm, setShowCreateForm] = useState(false);
  const [editingProviderId, setEditingProviderId] = useState<string | null>(null);

  // Group models by provider_id once per render. A single useModels query
  // feeds every ProviderCard — avoids N-per-card fetches.
  const modelsByProvider = useMemo(() => {
    const map = new Map<string, ModelCatalogEntryDto[]>();
    for (const m of modelsQuery.data?.models ?? []) {
      const list = map.get(m.provider_id) ?? [];
      list.push(m);
      map.set(m.provider_id, list);
    }
    return map;
  }, [modelsQuery.data]);

  const handleProviderDelete = (provider: ProviderDto) => {
    providerMutations.deleteProvider.mutate(provider.id, {
      onSuccess: () => {
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "provider.deleted",
          message: "Provider deleted",
          details: { id: provider.id, name: provider.name },
        });
      },
      onError: (err: Error) => {
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "provider.delete.failed",
          message: "Provider delete failed",
          details: { id: provider.id, name: provider.name, error: err.message },
        });
      },
    });
  };

  const handleTestConnection = (id: string) => {
    providerMutations.testConnection.mutate(id, {
      onSuccess: (response) => {
        // RPC-returned ok:false is NOT a client exception — it still goes
        // through onSuccess and emits `provider.tested` with ok:false
        // (preserves the "test ran, here's the result" semantics). Only
        // a client-side exception (RPC call itself failed) emits
        // `provider.test.failed` via onError.
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "provider.tested",
          message: "Provider connection test completed",
          details: { id, ok: response.ok, error: response.error },
        });
      },
      onError: (err: Error) => {
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "provider.test.failed",
          message: "Provider connection test failed (client exception)",
          details: { id, error: err.message },
        });
      },
    });
  };

  const handleModelCreate = (payload: ModelCreateRequestDto) => {
    modelMutations.createModel.mutate(payload, {
      onSuccess: (entry) => {
        // Use the created entry's provider_id + model_id (canonical ids),
        // not the db id — per spec §8 model event details payload.
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "model.added",
          message: "Model added",
          details: { provider_id: entry.provider_id, model_id: entry.model_id },
        });
      },
      onError: (err: Error) => {
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "model.add.failed",
          message: "Model creation failed",
          details: {
            provider_id: payload.provider_id,
            model_id: payload.model_id,
            error: err.message,
          },
        });
      },
    });
  };

  // The full model object is passed (not just the id) so the page can
  // emit spec-§8-correct details `{ provider_id, model_id }` — those ids
  // live on the catalog entry, not on the update patch.
  const handleModelUpdate = (
    model: ModelCatalogEntryDto,
    patch: ModelUpdateRequestDto,
  ) => {
    modelMutations.updateModel.mutate(
      { ...patch, id: model.model_db_id },
      {
        onSuccess: () => {
          reportFrontendEventSafely({
            level: "INFO",
            event_code: "model.updated",
            message: "Model updated",
            details: {
              provider_id: model.provider_id,
              model_id: model.model_id,
            },
          });
        },
        onError: (err: Error) => {
          reportFrontendEventSafely({
            level: "ERROR",
            event_code: "model.update.failed",
            message: "Model update failed",
            details: {
              provider_id: model.provider_id,
              model_id: model.model_id,
              error: err.message,
            },
          });
        },
      },
    );
  };

  const handleModelTagsUpdate = (modelId: string, tags: string[]) => {
    modelMutations.tagsUpdate.mutate(
      { modelId, tags },
      {
        onSuccess: () => {
          reportFrontendEventSafely({
            level: "INFO",
            event_code: "model.tags.updated",
            message: "Model tags updated",
            details: { model_id: modelId, tags },
          });
        },
        onError: (err: Error) => {
          reportFrontendEventSafely({
            level: "ERROR",
            event_code: "model.tags.update.failed",
            message: "Model tags update failed",
            details: { model_id: modelId, error: err.message },
          });
        },
      },
    );
  };

  const handleModelDelete = (model: ModelCatalogEntryDto) => {
    modelMutations.deleteModel.mutate(model.model_db_id, {
      onSuccess: () => {
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "model.deleted",
          message: "Model deleted",
          details: {
            provider_id: model.provider_id,
            model_id: model.model_id,
          },
        });
      },
      onError: (err: Error) => {
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "model.delete.failed",
          message: "Model delete failed",
          details: {
            provider_id: model.provider_id,
            model_id: model.model_id,
            error: err.message,
          },
        });
      },
    });
  };

  return (
    <div className="settings-page">
      <div className="settings-pane">
        <div className="settings-section">
          <h2>Providers</h2>
          <button type="button" onClick={() => setShowCreateForm((v) => !v)}>
            + 新建 Provider
          </button>
        </div>

        {showCreateForm && (
          <ProviderCreationForm onClose={() => setShowCreateForm(false)} />
        )}

        {(providersQuery.data?.providers ?? []).map((provider) => (
          <ProviderCard
            key={provider.id}
            provider={provider}
            models={modelsByProvider.get(provider.id) ?? []}
            isModelsLoading={modelsQuery.isLoading}
            providerMutations={providerMutations}
            modelMutations={modelMutations}
            onEdit={() => setEditingProviderId(provider.id)}
            onTestConnection={handleTestConnection}
            onDelete={handleProviderDelete}
            onModelCreate={handleModelCreate}
            onModelUpdate={handleModelUpdate}
            onModelTagsUpdate={handleModelTagsUpdate}
            onModelDelete={handleModelDelete}
            isEditing={editingProviderId === provider.id}
            onCancelEdit={() => setEditingProviderId(null)}
          />
        ))}
      </div>
    </div>
  );
}
