import { beforeEach, describe, expect, it, vi } from "vitest";

// vi.hoisted is the repo's mock idiom (see reporter.test.ts /
// PromptPalettePage.test.tsx): it binds the mock before vi.mock's hoisted
// factory runs, so there is no TDZ on the `reportMock` reference.
const { reportMock } = vi.hoisted(() => ({ reportMock: vi.fn() }));

vi.mock("./reporter", () => ({
  safeReportEvent: reportMock,
}));

import { DESIGN_SYSTEM_VERSION, reportDesignSystemApplied } from "./designSystem";

describe("reportDesignSystemApplied", () => {
  beforeEach(() => {
    reportMock.mockClear();
  });

  it("emits a single gui.design_system.applied INFO event with the version", () => {
    reportDesignSystemApplied();
    expect(reportMock).toHaveBeenCalledTimes(1);
    expect(reportMock).toHaveBeenCalledWith(
      "gui.design_system.applied",
      "Design system token layer applied",
      { version: DESIGN_SYSTEM_VERSION },
    );
  });

  it("never throws (observability must not break bootstrap)", () => {
    reportMock.mockImplementation(() => {
      throw new Error("reporter down");
    });
    expect(() => reportDesignSystemApplied()).not.toThrow();
  });
});
