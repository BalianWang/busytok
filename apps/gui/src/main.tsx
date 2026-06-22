import { StrictMode } from "react";
import "./styles/tokens.css";
import "./styles/surfaces.css";
import "./styles/components.css";
import "./styles/pages.css";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { App } from "./App";
import { PromptPaletteWindowApp } from "./PromptPaletteWindowApp";
import {
  getSessionId,
  reportFrontendError,
  reportFrontendEvent,
} from "./logging/reporter";
import { initThemeRuntime } from "./lib/themeRuntime";
import { initUpdaterAutoCheck } from "./lib/updaterClient";

function isPromptPaletteWindow() {
  return (
    (window as Window & { __BUSYTOK_PANEL_CONTEXT?: boolean })
      .__BUSYTOK_PANEL_CONTEXT === true ||
    new URLSearchParams(window.location.search).get("window") === "prompt-palette"
  );
}

const promptPaletteWindow = isPromptPaletteWindow();
if (promptPaletteWindow) {
  document.body.classList.add("prompt-palette-window-body");
}

// Initialize session_id before first render
getSessionId();
reportFrontendEvent({
  level: "INFO",
  event_code: "gui.frontend_bootstrap_begin",
  message: "Frontend bootstrap starting",
});

// Start the theme runtime once, before React mounts. Both window types share
// document.documentElement, so one init covers App and prompt-palette windows.
// StrictMode double-mount in development will not re-run this module-level init.
initThemeRuntime();

// Tauri 2 updater silent auto-check. Module-level latch survives StrictMode.
// MAIN APP ONLY — prompt-palette must not trigger update probes.
if (!promptPaletteWindow) {
  initUpdaterAutoCheck();
}

// Global error handlers — installed before React mounts.
window.addEventListener(
  "error",
  (event: Event) => {
    if (event instanceof ErrorEvent) {
      reportFrontendError({
        event_code: "gui.unhandled_error",
        message: event.message,
        details: {
          filename: event.filename || undefined,
          lineno: event.lineno || undefined,
          colno: event.colno || undefined,
          stack: event.error instanceof Error ? event.error.stack : undefined,
        },
      });
      return;
    }

    reportFrontendError({
      event_code: "gui.resource_load_error",
      message: "A frontend resource failed to load",
      details:
        event.target instanceof HTMLElement
          ? {
              tagName: event.target.tagName,
              id: event.target.id || undefined,
              className: event.target.className || undefined,
              url:
                (event.target as HTMLElement & { src?: string; href?: string })
                  .src ||
                (event.target as HTMLElement & { src?: string; href?: string })
                  .href ||
                undefined,
            }
          : { tagName: "unknown" },
    });
  },
  true,
);

window.addEventListener("unhandledrejection", (event) => {
  const reason = event.reason;
  reportFrontendError({
    event_code: "gui.unhandled_rejection",
    message:
      typeof reason === "string"
        ? reason
        : reason?.message ?? String(reason),
    details: {
      reason_type: typeof reason,
      reason_stack: reason instanceof Error ? reason.stack : undefined,
    },
  });
});

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 3,
      retryDelay: (attemptIndex) => Math.min(1000 * 2 ** attemptIndex, 10000),
      refetchOnWindowFocus: true,
    },
  },
});

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ErrorBoundary>
      <QueryClientProvider client={queryClient}>
        {promptPaletteWindow ? <PromptPaletteWindowApp /> : <App />}
      </QueryClientProvider>
    </ErrorBoundary>
  </StrictMode>,
);

reportFrontendEvent({
  level: "INFO",
  event_code: "gui.frontend_bootstrap_rendered",
  message: "React root rendered",
});
