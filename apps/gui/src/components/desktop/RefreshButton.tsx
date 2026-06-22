interface RefreshButtonProps {
  onRefresh: () => Promise<unknown> | unknown;
  isFetching?: boolean;
}

export function RefreshButton({ onRefresh, isFetching }: RefreshButtonProps) {
  return (
    <button
      className="refresh-button"
      type="button"
      onClick={() => {
        void onRefresh();
      }}
      disabled={isFetching}
      aria-label="Refresh data"
      title="Refresh data"
    >
      <span className={`refresh-button__icon${isFetching ? " refresh-button__icon--spinning" : ""}`}>
        ↻
      </span>
    </button>
  );
}
