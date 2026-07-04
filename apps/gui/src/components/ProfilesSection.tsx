import { useCallback, useState } from "react";
import type { ProfileDto } from "@busytok/protocol-types";
import {
  useSettingsSnapshot,
  useProfileMutations,
} from "../api/useBusytokData";
import { PageState } from "./PageState";
import { SettingsActionGroup } from "./desktop/SettingsActionGroup";
import { SettingsRow } from "./desktop/SettingsRow";
import { SettingsValue } from "./desktop/SettingsValue";
import { reportFrontendEventSafely } from "../logging/safeReporter";

// ── ProfileRow ───────────────────────────────────────────────────────
//
// Per-profile row. The post-Task-4 `ProfileDto` no longer carries
// `provider_id` / `model` (the bound-fields migration moved binding
// down to the behavior-template layer), so this row is a read-only
// summary with a Delete button for non-builtin profiles.

interface ProfileRowProps {
  profile: ProfileDto;
  isDeletePending: boolean;
  onDelete: (id: string) => void;
}

function ProfileRow({
  profile,
  isDeletePending,
  onDelete,
}: ProfileRowProps) {
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
                className="btn btn--danger btn--sm"
                onClick={() => onDelete(profile.id)}
                disabled={isDeletePending}
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
 * Read-only profile list. Profiles are READ via settings.snapshot (the
 * subagent.profiles[] array); the only write path exposed here is
 * delete (for non-builtin profiles). Updating tools/budget/timeout/
 * write_access is handled by the backend behavior-template migration
 * (Task 4 spec §6) — the GUI no longer edits those fields directly.
 */
export function ProfilesSection() {
  const snapshotQuery = useSettingsSnapshot();
  const { deleteProfile } = useProfileMutations();

  const [mutationError, setMutationError] = useState<string | null>(null);

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
            onDelete={handleDelete}
          />
        ))
      )}
    </section>
  );
}
