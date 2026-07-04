import { useCallback, useMemo, useState } from "react";
import type { ModelCatalogEntryDto, ModelCreateRequestDto } from "@busytok/protocol-types";
import { useModels, useModelMutations, useProviders } from "../api/useBusytokData";
import { PageState } from "./PageState";
import { SettingsActionGroup } from "./desktop/SettingsActionGroup";
import { SettingsRow } from "./desktop/SettingsRow";
import { SettingsValue } from "./desktop/SettingsValue";
import { ToggleSwitch } from "./desktop/ToggleSwitch";
import { reportFrontendEventSafely } from "../logging/safeReporter";

// ── Helpers ──────────────────────────────────────────────────────────

function parseTags(input: string): string[] {
  return input
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

function tagsLabel(tags: string[]): string {
  return tags.length === 0 ? "—" : tags.join(", ");
}

// ── Model row ────────────────────────────────────────────────────────

interface ModelRowProps {
  model: ModelCatalogEntryDto;
  isToggling: boolean;
  isDeleting: boolean;
  isTagsSaving: boolean;
  onToggle: (model: ModelCatalogEntryDto) => void;
  onDelete: (model: ModelCatalogEntryDto) => void;
  onSaveTags: (
    model: ModelCatalogEntryDto,
    newTags: string[],
    onDone: () => void,
  ) => void;
}

function ModelRow({
  model,
  isToggling,
  isDeleting,
  isTagsSaving,
  onToggle,
  onDelete,
  onSaveTags,
}: ModelRowProps) {
  // Inline tag-edit state. Local to the row so multiple rows can be edited
  // independently. Initialised lazily from the model's current tags.
  const [editingTags, setEditingTags] = useState(false);
  const [tagDraft, setTagDraft] = useState(model.tags.join(", "));

  const beginEdit = () => {
    setTagDraft(model.tags.join(", "));
    setEditingTags(true);
  };

  const cancelEdit = () => {
    setEditingTags(false);
  };

  const submitTags = () => {
    onSaveTags(model, parseTags(tagDraft), () => {
      setEditingTags(false);
      setTagDraft(model.tags.join(", "));
    });
  };

  return (
    <div className="settings-panel">
      <SettingsRow
        label={model.model_id}
        description={
          model.model_enabled
            ? undefined
            : "This model is disabled and will not be offered."
        }
        dangerous={!model.model_enabled}
        control={
          <ToggleSwitch
            checked={model.model_enabled}
            onChange={() => onToggle(model)}
            aria-label={`Toggle ${model.model_id}`}
            disabled={isToggling}
          />
        }
      />
      <SettingsRow
        label="Provider"
        control={
          <SettingsValue
            value={`${model.provider_name} (${model.provider_id})`}
            tone="muted"
          />
        }
      />
      <SettingsRow
        label="Tags"
        control={
          editingTags ? (
            <SettingsActionGroup direction="row">
              <input
                type="text"
                className="input"
                aria-label={`Tags for ${model.model_id}`}
                placeholder="chat, reasoning"
                value={tagDraft}
                onChange={(e) => setTagDraft(e.currentTarget.value)}
              />
              <button
                type="button"
                className="btn btn--secondary btn--sm"
                onClick={submitTags}
                disabled={isTagsSaving}
              >
                {isTagsSaving ? "Saving..." : "Save Tags"}
              </button>
              <button
                type="button"
                className="btn btn--secondary btn--sm"
                onClick={cancelEdit}
                disabled={isTagsSaving}
              >
                Cancel
              </button>
            </SettingsActionGroup>
          ) : (
            <SettingsActionGroup direction="row">
              <SettingsValue value={tagsLabel(model.tags)} tone="muted" />
              <button
                type="button"
                className="btn btn--secondary btn--sm"
                onClick={beginEdit}
              >
                Edit Tags
              </button>
            </SettingsActionGroup>
          )
        }
      />
      <SettingsRow
        label="Actions"
        control={
          <SettingsActionGroup direction="row">
            <button
              type="button"
              className="btn btn--danger btn--sm"
              onClick={() => onDelete(model)}
              disabled={isDeleting}
            >
              {isDeleting ? "Deleting..." : "Delete"}
            </button>
          </SettingsActionGroup>
        }
      />
    </div>
  );
}

// ── ModelsSection ────────────────────────────────────────────────────

/**
 * Model catalog CRUD + tag management. Reads the catalog via `useModels`
 * and writes via `useModelMutations` (both from `useBusytokData`).
 *
 * Query cache, loading state, error state, and invalidation are all owned
 * by React Query (through the hooks); the component itself holds only
 * filter UI input values and per-row tag-edit drafts.
 */
export function ModelsSection() {
  const providersQuery = useProviders();
  const [filterProvider, setFilterProvider] = useState<string>("");
  const [filterTag, setFilterTag] = useState<string>("");
  const [showAll, setShowAll] = useState(false);

  const tags = useMemo(
    () => parseTags(filterTag),
    [filterTag],
  );

  const modelsQuery = useModels({
    providerId: filterProvider || undefined,
    tags,
    includeDisabled: showAll,
  });
  const { createModel, updateModel, deleteModel, tagsUpdate } =
    useModelMutations();

  // Create-form state.
  const [createForm, setCreateForm] = useState<{
    providerId: string;
    modelId: string;
    tags: string;
  }>({ providerId: "", modelId: "", tags: "" });
  const [mutationError, setMutationError] = useState<string | null>(null);

  const providers = useMemo(
    () => providersQuery.data?.providers ?? [],
    [providersQuery.data?.providers],
  );

  const handleCreateSubmit = useCallback(() => {
    if (createForm.providerId === "") {
      setMutationError("Select a provider before adding a model.");
      return;
    }
    const modelId = createForm.modelId.trim();
    if (modelId === "") {
      setMutationError("Model ID cannot be empty.");
      return;
    }
    setMutationError(null);
    const payload: ModelCreateRequestDto = {
      provider_id: createForm.providerId,
      model_id: modelId,
      enabled: true,
      tags: parseTags(createForm.tags),
    };
    createModel.mutate(payload, {
      onSuccess: (entry: ModelCatalogEntryDto) => {
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "model.added",
          message: "Model added",
          details: {
            provider_id: entry.provider_id,
            model_id: entry.model_id,
          },
        });
        setCreateForm({ providerId: "", modelId: "", tags: "" });
      },
      onError: (err: unknown) => {
        setMutationError((err as Error)?.message ?? String(err));
      },
    });
  }, [createForm, createModel]);

  const handleToggle = useCallback(
    (model: ModelCatalogEntryDto) => {
      setMutationError(null);
      updateModel.mutate(
        {
          id: model.model_db_id,
          enabled: !model.model_enabled,
        },
        {
          onError: (err: unknown) => {
            setMutationError((err as Error)?.message ?? String(err));
          },
        },
      );
    },
    [updateModel],
  );

  const handleDelete = useCallback(
    (model: ModelCatalogEntryDto) => {
      const confirmed = globalThis.confirm(
        `Delete model "${model.model_id}" from ${model.provider_name}? This cannot be undone.`,
      );
      if (!confirmed) return;
      setMutationError(null);
      deleteModel.mutate(model.model_db_id, {
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
        onError: (err: unknown) => {
          setMutationError((err as Error)?.message ?? String(err));
        },
      });
    },
    [deleteModel],
  );

  const handleSaveTags = useCallback(
    (
      model: ModelCatalogEntryDto,
      newTags: string[],
      onDone: () => void,
    ) => {
      setMutationError(null);
      tagsUpdate.mutate(
        { modelId: model.model_db_id, tags: newTags },
        {
          onSuccess: onDone,
          onError: (err: unknown) => {
            setMutationError((err as Error)?.message ?? String(err));
          },
        },
      );
    },
    [tagsUpdate],
  );

  // ── Loading / error states ─────────────────────────────────────────
  // The catalog query is enabled only when `useModels` decides (here it is
  // always enabled because providerId is `undefined` when no filter is
  // set — we want a cross-provider listing in that case).
  if (modelsQuery.isLoading && !modelsQuery.data) {
    return (
      <section className="settings-section">
        <h2>Models</h2>
        <PageState
          kind="loading"
          title="Models"
          message="Loading models..."
        />
      </section>
    );
  }
  if (modelsQuery.isError && !modelsQuery.data) {
    return (
      <section className="settings-section">
        <h2>Models</h2>
        <PageState
          kind="error"
          title="Models unavailable"
          message="Could not load models."
        />
      </section>
    );
  }

  const models = modelsQuery.data?.models ?? [];

  return (
    <section className="settings-section">
      <h2>Models</h2>
      <div className="settings-panel">
        <SettingsRow
          layout="vertical"
          label="Filter by provider"
          description="Restrict the listing to a single provider, or leave blank for all."
          control={
            <select
              className="input"
              aria-label="Filter by provider"
              value={filterProvider}
              onChange={(e) => setFilterProvider(e.currentTarget.value)}
            >
              <option value="">— All providers —</option>
              {providers.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name} ({p.id})
                </option>
              ))}
            </select>
          }
        />
        <SettingsRow
          layout="vertical"
          label="Filter by tag"
          description="Comma-separated tag list; models matching ALL tags are returned."
          control={
            <input
              type="text"
              className="input"
              aria-label="Filter by tag"
              placeholder="chat, reasoning"
              value={filterTag}
              onChange={(e) => setFilterTag(e.currentTarget.value)}
            />
          }
        />
        <SettingsRow
          label="Show disabled"
          description="Include disabled models in the listing."
          control={
            <ToggleSwitch
              checked={showAll}
              onChange={setShowAll}
              aria-label="Show disabled models"
            />
          }
        />
      </div>

      {mutationError && (
        <div className="settings-panel">
          <SettingsRow
            label="Error"
            control={<SettingsValue value={mutationError} tone="danger" />}
          />
        </div>
      )}

      <div className="settings-panel">
        <SettingsRow
          layout="vertical"
          label="Add Model"
          description="Register a new OpenAI-style model ID under a provider."
          control={
            <SettingsActionGroup direction="col">
              <select
                className="input"
                aria-label="Provider for new model"
                value={createForm.providerId}
                onChange={(e) => {
                  const v = e.currentTarget.value;
                  setCreateForm((prev) => ({ ...prev, providerId: v }));
                }}
              >
                <option value="">— Select provider —</option>
                {providers.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name} ({p.id})
                  </option>
                ))}
              </select>
              <input
                type="text"
                className="input"
                aria-label="Model ID"
                placeholder="deepseek-chat"
                value={createForm.modelId}
                onChange={(e) => {
                  const v = e.currentTarget.value;
                  setCreateForm((prev) => ({ ...prev, modelId: v }));
                }}
              />
              <input
                type="text"
                className="input"
                aria-label="Tags for new model"
                placeholder="chat, reasoning"
                value={createForm.tags}
                onChange={(e) => {
                  const v = e.currentTarget.value;
                  setCreateForm((prev) => ({ ...prev, tags: v }));
                }}
              />
              <SettingsActionGroup direction="row">
                <button
                  type="button"
                  className="btn btn--primary btn--sm"
                  onClick={handleCreateSubmit}
                  disabled={createModel.isPending}
                >
                  {createModel.isPending ? "Saving..." : "Add Model"}
                </button>
              </SettingsActionGroup>
            </SettingsActionGroup>
          }
        />
      </div>

      {models.length === 0 ? (
        <div className="settings-panel">
          <p>No models match the current filter.</p>
        </div>
      ) : (
        models.map((model) => (
          <ModelRow
            key={model.model_db_id}
            model={model}
            isToggling={updateModel.isPending}
            isDeleting={deleteModel.isPending}
            isTagsSaving={tagsUpdate.isPending}
            onToggle={handleToggle}
            onDelete={handleDelete}
            onSaveTags={handleSaveTags}
          />
        ))
      )}
    </section>
  );
}
