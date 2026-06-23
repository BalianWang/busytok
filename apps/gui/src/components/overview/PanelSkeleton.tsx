//! PanelSkeleton — a low-contrast in-frame placeholder so the panel frame
//! stays stable during loading (no layout jump, no full-page spinner card).

interface PanelSkeletonProps {
  variant: "metrics" | "chart" | "table" | "list";
  rows?: number;
}

export function PanelSkeleton({ variant, rows = 3 }: PanelSkeletonProps) {
  if (variant === "metrics") {
    return (
      <div className="panel-skeleton panel-skeleton--metrics" aria-hidden="true">
        {[0, 1, 2].map((i) => (
          <div key={i} className="panel-skeleton__metric">
            <div className="panel-skeleton__bar panel-skeleton__bar--label" />
            <div className="panel-skeleton__bar panel-skeleton__bar--value" />
            <div className="panel-skeleton__bar panel-skeleton__bar--helper" />
          </div>
        ))}
      </div>
    );
  }
  if (variant === "chart") {
    return (
      <div className="panel-skeleton panel-skeleton--chart" aria-hidden="true">
        <div className="panel-skeleton__curve" />
      </div>
    );
  }
  // table / list
  return (
    <div className="panel-skeleton panel-skeleton--rows" aria-hidden="true">
      {Array.from({ length: rows }).map((_, i) => (
        <div key={i} className="panel-skeleton__row" />
      ))}
    </div>
  );
}
