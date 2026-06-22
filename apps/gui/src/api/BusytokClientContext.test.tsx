import { describe, expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";
import { BusytokClientProvider, useBusytokClient } from "./BusytokClientContext";

const mockClient = {
  shellStatus: vi.fn(),
  overviewSummary: vi.fn(),
  overviewTrend: vi.fn(),
  overviewHeatmap: vi.fn(),
  overviewRankings: vi.fn(),
  activityRecent: vi.fn(),
  activityList: vi.fn(),
  activityDetail: vi.fn(),
  breakdownList: vi.fn(),
  breakdownDetail: vi.fn(),
  settingsSnapshot: vi.fn(),
  settingsUpdate: vi.fn(),
  settingsDiagnostics: vi.fn(),
  settingsRecoveryAction: vi.fn(),
  promptsList: vi.fn(),
  promptsGet: vi.fn(),
  promptsCreate: vi.fn(),
  promptsUpdate: vi.fn(),
  promptsDelete: vi.fn(),
  promptsUse: vi.fn(),
  liveWindow: vi.fn(),
} as const;

describe("BusytokClientContext", () => {
  it("returns default client when no provider", () => {
    const { result } = renderHook(() => useBusytokClient());
    // The default client is the module-level busytokClient singleton
    expect(result.current).toBeDefined();
    expect(typeof result.current.shellStatus).toBe("function");
  });

  it("returns panel client when wrapped in provider", () => {
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <BusytokClientProvider client={mockClient as any}>
        {children}
      </BusytokClientProvider>
    );
    const { result } = renderHook(() => useBusytokClient(), { wrapper });
    expect(result.current).toBe(mockClient);
  });
});
