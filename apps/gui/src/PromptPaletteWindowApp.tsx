import { useCallback, useEffect, useRef, useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { PromptPaletteOverlayController } from "./components/prompt-palette/PromptPaletteOverlayController";
import { BusytokClientProvider } from "./api/BusytokClientContext";
import { panelBusytokClient } from "./api/panelBusytokClient";
import { useSettingsSnapshot } from "./api/useBusytokData";
import { PanelEventSubscriptionProvider } from "./api/PanelEventSubscriptionProvider";
import { requestPanelClose, requestShowGui } from "./lib/panelWindowOps";
import { reportFrontendEvent } from "./logging/reporter";

declare global {
  interface Window {
    __busytokPanelBridgeDiagnostic?: (
      name: string,
      details?: Record<string, unknown>,
    ) => void;
  }
}

function promptPaletteErrorReason(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function runWindowAction(
  action: () => Promise<void>,
  failure: { event_code: string; message: string },
) {
  void action().catch((error) => {
    reportFrontendEvent({
      level: "WARN",
      ...failure,
      details: { reason: promptPaletteErrorReason(error) },
    });
  });
}

// Separate QueryClient (not shared with the main GUI window) because the panel
// runs in its own WKWebView with an isolated JS context — there is no shared
// cache to synchronize with.
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 1,
      refetchOnWindowFocus: true,
    },
  },
});

// Log the panel's resolved theme exactly once per mount lifecycle. main.tsx
// calls initThemeRuntime() before React mounts, so <html data-theme> is set
// before this effect runs. We log inside a useEffect (not at module scope)
// so the emission does not race test reporter mocks during module import,
// and we guard with a ref so StrictMode's dev double-invoke of effects
// does not double-fire.
function PromptPalettePanelInner() {
  const [sessionKey, setSessionKey] = useState(0);
  const settingsSnapshot = useSettingsSnapshot();
  const defaultAction = settingsSnapshot.data?.data.prompt_palette_default_action ?? "Copy&Paste";
  const themeLoggedRef = useRef(false);

  const closePaletteWindow = useCallback(async () => {
    setSessionKey((current) => current + 1);
    reportFrontendEvent({
      level: "INFO",
      event_code: "gui.prompt_palette.panel_close_requested",
      message: "Prompt Palette panel close requested from React",
    });
    await requestPanelClose();
  }, []);

  useEffect(() => {
    if (themeLoggedRef.current) return;
    themeLoggedRef.current = true;
    reportFrontendEvent({
      level: "INFO",
      event_code: "gui.prompt_palette.panel_theme_resolved",
      message: "Prompt Palette panel resolved theme",
      details: {
        resolved_theme: document.documentElement.dataset.theme ?? "unknown",
      },
    });
  }, []);

  useEffect(() => {
    reportFrontendEvent({
      level: "INFO",
      event_code: "gui.prompt_palette.panel_app_mounted",
      message: "Prompt Palette panel React app mounted",
      details: {
        href: window.location.href,
        body_class: document.body.className,
        root_child_count: document.getElementById("root")?.childElementCount ?? -1,
      },
    });
    window.__busytokPanelBridgeDiagnostic?.("react_app_mounted", {
      bodyClass: document.body.className,
      rootChildCount: document.getElementById("root")?.childElementCount ?? -1,
    });

    const timer = window.setTimeout(() => {
      const root = document.getElementById("root");
      reportFrontendEvent({
        level: "INFO",
        event_code: "gui.prompt_palette.panel_app_after_paint",
        message: "Prompt Palette panel React app paint probe",
        details: {
          body_text_length: document.body.innerText?.length ?? 0,
          root_text_preview: root?.innerText.slice(0, 160) ?? "",
          active_element: document.activeElement?.tagName,
        },
      });
    }, 250);

    return () => window.clearTimeout(timer);
  }, []);

  // React-level Escape handler as a second layer on top of the bootstrap JS
  // listener (BRIDGE_BOOTSTRAP_JS in palette_native.rs). The bootstrap
  // listener fires even before React mounts, so no Escape press is lost.
  // Both handlers call requestPanelClose, which is idempotent — duplicate
  // calls are harmless.
  useEffect(() => {
    function handleWindowKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") {
        return;
      }

      reportFrontendEvent({
        level: "INFO",
        event_code: "gui.prompt_palette.panel_escape_keydown",
        message: "Prompt Palette panel Escape keydown reached React window listener",
        details: {
          active_element: document.activeElement?.tagName,
        },
      });
      event.preventDefault();
      event.stopPropagation();
      void closePaletteWindow();
    }

    window.addEventListener("keydown", handleWindowKeyDown, { capture: true });
    return () => {
      window.removeEventListener("keydown", handleWindowKeyDown, { capture: true });
    };
  }, [closePaletteWindow]);

  return (
    <PanelEventSubscriptionProvider>
      <PromptPaletteOverlayController
        key={sessionKey}
        open
        presentation="window"
        defaultAction={defaultAction}
        onClose={() => {
          void closePaletteWindow();
        }}
        onOpenPage={() => {
          runWindowAction(
            async () => {
              await requestShowGui();
              await requestPanelClose();
            },
            {
              event_code: "gui.prompt_palette.open_management_failed",
              message: "Prompt Palette could not open the management page",
            },
          );
        }}
        onCreateNew={() => {
          runWindowAction(
            async () => {
              await requestShowGui();
              await requestPanelClose();
            },
            {
              event_code: "gui.prompt_palette.open_create_failed",
              message: "Prompt Palette could not open the create prompt flow",
            },
          );
        }}
      />
    </PanelEventSubscriptionProvider>
  );
}

export function PromptPaletteWindowApp() {
  return (
    <QueryClientProvider client={queryClient}>
      <BusytokClientProvider client={panelBusytokClient}>
        <PromptPalettePanelInner />
      </BusytokClientProvider>
    </QueryClientProvider>
  );
}
