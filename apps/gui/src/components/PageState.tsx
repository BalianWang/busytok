interface PageStateProps {
  kind: "loading" | "empty" | "error" | "unavailable" | "degraded";
  title: string;
  message: string;
  /** Optional action button label (e.g. "Retry", "Clear filter"). */
  actionLabel?: string;
  /** Callback fired when the action button is clicked. */
  onAction?: () => void;
  /** Number of diagnostic items (shown only for degraded kind). */
  diagnosticsCount?: number;
}

const BADGE_LABELS: Record<PageStateProps["kind"], string> = {
  loading: "Loading",
  empty: "Empty state",
  error: "Error",
  unavailable: "Unavailable",
  degraded: "Degraded",
};

export function PageState({
  kind,
  title,
  message,
  actionLabel,
  onAction,
  diagnosticsCount,
}: PageStateProps) {
  return (
    <div className="page-state surface-inset" data-state-kind={kind}>
      <div className="page-state__badge">{BADGE_LABELS[kind]}</div>
      <h2 className="page-state__title">{title}</h2>
      <p className="page-state__message">{message}</p>
      {diagnosticsCount != null && diagnosticsCount > 0 && (
        <div className="page-state__diagnostics">
          <span className="page-state__diag-count">{diagnosticsCount} diagnostic{diagnosticsCount !== 1 ? "s" : ""}</span>
        </div>
      )}
      {actionLabel && onAction ? (
        <div className="page-state__action">
          <button type="button" className="btn btn--primary" onClick={onAction}>
            {actionLabel}
          </button>
        </div>
      ) : null}
    </div>
  );
}
