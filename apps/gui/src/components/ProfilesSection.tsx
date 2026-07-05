import { useCallback, useState } from "react";
import type {
  ProfileDto,
  ProfileCreateRequestDto,
  ProfileUpdateRequestDto,
} from "@busytok/protocol-types";
import {
  useSettingsSnapshot,
  useProfileMutations,
} from "../api/useBusytokData";
import { PageState } from "./PageState";
import { SettingsActionGroup } from "./desktop/SettingsActionGroup";
import { SettingsRow } from "./desktop/SettingsRow";
import { SettingsValue } from "./desktop/SettingsValue";
import { reportFrontendEventSafely } from "../logging/safeReporter";

// ── Helpers ──────────────────────────────────────────────────────────

function parseTools(input: string): string[] {
  return input
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

// ── ProfileRow ───────────────────────────────────────────────────────
//
// Per-profile row. The post-Task-4 `ProfileDto` no longer carries
// `provider_id` / `model` (the bound-fields migration moved binding down
// to the behavior-template layer), so this row edits ONLY the behavior
// template fields: tools, context_budget_tokens, timeout_seconds,
// write_access. Provider/model selection is gone per spec §6.
//
// Built-in profiles are read-only (no Edit, no Delete). User profiles
// show an inline edit form (toggled by the Edit button) + a Delete button.

interface ProfileRowProps {
  profile: ProfileDto;
  isDeletePending: boolean;
  isUpdatePending: boolean;
  onDelete: (id: string) => void;
  onSave: (
    id: string,
    patch: ProfileUpdateRequestDto,
    onDone: () => void,
  ) => void;
}

function ProfileRow({
  profile,
  isDeletePending,
  isUpdatePending,
  onDelete,
  onSave,
}: ProfileRowProps) {
  // Inline edit state — local to the row so multiple rows can be edited
  // independently. Initialised lazily from the profile's current values.
  const [editing, setEditing] = useState(false);
  const [toolsDraft, setToolsDraft] = useState(profile.tools.join(", "));
  const [budgetDraft, setBudgetDraft] = useState<number | undefined>(
    profile.context_budget_tokens,
  );
  const [timeoutDraft, setTimeoutDraft] = useState<number | undefined>(
    profile.timeout_seconds,
  );
  const [writeAccessDraft, setWriteAccessDraft] = useState(
    profile.write_access,
  );

  const beginEdit = () => {
    setToolsDraft(profile.tools.join(", "));
    setBudgetDraft(profile.context_budget_tokens);
    setTimeoutDraft(profile.timeout_seconds);
    setWriteAccessDraft(profile.write_access);
    setEditing(true);
  };

  const cancelEdit = () => setEditing(false);

  const submitEdit = () => {
    const patch: ProfileUpdateRequestDto = {
      id: profile.id,
      tools: parseTools(toolsDraft),
      context_budget_tokens: budgetDraft ?? null,
      timeout_seconds: timeoutDraft ?? null,
      write_access: writeAccessDraft,
    };
    onSave(profile.id, patch, () => setEditing(false));
  };

  if (editing) {
    return (
      <div className="settings-panel">
        <SettingsRow
          label={profile.id}
          description="Editing behavior template"
          control={
            <SettingsValue value="Custom" tone="muted" />
          }
        />
        <SettingsRow
          layout="vertical"
          label="Tools"
          description="Comma-separated tool list."
          control={
            <input
              type="text"
              className="input"
              aria-label={`Tools for ${profile.id}`}
              placeholder="read, grep, glob"
              value={toolsDraft}
              onChange={(e) => setToolsDraft(e.currentTarget.value)}
            />
          }
        />
        <SettingsRow
          layout="vertical"
          label="Context budget (tokens)"
          control={
            <input
              type="number"
              className="input"
              aria-label={`Context budget for ${profile.id}`}
              value={budgetDraft ?? ""}
              onChange={(e) => {
                const v = e.currentTarget.value;
                setBudgetDraft(v ? Number(v) : undefined);
              }}
            />
          }
        />
        <SettingsRow
          layout="vertical"
          label="Timeout (seconds)"
          control={
            <input
              type="number"
              className="input"
              aria-label={`Timeout for ${profile.id}`}
              value={timeoutDraft ?? ""}
              onChange={(e) => {
                const v = e.currentTarget.value;
                setTimeoutDraft(v ? Number(v) : undefined);
              }}
            />
          }
        />
        <SettingsRow
          label="Write access"
          control={
            <input
              type="checkbox"
              checked={writeAccessDraft}
              onChange={(e) => setWriteAccessDraft(e.currentTarget.checked)}
              aria-label={`Write access for ${profile.id}`}
            />
          }
        />
        <SettingsRow
          label="Actions"
          control={
            <SettingsActionGroup direction="row">
              <button
                type="button"
                className="btn btn--primary btn--sm"
                onClick={submitEdit}
                disabled={isUpdatePending}
              >
                {isUpdatePending ? "Saving..." : "Save"}
              </button>
              <button
                type="button"
                className="btn btn--secondary btn--sm"
                onClick={cancelEdit}
                disabled={isUpdatePending}
              >
                Cancel
              </button>
            </SettingsActionGroup>
          }
        />
      </div>
    );
  }

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
      <SettingsRow
        label="Advanced (read-only)"
        control={
          <SettingsActionGroup direction="col">
            <SettingsValue
              value={`Tools: ${profile.tools.join(", ")}`}
              tone="muted"
            />
            <SettingsValue
              value={`Budget: ${profile.context_budget_tokens} tokens`}
              tone="muted"
            />
            <SettingsValue
              value={`Timeout: ${profile.timeout_seconds}s`}
              tone="muted"
            />
            <SettingsValue
              value={`Write access: ${profile.write_access ? "yes" : "no"}`}
              tone="muted"
            />
          </SettingsActionGroup>
        }
      />
      {!profile.is_builtin && (
        <SettingsRow
          label="Actions"
          control={
            <SettingsActionGroup direction="row">
              <button
                type="button"
                className="btn btn--secondary btn--sm"
                onClick={beginEdit}
                disabled={isUpdatePending || isDeletePending}
              >
                Edit
              </button>
              <button
                type="button"
                className="btn btn--danger btn--sm"
                onClick={() => onDelete(profile.id)}
                disabled={isDeletePending || isUpdatePending}
              >
                {isDeletePending ? "Deleting..." : "Delete"}
              </button>
            </SettingsActionGroup>
          }
        />
      )}
    </div>
  );
}

// ── ProfilesSection ──────────────────────────────────────────────────

/**
 * Profile CRUD for behavior-template fields (tools, context_budget_tokens,
 * timeout_seconds, write_access). Provider/model selection is NOT exposed
 * here — those are gone per spec §6 (binding moved to the subagent layer).
 *
 * Reads via settings.snapshot (subagent.profiles[]); writes via the
 * `profile.create` / `profile.update` / `profile.delete` RPCs.
 */
export function ProfilesSection() {
  const snapshotQuery = useSettingsSnapshot();
  const { createProfile, updateProfile, deleteProfile } = useProfileMutations();

  const [mutationError, setMutationError] = useState<string | null>(null);

  // Create-form state. `id` is required; the rest are optional behavior
  // template fields (sent as-is when provided).
  const [createForm, setCreateForm] = useState<{
    id: string;
    tools: string;
    context_budget_tokens: number | undefined;
    timeout_seconds: number | undefined;
    write_access: boolean;
  }>({
    id: "",
    tools: "",
    context_budget_tokens: undefined,
    timeout_seconds: undefined,
    write_access: false,
  });

  const profiles = snapshotQuery.data?.data?.subagent?.profiles ?? [];

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

  const handleSave = useCallback(
    (
      id: string,
      patch: ProfileUpdateRequestDto,
      onDone: () => void,
    ) => {
      setMutationError(null);
      updateProfile.mutate(patch, {
        onSuccess: () => {
          reportFrontendEventSafely({
            level: "INFO",
            event_code: "profile.updated",
            message: "Profile updated",
            details: { id },
          });
          onDone();
        },
        onError: (err) => {
          const msg = (err as Error)?.message ?? String(err);
          setMutationError(msg);
          reportFrontendEventSafely({
            level: "ERROR",
            event_code: "profile.update.failed",
            message: "Profile update failed",
            details: { id, error: msg },
          });
        },
      });
    },
    [updateProfile],
  );

  const handleCreateSubmit = useCallback(() => {
    const id = createForm.id.trim();
    if (id === "") {
      setMutationError("Profile ID cannot be empty.");
      return;
    }
    setMutationError(null);
    const payload: ProfileCreateRequestDto = {
      id,
      tools: parseTools(createForm.tools),
      context_budget_tokens: createForm.context_budget_tokens ?? null,
      timeout_seconds: createForm.timeout_seconds ?? null,
      write_access: createForm.write_access,
    };
    createProfile.mutate(payload, {
      onSuccess: () => {
        reportFrontendEventSafely({
          level: "INFO",
          event_code: "profile.created",
          message: "Profile created",
          details: { id },
        });
        setCreateForm({
          id: "",
          tools: "",
          context_budget_tokens: undefined,
          timeout_seconds: undefined,
          write_access: false,
        });
      },
      onError: (err) => {
        const msg = (err as Error)?.message ?? String(err);
        setMutationError(msg);
        reportFrontendEventSafely({
          level: "ERROR",
          event_code: "profile.create.failed",
          message: "Profile create failed",
          details: { id, error: msg },
        });
      },
    });
  }, [createForm, createProfile]);

  if (snapshotQuery.isLoading) {
    return <PageState kind="loading" title="Profiles" message="Loading profiles..." />;
  }
  if (snapshotQuery.isError) {
    return <PageState kind="error" title="Profiles" message="Failed to load profiles" />;
  }

  return (
    <section className="settings-section">
      <h2>Profiles</h2>
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
          label="Create Profile"
          description="Add a new behavior-template profile. Provider/model binding is configured at the subagent level (spec §6)."
          control={
            <SettingsActionGroup direction="col">
              <input
                type="text"
                className="input"
                aria-label="Profile ID"
                placeholder="my-profile"
                value={createForm.id}
                onChange={(e) => {
                  const v = e.currentTarget.value;
                  setCreateForm((prev) => ({ ...prev, id: v }));
                }}
              />
              <input
                type="text"
                className="input"
                aria-label="Tools for new profile"
                placeholder="read, grep, glob"
                value={createForm.tools}
                onChange={(e) => {
                  const v = e.currentTarget.value;
                  setCreateForm((prev) => ({ ...prev, tools: v }));
                }}
              />
              <input
                type="number"
                className="input"
                aria-label="Context budget (tokens)"
                placeholder="Context budget (tokens)"
                value={createForm.context_budget_tokens ?? ""}
                onChange={(e) => {
                  const v = e.currentTarget.value;
                  setCreateForm((prev) => ({
                    ...prev,
                    context_budget_tokens: v ? Number(v) : undefined,
                  }));
                }}
              />
              <input
                type="number"
                className="input"
                aria-label="Timeout (seconds)"
                placeholder="Timeout (seconds)"
                value={createForm.timeout_seconds ?? ""}
                onChange={(e) => {
                  const v = e.currentTarget.value;
                  setCreateForm((prev) => ({
                    ...prev,
                    timeout_seconds: v ? Number(v) : undefined,
                  }));
                }}
              />
              <label>
                <input
                  type="checkbox"
                  checked={createForm.write_access}
                  onChange={(e) => {
                    const v = e.currentTarget.checked;
                    setCreateForm((prev) => ({ ...prev, write_access: v }));
                  }}
                />
                Write access
              </label>
              <SettingsActionGroup direction="row">
                <button
                  type="button"
                  className="btn btn--primary btn--sm"
                  onClick={handleCreateSubmit}
                  disabled={createProfile.isPending}
                >
                  {createProfile.isPending ? "Creating..." : "Create Profile"}
                </button>
              </SettingsActionGroup>
            </SettingsActionGroup>
          }
        />
      </div>

      {profiles.length === 0 ? (
        <div className="settings-panel">
          <p>No profiles configured.</p>
        </div>
      ) : (
        profiles.map((profile) => (
          <ProfileRow
            key={profile.id}
            profile={profile}
            isDeletePending={deleteProfile.isPending}
            isUpdatePending={updateProfile.isPending}
            onDelete={handleDelete}
            onSave={handleSave}
          />
        ))
      )}
    </section>
  );
}
