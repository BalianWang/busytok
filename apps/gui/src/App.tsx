//! Busytok GUI root — Surge desktop shell routing between
//! Overview, Usage, Prompt Palette, and Settings.

import { useEffect, useState } from "react";
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { AppShell, type DesktopPage } from "./components/AppShell";
import { OverviewPage } from "./pages/OverviewPage";
import { UsagePage } from "./pages/UsagePage";
import { PromptPalettePage } from "./pages/PromptPalettePage";
import { ProvidersPage } from "./pages/ProvidersPage";
import { SettingsPage } from "./pages/SettingsPage";
import { SubagentsPage } from "./pages/SubagentsPage";
import { EventSubscriptionProvider } from "./api/EventSubscriptionProvider";
import { UpdaterProvider } from "./api/UpdaterProvider";
import { PageToolbarProvider } from "./components/desktop/PageToolbarContext";
import { PromptPaletteOverlayController } from "./components/prompt-palette/PromptPaletteOverlayController";
import {
  flushBuffer,
  hasBufferedLogs,
  reportFrontendEvent,
} from "./logging/reporter";
import { prefetchStartupQueries, useSettingsSnapshot } from "./api/useBusytokData";

const prefetchedQueryClients = new WeakSet<QueryClient>();
const CURRENT_PAGE_STORAGE_KEY = "busytok.desktop.currentPage.v1";
const DESKTOP_PAGES: readonly DesktopPage[] = [
  "overview",
  "usage",
  "prompt_palette",
  "providers",
  "subagents",
  "settings",
];

function isDesktopPage(value: string | null): value is DesktopPage {
  return DESKTOP_PAGES.includes(value as DesktopPage);
}

function loadInitialPage(): DesktopPage {
  try {
    const storedPage = localStorage.getItem(CURRENT_PAGE_STORAGE_KEY);
    return isDesktopPage(storedPage) ? storedPage : "overview";
  } catch {
    return "overview";
  }
}

function persistCurrentPage(page: DesktopPage): void {
  try {
    localStorage.setItem(CURRENT_PAGE_STORAGE_KEY, page);
  } catch {
    // Page persistence is best-effort; navigation state still works in memory.
  }
}

export function App() {
  const [currentPage, setCurrentPage] = useState<DesktopPage>(loadInitialPage);
  const [promptOverlayOpen, setPromptOverlayOpen] = useState(false);
  const queryClient = useQueryClient();
  const settingsSnapshot = useSettingsSnapshot();
  const defaultAction = settingsSnapshot.data?.data.prompt_palette_default_action ?? "CopyAndPaste";

  useEffect(() => {
    if (prefetchedQueryClients.has(queryClient)) {
      return;
    }

    prefetchedQueryClients.add(queryClient);
    prefetchStartupQueries(queryClient);
  }, [queryClient]);

  useEffect(() => {
    reportFrontendEvent({
      level: "INFO",
      event_code: "gui.app_mounted",
      message: "App component mounted",
    });
    if (hasBufferedLogs()) {
      flushBuffer();
    }
    const interval = setInterval(() => {
      if (hasBufferedLogs()) {
        flushBuffer();
      }
    }, 30_000);
    return () => clearInterval(interval);
  }, []);

  useEffect(() => {
    persistCurrentPage(currentPage);
  }, [currentPage]);

  let pageContent: React.ReactNode;
  switch (currentPage) {
    case "overview":
      pageContent = <OverviewPage />;
      break;
    case "usage":
      pageContent = <UsagePage />;
      break;
    case "prompt_palette":
      pageContent = <PromptPalettePage />;
      break;
    case "providers":
      pageContent = <ProvidersPage />;
      break;
    case "subagents":
      pageContent = <SubagentsPage />;
      break;
    case "settings":
      pageContent = <SettingsPage />;
      break;
    default:
      pageContent = <OverviewPage />;
  }

  return (
    <EventSubscriptionProvider>
      <UpdaterProvider>
        <PageToolbarProvider>
          <AppShell currentPage={currentPage} onNavigate={setCurrentPage}>
            {pageContent}
          </AppShell>
        </PageToolbarProvider>
      </UpdaterProvider>
      <PromptPaletteOverlayController
        open={promptOverlayOpen}
        onClose={() => setPromptOverlayOpen(false)}
        defaultAction={defaultAction}
        onOpenPage={() => {
          setCurrentPage("prompt_palette");
          setPromptOverlayOpen(false);
        }}
        onCreateNew={() => {
          setCurrentPage("prompt_palette");
          setPromptOverlayOpen(false);
        }}
      />
    </EventSubscriptionProvider>
  );
}
