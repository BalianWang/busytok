import { useCallback } from "react";
import { reportFrontendEventSafely } from "../../logging/safeReporter";

export interface RefreshHandlerOptions {
  surface: string;
  onRefresh: () => Promise<unknown> | unknown;
}

export function useRefreshClickHandler({ surface, onRefresh }: RefreshHandlerOptions) {
  return useCallback(async () => {
    reportFrontendEventSafely({
      level: "INFO",
      event_code: "gui.refresh.requested",
      message: "User requested page refresh from titlebar",
      details: { surface, trigger: "titlebar" },
    });
    try {
      await onRefresh();
      reportFrontendEventSafely({
        level: "INFO",
        event_code: "gui.refresh.succeeded",
        message: "Page refresh completed from titlebar",
        details: { surface, trigger: "titlebar" },
      });
    } catch (error) {
      reportFrontendEventSafely({
        level: "ERROR",
        event_code: "gui.refresh.failed",
        message: "Page refresh failed",
        details: {
          surface,
          trigger: "titlebar",
          error_message: error instanceof Error ? error.message : String(error),
        },
      });
    }
  }, [onRefresh, surface]);
}
