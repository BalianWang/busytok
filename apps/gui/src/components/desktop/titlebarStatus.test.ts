import { describe, expect, it } from "vitest";
import { deriveTitlebarStatus, type TitlebarStatusInput } from "./titlebarStatus";

function baseInput(over: Partial<TitlebarStatusInput> = {}): TitlebarStatusInput {
  return {
    readiness: "ready_exact",
    statusChips: [],
    connection: "connected",
    queueDepth: null,
    aggregateLagMs: null,
    generatedAtMs: 1_000,
    ...over,
  };
}

describe("deriveTitlebarStatus", () => {
  it("is neutral/healthy when ready, connected, no queue, no lag, no warning chips", () => {
    const s = deriveTitlebarStatus(baseInput());
    expect(s.tone).toBe("neutral");
    expect(s.label).toBe("Live capture active");
    expect(s.dotToken).toBe("var(--color-status-success)");
    expect(s.auxiliary).toBeUndefined();
  });

  it("escalates to warning on ready_degraded", () => {
    const s = deriveTitlebarStatus(baseInput({ readiness: "ready_degraded" }));
    expect(s.tone).toBe("warning");
    expect(s.label).toBe("Degraded");
  });

  it("escalates to warning on reconnecting (backlog/connection)", () => {
    expect(deriveTitlebarStatus(baseInput({ connection: "reconnecting" })).tone).toBe("warning");
    expect(deriveTitlebarStatus(baseInput({ queueDepth: 42 })).tone).toBe("warning");
  });

  it("escalates to warning on aggregate lag >= warning threshold", () => {
    expect(deriveTitlebarStatus(baseInput({ aggregateLagMs: 6_000 })).tone).toBe("warning");
  });

  it("keeps a perceivable-but-non-blocking warning chip (e.g. budget) as a single warning, NOT +1 danger", () => {
    const s = deriveTitlebarStatus(
      baseInput({ statusChips: [{ id: "budget", label: "Budget at 90%", tone: "warning", detail: null, action: null }] }),
    );
    expect(s.tone).toBe("warning");
    expect(s.auxiliary).toBeUndefined();
  });

  it("adds the +1 danger auxiliary only for an allowlisted blocking chip (scan offline = service down)", () => {
    // The backend (supervisor.rs:1397) emits exactly one danger-tone chip:
    // id "scan" when scan_state == "offline". Only allowlisted ids get +1.
    const s = deriveTitlebarStatus(
      baseInput({ statusChips: [{ id: "scan", label: "Service offline", tone: "danger", detail: "Realtime capture is not running", action: null }] }),
    );
    expect(s.tone).toBe("warning"); // primary stays the consolidated status
    expect(s.auxiliary).toBeDefined();
    expect(s.auxiliary?.tone).toBe("danger");
    expect(s.auxiliary?.label).toBe("Service offline");
  });

  it("does NOT +1 a non-allowlisted danger chip (perceivable-non-blocking stays single warning, no auxiliary)", () => {
    const s = deriveTitlebarStatus(
      baseInput({ statusChips: [{ id: "budget", label: "Budget at 90%", tone: "danger", detail: null, action: null }] }),
    );
    expect(s.auxiliary).toBeUndefined();
  });

  it("exposes read-only popover sections (Service / Live) and existing nav actions only", () => {
    const s = deriveTitlebarStatus(
      baseInput({ readiness: "ready_degraded", connection: "reconnecting", queueDepth: 7, aggregateLagMs: 6_000 }),
    );
    const sectionLabels = s.sections.map((sec) => sec.label);
    expect(sectionLabels).toEqual(["SERVICE", "LIVE"]);
    const live = s.sections.find((sec) => sec.label === "LIVE")!;
    const rowLabels = live.rows.map((r) => r.label);
    expect(rowLabels).toEqual(["Connection", "Queue depth", "Aggregate lag"]);
    // actions are the existing read-only nav actions, nothing new
    expect(s.actions.every((a) => a.action === "open_activity" || a.action === "open_settings")).toBe(true);
  });

  it("label shortens to 'Capture active' fallback via separate field (窄宽)", () => {
    const s = deriveTitlebarStatus(baseInput());
    expect(s.labelShort).toBe("Capture active");
  });
});
