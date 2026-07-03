import { useCallback, useMemo, useState } from "react";
import type {
  ProfileDto,
  ProviderDto,
  ProfileUpdateRequestDto,
} from "@busytok/protocol-types";
import {
  useSettingsSnapshot,
  useProviders,
  useProfileMutations,
} from "../api/useBusytokData";
import { PageState } from "./PageState";
import { SettingsActionGroup } from "./desktop/SettingsActionGroup";
import { SettingsRow } from "./desktop/SettingsRow";
import { SettingsValue } from "./desktop/SettingsValue";
import { reportFrontendEventSafely } from "../logging/safeReporter";

// ── Helpers ──────────────────────────────────────────────────────────

/** Returns enabled providers for the binding dropdown (spec: "only enabled"). */
function enabledProviders(providers: ProviderDto[]): ProviderDto[] {
  return providers.filter((p) => p.enabled);
}

/** Returns true if the profile's model is NOT in the bound provider's whitelist. */
function isStaleModel(profile: ProfileDto, providers: ProviderDto[]): boolean {
  if (!profile.provider_id) return false;
  const provider = providers.find((p) => p.id === profile.provider_id);
  if (!provider) return true; // provider deleted → stale
  return !provider.models.includes(profile.model);
}

/** Returns true if the profile is bound to a disabled provider. */
function isBoundToDisabledProvider(
  profile: ProfileDto,
  providers: ProviderDto[],
): boolean {
  if (!profile.provider_id) return false;
  const provider = providers.find((p) => p.id === profile.provider_id);
  return provider != null && !provider.enabled;
}

// ── ProfileRow ───────────────────────────────────────────────────────

interface ProfileRowProps {
  profile: ProfileDto;
  providers: ProviderDto[];
  providersDegraded: boolean;
  isEditing: boolean;
  editProviderId: string;
  editModel: string;
  onEdit: (profile: ProfileDto) => void;
  onEditChange: (patch: { providerId?: string; model?: string }) => void;
  onEditSubmit: () => void;
  onEditCancel: () => void;
  isEditPending: boolean;
  onDelete: (id: string) => void;
  isDeletePending: boolean;
}

function ProfileRow({
  profile,
  providers,
  providersDegraded,
  isEditing,
  editProviderId,
  editModel,
  onEdit,
  onEditChange,
  onEditSubmit,
  onEditCancel,
  isEditPending,
  onDelete,
  isDeletePending,
}: ProfileRowProps) {
  // When the providers query failed, we cannot reliably compute
  // stale/disabled state — skip both to avoid false positives.
  const disabled = providersDegraded ? false : isBoundToDisabledProvider(profile, providers);
  const stale = providersDegraded ? false : isStaleModel(profile, providers);
  const enabledProvs = enabledProviders(providers);

  // Cascade-filtered models: only show models from the selected provider.
  const availableModels = useMemo(() => {
    const selected = providers.filter((p) => p.enabled).find((p) => p.id === editProviderId);
    return selected ? selected.models : [];
  }, [providers, editProviderId]);

  // Disable Save when a provider is selected but the model is not in its
  // whitelist (stale or unselected) — spec: "requires re-selection before save".
  const isEditModelStale =
    editProviderId !== "" && !availableModels.includes(editModel);

  return (
    <div className="settings-panel">
      <SettingsRow
        label={profile.id}
        description={profile.is_builtin ? "Built-in profile" : "User profile"}
        control={
          <SettingsValue
            value={profile.is_builtin ? "Built-in" : "Custom"}
            tone="muted"
          />
        }
      />
      {disabled && (
        <SettingsRow
          label="⚠ Warning"
          control={
            <SettingsValue
              value="Bound to a disabled provider — delegate will fail until rebound"
              tone="danger"
            />
          }
        />
      )}
      {stale && !isEditing && (
        <SettingsRow
          label="⚠ Stale Model"
          control={
            <SettingsValue
              value="Not in the provider's whitelist — re-select before save"
              tone="danger"
            />
          }
        />
      )}
      {isEditing ? (
        <>
          <SettingsRow
            layout="vertical"
            label="Provider"
            description="Only enabled providers can be selected."
            control={
              <select
                className="input"
                aria-label="Provider"
                value={editProviderId}
                onChange={(e) => onEditChange({ providerId: e.currentTarget.value })}
              >
                <option value="">— None (unbound) —</option>
                {enabledProvs.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name} ({p.id})
                  </option>
                ))}
              </select>
            }
          />
          <SettingsRow
            layout="vertical"
            label="Model"
            description="Models available from the selected provider."
            control={
              <select
                className="input"
                aria-label="Model"
                value={editModel}
                onChange={(e) => onEditChange({ model: e.currentTarget.value })}
                disabled={availableModels.length === 0}
              >
                <option value="">— Select model —</option>
                {availableModels.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            }
          />
          <SettingsRow
            label="Advanced (read-only)"
            control={
              <SettingsActionGroup direction="col">
                <SettingsValue value={`Tools: ${profile.tools.join(", ")}`} tone="muted" />
                <SettingsValue value={`Budget: ${profile.context_budget_tokens} tokens`} tone="muted" />
                <SettingsValue value={`Timeout: ${profile.timeout_seconds}s`} tone="muted" />
              </SettingsActionGroup>
            }
          />
          <SettingsRow
            label="Actions"
            control={
              <SettingsActionGroup direction="row">
                <button
                  type="button"
                  className="btn btn--primary btn--sm"
                  onClick={onEditSubmit}
                  disabled={isEditPending || isEditModelStale}
                >
                  {isEditPending ? "Saving..." : "Save"}
                </button>
                <button
                  type="button"
                  className="btn btn--secondary btn--sm"
                  onClick={onEditCancel}
                  disabled={isEditPending}
                >
                  Cancel
                </button>
              </SettingsActionGroup>
            }
          />
        </>
      ) : (
        <>
          <SettingsRow
            label="Provider"
            control={
              <SettingsValue
                value={profile.provider_id ?? "— unbound —"}
                tone={profile.provider_id ? "default" : "muted"}
              />
            }
          />
          <SettingsRow
            label="Model"
            control={
              <SettingsValue
                value={stale ? "—" : profile.model}
                tone={stale ? "danger" : "default"}
              />
            }
          />
          <SettingsRow
            label="Advanced (read-only)"
            control={
              <SettingsActionGroup direction="col">
                <SettingsValue value={`Tools: ${profile.tools.join(", ")}`} tone="muted" />
                <SettingsValue value={`Budget: ${profile.context_budget_tokens} tokens`} tone="muted" />
                <SettingsValue value={`Timeout: ${profile.timeout_seconds}s`} tone="muted" />
              </SettingsActionGroup>
            }
          />
          <SettingsRow
            label="Actions"
            control={
              <SettingsActionGroup direction="row">
                <button
                  type="button"
                  className="btn btn--primary btn--sm"
                  onClick={() => onEdit(profile)}
                >
                  Edit
                </button>
                {!profile.is_builtin && (
                  <button
                    type="button"
                    className="btn btn--danger btn--sm"
                    onClick={() => onDelete(profile.id)}
                    disabled={isDeletePending}
                  >
                    Delete
                  </button>
                )}
              </SettingsActionGroup>
            }
          />
        </>
      )}
    </div>
  );
}

// ── ProfilesSection ──────────────────────────────────────────────────

export function ProfilesSection() {
  const snapshotQuery = useSettingsSnapshot();
  const providersQuery = useProviders();
  const { updateProfile, deleteProfile } = useProfileMutations();

  const [editingId, setEditingId] = useState<string | null>(null);
  const [editProviderId, setEditProviderId] = useState("");
  const [editModel, setEditModel] = useState("");
  const [mutationError, setMutationError] = useState<string | null>(null);

  const profiles = snapshotQuery.data?.data?.subagent?.profiles ?? [];
  const providers = providersQuery.data?.providers ?? [];
  const providersDegraded = providersQuery.isError;

  const handleEdit = useCallback((profile: ProfileDto) => {
    setEditingId(profile.id);
    setEditProviderId(profile.provider_id ?? "");
    setEditModel(profile.model);
    setMutationError(null);
  }, []);

  const handleEditChange = useCallback(
    (patch: { providerId?: string; model?: string }) => {
      if (patch.providerId !== undefined) {
        setEditProviderId(patch.providerId);
        // Cascade: when the provider changes, reset the model to the first
        // available model from the new provider (or empty if none).
        const newProvider = providers.find((p) => p.id === patch.providerId);
        setEditModel(newProvider?.models[0] ?? "");
      }
      if (patch.model !== undefined) {
        setEditModel(patch.model);
      }
    },
    [providers],
  );

  const handleEditSubmit = useCallback(() => {
    if (!editingId) return;
    setMutationError(null);
    const req: ProfileUpdateRequestDto = {
      id: editingId,
      provider_id: editProviderId, // empty string = unbind
      model: editModel,
      tools: null,
      context_budget_tokens: null,
      timeout_seconds: null,
      write_access: null,
    };
    updateProfile.mutate(req, {
      onSuccess: () => {
        setEditingId(null);
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "profile.updated",
          message: "Profile updated",
          details: { id: editingId },
        });
      },
      onError: (err) => {
        const msg = (err as Error)?.message ?? String(err);
        // Exit edit mode so the user can see the error and retry from the
        // view layout; `handleEdit` clears `mutationError` on re-entry.
        setEditingId(null);
        setMutationError(msg);
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "profile.update.failed",
          message: "Profile update failed",
          details: { id: editingId, error: msg },
        });
      },
    });
  }, [editingId, editProviderId, editModel, updateProfile]);

  const handleEditCancel = useCallback(() => {
    setEditingId(null);
    setMutationError(null);
  }, []);

  const handleDelete = useCallback(
    (id: string) => {
      setMutationError(null);
      deleteProfile.mutate(id, {
        onError: (err) => {
          const msg = (err as Error)?.message ?? String(err);
          setMutationError(msg);
          reportFrontendEventSafely({
            level: "ERROR",
            event_code: "profile.delete.failed",
            message: "Profile delete failed",
            details: { id, error: msg },
          });
        },
      });
    },
    [deleteProfile],
  );

  if (snapshotQuery.isLoading) {
    return <PageState kind="loading" title="Profiles" message="Loading profiles..." />;
  }
  if (snapshotQuery.isError) {
    return <PageState kind="error" title="Profiles" message="Failed to load profiles" />;
  }

  return (
    <section className="settings-section">
      <h2>Profiles</h2>
      {providersDegraded && (
        <div className="settings-panel">
          <SettingsRow
            label="⚠ Warning"
            control={
              <SettingsValue
                value="Provider list unavailable — binding checks disabled. Retry by reloading."
                tone="warning"
              />
            }
          />
        </div>
      )}
      {mutationError && (
        <div className="settings-panel">
          <SettingsRow
            label="Error"
            control={<SettingsValue value={mutationError} tone="danger" />}
          />
        </div>
      )}
      {profiles.length === 0 ? (
        <div className="settings-panel">
          <p>No profiles configured.</p>
        </div>
      ) : (
        profiles.map((profile) => (
          <ProfileRow
            key={profile.id}
            profile={profile}
            providers={providers}
            providersDegraded={providersDegraded}
            isEditing={editingId === profile.id}
            editProviderId={editProviderId}
            editModel={editModel}
            onEdit={handleEdit}
            onEditChange={handleEditChange}
            onEditSubmit={handleEditSubmit}
            onEditCancel={handleEditCancel}
            isEditPending={updateProfile.isPending}
            onDelete={handleDelete}
            isDeletePending={deleteProfile.isPending}
          />
        ))
      )}
    </section>
  );
}
