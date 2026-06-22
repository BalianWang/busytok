import { useCallback, useMemo } from "react";
import { reportFrontendEventSafely } from "../../logging/safeReporter";
import { RefreshButton } from "./RefreshButton";
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
  const handleRefresh = useCallback(async () => {
    reportFrontendEventSafely({
      level: "INFO",
      event_code: "gui.refresh.requested",
      message: "User requested page refresh from titlebar",
      details: {
        surface,
        trigger: "titlebar",
      },
    });

    try {
      await onRefresh();
      reportFrontendEventSafely({
        level: "INFO",
        event_code: "gui.refresh.succeeded",
        message: "Page refresh completed from titlebar",
        details: {
          surface,
          trigger: "titlebar",
        },
      });
    } catch (error) {
      reportFrontendEventSafely({
        level: "ERROR",
        event_code: "gui.refresh.failed",
        message: "Page refresh failed from titlebar",
        details: {
          surface,
          trigger: "titlebar",
          error_message: error instanceof Error ? error.message : String(error),
        },
      });
    }
  }, [onRefresh, surface]);

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
