export type StatusTone = "ok" | "warning" | "error";

export function StatusPill({ tone, label }: { tone: StatusTone; label?: string }) {
  return (
    <span className={`status-pill status-pill--${tone}`}>{label ?? tone}</span>
  );
}
