import { DownloadCloud } from "lucide-react";
import { useUpdater } from "../../hooks/useUpdater";

/** Titlebar badge: hidden unless an update is available/active. */
export function UpdateBadgeButton() {
  const { status, checkNow, applyNow } = useUpdater();

  if (status.state === "idle" || status.state === "up-to-date" || status.state === "checking") {
    return null;
  }

  if (status.state === "downloading") {
    const label = status.percent == null ? "Updating…" : `Update ${status.percent}%`;
    return (
      <button type="button" className="update-badge update-badge--busy" disabled aria-label={label}>
        <DownloadCloud size={14} />
        <span>{label}</span>
      </button>
    );
  }

  if (status.state === "installed-pending-restart") {
    return (
      <button type="button" className="update-badge" disabled aria-label="Restarting">
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

  if (status.state === "error") {
    return (
      <button type="button" className="update-badge update-badge--error" onClick={() => void checkNow()} aria-label="Retry update check">
        Retry check
      </button>
    );
  }

  // available
  return (
    <button
      type="button"
      className="update-badge"
      onClick={() => void applyNow()}
      title={`v${status.version} available\n\n${status.notes || "(no release notes)"}`}
      aria-label={`Update to version ${status.version}`}
    >
      <DownloadCloud size={14} />
      <span className="update-badge__dot" aria-hidden="true" />
      <span>Update</span>
    </button>
  );
}
