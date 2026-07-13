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
import { ProviderCard, type TestConnectionResult } from "../components/ProviderCard";
import { ProviderCreationForm } from "../components/ProviderCreationForm";
import { reportFrontendEventSafely } from "../logging/safeReporter";
import { errorMessage } from "./providerFormUtils";

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
  // f3: surface test-connection results to the UI (keyed by provider id).
  const [testResults, setTestResults] = useState<Record<string, TestConnectionResult>>({});

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

  const handleProviderDelete = async (provider: ProviderDto): Promise<void> => {
    try {
      await providerMutations.deleteProvider.mutateAsync(provider.id);
      reportFrontendEventSafely({
        level: "INFO",
        event_code: "provider.deleted",
        message: "Provider deleted",
        details: { id: provider.id, name: provider.name },
      });
      // P2 #8: clear stale test result for deleted provider.
      setTestResults((prev) => {
        if (!(provider.id in prev)) return prev;
        const next = { ...prev };
        delete next[provider.id];
        return next;
      });
    } catch (err) {
      reportFrontendEventSafely({
        level: "ERROR",
        event_code: "provider.delete.failed",
        message: "Provider delete failed",
        details: { id: provider.id, name: provider.name, error: errorMessage(err, "unknown error") },
      });
      throw err;
    }
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
        // f3: surface result to the card UI.
        setTestResults((prev) => ({
          ...prev,
          [id]: { ok: response.ok, error: response.error },
        }));
      },
      onError: (err: Error) => {
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "provider.test.failed",
          message: "Provider connection test failed (client exception)",
          details: { id, error: err.message },
        });
        // f3: surface client-side exception as a failure result.
        setTestResults((prev) => ({
          ...prev,
          [id]: { ok: false, error: err.message },
        }));
      },
    });
  };

  const handleModelCreate = async (payload: ModelCreateRequestDto): Promise<void> => {
    try {
      const entry = await modelMutations.createModel.mutateAsync(payload);
      // Use the created entry's provider_id + model_id (canonical ids),
      // not the db id — per spec §8 model event details payload.
      reportFrontendEventSafely({
        level: "INFO",
        event_code: "model.added",
        message: "Model added",
        details: { provider_id: entry.provider_id, model_id: entry.model_id },
      });
    } catch (err) {
      reportFrontendEventSafely({
        level: "ERROR",
        event_code: "model.add.failed",
        message: "Model creation failed",
        details: {
          provider_id: payload.provider_id,
          model_id: payload.model_id,
          error: errorMessage(err, "unknown error"),
        },
      });
      throw err;
    }
  };

  // The full model object is passed (not just the id) so the page can
  // emit spec-§8-correct details `{ provider_id, model_id }` — those ids
  // live on the catalog entry, not on the update patch.
  // Returns a Promise so ProviderCard can await success before closing
  // the inline form (prevents input loss on RPC failure).
  const handleModelUpdate = async (
    model: ModelCatalogEntryDto,
    patch: ModelUpdateRequestDto,
  ): Promise<void> => {
    try {
      await modelMutations.updateModel.mutateAsync({ ...patch, id: model.model_db_id });
      reportFrontendEventSafely({
        level: "INFO",
        event_code: "model.updated",
        message: "Model updated",
        details: {
          provider_id: model.provider_id,
          model_id: model.model_id,
        },
      });
    } catch (err) {
      reportFrontendEventSafely({
        level: "ERROR",
        event_code: "model.update.failed",
        message: "Model update failed",
        details: {
          provider_id: model.provider_id,
          model_id: model.model_id,
          error: errorMessage(err, "unknown error"),
        },
      });
      throw err;
    }
  };

  const handleModelTagsUpdate = async (
    model: ModelCatalogEntryDto,
    tags: string[],
  ): Promise<void> => {
    try {
      await modelMutations.tagsUpdate.mutateAsync({ modelId: model.model_db_id, tags });
      reportFrontendEventSafely({
        level: "INFO",
        event_code: "model.tags.updated",
        message: "Model tags updated",
        details: {
          provider_id: model.provider_id,
          model_id: model.model_id,
          tags,
        },
      });
    } catch (err) {
      reportFrontendEventSafely({
        level: "ERROR",
        event_code: "model.tags.update.failed",
        message: "Model tags update failed",
        details: {
          provider_id: model.provider_id,
          model_id: model.model_id,
          error: errorMessage(err, "unknown error"),
        },
      });
      throw err;
    }
  };

  const handleModelDelete = async (model: ModelCatalogEntryDto): Promise<void> => {
    try {
      await modelMutations.deleteModel.mutateAsync(model.model_db_id);
      reportFrontendEventSafely({
        level: "INFO",
        event_code: "model.deleted",
        message: "Model deleted",
        details: {
          provider_id: model.provider_id,
          model_id: model.model_id,
        },
      });
    } catch (err) {
      reportFrontendEventSafely({
        level: "ERROR",
        event_code: "model.delete.failed",
        message: "Model delete failed",
        details: {
          provider_id: model.provider_id,
          model_id: model.model_id,
          error: errorMessage(err, "unknown error"),
        },
      });
      throw err;
    }
  };

  return (
    <div className="settings-page">
      <div className="provider-catalog">
        <div className="provider-catalog__header">
          <h1>Providers</h1>
          <button type="button" className="btn btn--primary btn--sm" onClick={() => setShowCreateForm((v) => !v)}>
            + New Provider
          </button>
        </div>

        {showCreateForm && (
          <ProviderCreationForm onClose={() => setShowCreateForm(false)} />
        )}

        {providersQuery.isError && (
          <div className="degraded-ribbon" role="alert">
            <span className="degraded-ribbon__dot" />
            Failed to load providers. Please refresh the page.
          </div>
        )}

        {modelsQuery.isError && (
          <div className="degraded-ribbon" role="alert">
            <span className="degraded-ribbon__dot" />
            Failed to load models. Model operations may be unavailable. Please refresh the page.
          </div>
        )}

        {!providersQuery.isError && (providersQuery.data?.providers ?? []).map((provider) => (
          <ProviderCard
            key={provider.id}
            provider={provider}
            models={modelsByProvider.get(provider.id) ?? []}
            isModelsLoading={modelsQuery.isLoading}
            providerMutations={providerMutations}
            onEdit={() => {
              setEditingProviderId(provider.id);
              // P2 #8: clear stale test result when entering edit mode.
              setTestResults((prev) => {
                if (!(provider.id in prev)) return prev;
                const next = { ...prev };
                delete next[provider.id];
                return next;
              });
            }}
            onTestConnection={handleTestConnection}
            onDelete={handleProviderDelete}
            onModelCreate={handleModelCreate}
            onModelUpdate={handleModelUpdate}
            onModelTagsUpdate={handleModelTagsUpdate}
            onModelDelete={handleModelDelete}
            isEditing={editingProviderId === provider.id}
            onCancelEdit={() => setEditingProviderId(null)}
            testResult={testResults[provider.id]}
          />
        ))}
      </div>
    </div>
  );
}
