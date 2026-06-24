import { DownloadCloud } from "lucide-react";
import { useUpdater } from "../../hooks/useUpdater";

/** Titlebar badge: hidden unless an update is available/active. */
export function UpdateBadgeButton() {
  const { status, applyNow } = useUpdater();

  if (
    status.state === "idle" ||
    status.state === "up-to-date" ||
    status.state === "checking" ||
    status.state === "error"
  ) {
    return null;
  }

  if (status.state === "downloading") {
    const label = status.percent == null ? "Updating…" : `Update ${status.percent}%`;
    return (
      <button type="button" className="update-badge update-badge--info" disabled aria-label={label}>
        <DownloadCloud size={14} />
        <span>{label}</span>
      </button>
    );
  }

  if (status.state === "installed-pending-restart") {
    return (
      <button type="button" className="update-badge update-badge--info" disabled aria-label="Restarting">
        <span>Restarting…</span>
      </button>
    );
  }

  if (status.state === "installed-needs-manual-restart") {
    return (
      <span className="update-badge update-badge--info" role="status">
        Updated to v{status.version} — restart Busytok manually
      </span>
    );
  }

  // available: success chip
  return (
    <button
      type="button"
      className="update-badge update-badge--available"
      onClick={() => void applyNow()}
      title={`v${status.version} available\n\n${status.notes || "(no release notes)"}`}
      aria-label={`Update to version ${status.version}`}
    >
      <span className="update-badge__dot" aria-hidden="true" />
      <span>Update</span>
    </button>
  );
}
