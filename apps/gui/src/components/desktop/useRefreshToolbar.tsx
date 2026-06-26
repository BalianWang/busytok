import { useMemo } from "react";
import { RefreshButton } from "./RefreshButton";
import { useRefreshClickHandler } from "./useRefreshClickHandler";
import { useRegisterPageToolbar } from "./PageToolbarContext";

interface UseRefreshToolbarOptions {
  surface: string;
  isFetching?: boolean;
  onRefresh: () => Promise<unknown> | unknown;
}

export function useRefreshToolbar({
  surface,
  isFetching = false,
  onRefresh,
}: UseRefreshToolbarOptions) {
  const handleRefresh = useRefreshClickHandler({ surface, onRefresh });

  const toolbar = useMemo(
    () => (
      <RefreshButton
        onRefresh={handleRefresh}
        isFetching={isFetching}
      />
    ),
    [handleRefresh, isFetching],
  );

  useRegisterPageToolbar(toolbar);
}
