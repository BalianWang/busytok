import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ReceiptStamp, type ReceiptStampVariant } from "./ReceiptStamp";

afterEach(() => cleanup());

function renderStamp(overrides: Partial<Parameters<typeof ReceiptStamp>[0]> = {}) {
  return render(<ReceiptStamp {...overrides} />);
}

describe("ReceiptStamp", () => {
  it("renders an SVG with the default size and className", () => {
    const { container } = renderStamp({ className: "receipt-stamp" });
    const svg = container.querySelector("svg.receipt-stamp");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("width")).toBe("136");
    expect(svg?.getAttribute("height")).toBe("136");
    expect(svg?.getAttribute("viewBox")).toBe("0 0 200 200");
  });

  it("is marked aria-hidden (decorative)", () => {
    const { container } = renderStamp();
    const svg = container.querySelector("svg");
    expect(svg?.getAttribute("aria-hidden")).toBe("true");
  });

  it("applies opacity and rotation via inline style", () => {
    const { container } = renderStamp({ opacity: 0.3, rotate: -8 });
    const svg = container.querySelector("svg") as SVGElement;
    expect(svg.style.opacity).toBe("0.3");
    expect(svg.style.transform).toBe("rotate(-8deg)");
  });

  it("renders the BUSYTOK center wordmark", () => {
    const { container } = renderStamp();
    const texts = Array.from(container.querySelectorAll("text"));
    const busytok = texts.find((t) => t.textContent === "BUSYTOK");
    expect(busytok).toBeDefined();
    expect(busytok?.getAttribute("font-weight")).toBe("700");
    expect(busytok?.getAttribute("font-size")).toBe("34");
  });

  it("renders the default localFirst variant captions via textPath", () => {
    const { container } = renderStamp();
    const paths = Array.from(container.querySelectorAll("textPath"));
    const texts = paths.map((p) => p.textContent);
    // Top arc reads normally.
    expect(texts).toContain("LOCAL-FIRST");
    // Bottom arc string is reversed (right-to-left path) so the visual
    // reading order is left-to-right. Reversing back gives the caption.
    const bottomRaw = texts.find((t) => t !== "LOCAL-FIRST") ?? "";
    expect([...bottomRaw].reverse().join("")).toBe("TOKEN AUDIT");
  });

  it.each([
    ["localFirst", "LOCAL-FIRST", "TOKEN AUDIT"],
    ["openSource", "OPEN SOURCE", "TOKEN AUDIT"],
    ["generated", "GENERATED", "BY BUSYTOK"],
  ] as Array<[ReceiptStampVariant, string, string]>)(
    "renders the %s variant captions",
    (variant, topText, bottomText) => {
      const { container } = renderStamp({ variant });
      const texts = Array.from(container.querySelectorAll("textPath")).map(
        (p) => p.textContent,
      );
      expect(texts).toContain(topText);
      // Bottom is reversed on the right-to-left arc; reverse back to verify.
      const bottomRaw = texts.find((t) => t !== topText) ?? "";
      expect([...bottomRaw].reverse().join("")).toBe(bottomText);
    },
  );

  it("renders two concentric circles + a divider line (stamp rings)", () => {
    const { container } = renderStamp();
    // Scope to the stroke group (fill="none") — the distress <mask> also
    // contains <circle> elements for missing-ink spots.
    const strokeGroup = container.querySelector('g[fill="none"]');
    const circles = strokeGroup?.querySelectorAll("circle") ?? [];
    expect(circles.length).toBe(2);
    expect(circles[0].getAttribute("r")).toBe("78");
    expect(circles[1].getAttribute("r")).toBe("62");
    const line = strokeGroup?.querySelector("line");
    expect(line).not.toBeNull();
    expect(line?.getAttribute("stroke-width")).toBe("3");
  });

  it("uses the provided color for stroke and fill", () => {
    const { container } = renderStamp({ color: "#B5483D" });
    const strokeGroup = container.querySelector('g[fill="none"]');
    expect(strokeGroup?.getAttribute("stroke")).toBe("#B5483D");
    const fillGroup = container.querySelector('g[fill="#B5483D"]');
    expect(fillGroup).not.toBeNull();
  });

  it("uses a unique ID per instance (no collision between preview + export)", () => {
    const { container: a } = renderStamp();
    const { container: b } = renderStamp();
    const idsA = Array.from(a.querySelectorAll("[id]")).map((el) => el.id);
    const idsB = Array.from(b.querySelectorAll("[id]")).map((el) => el.id);
    // Every ID in the first instance is absent from the second.
    for (const id of idsA) {
      expect(idsB).not.toContain(id);
    }
  });

  it("references its own arc IDs in textPath href", () => {
    const { container } = renderStamp();
    const paths = container.querySelectorAll("path[id]");
    const textPaths = container.querySelectorAll("textPath");
    expect(paths.length).toBeGreaterThanOrEqual(2);
    expect(textPaths.length).toBe(2);
    const arcIds = Array.from(paths).map((p) => p.id);
    for (const tp of textPaths) {
      const href = tp.getAttribute("href") ?? "";
      const refId = href.replace(/^#/, "");
      expect(arcIds).toContain(refId);
    }
  });

  it("bottom arc is drawn right-to-left (so text baseline faces outward)", () => {
    const { container } = renderStamp();
    const paths = Array.from(container.querySelectorAll("path[id]"));
    // Two arc paths: top (left-to-right) and bottom (right-to-left).
    const bottomArc = paths[1];
    const d = bottomArc.getAttribute("d") ?? "";
    // Bottom arc starts at x=158 (right) and ends at x=42 (left).
    expect(d.startsWith("M 158 100")).toBe(true);
    expect(d.includes("42 100")).toBe(true);
  });

  it("includes a distress mask with white background and black spots", () => {
    const { container } = renderStamp();
    const mask = container.querySelector("mask");
    expect(mask).not.toBeNull();
    const whiteRect = mask?.querySelector("rect[fill='white']");
    expect(whiteRect).not.toBeNull();
    const blackSpots = mask?.querySelectorAll("circle[fill='black'], rect[fill='black']");
    expect(blackSpots?.length).toBeGreaterThan(0);
  });

  it("includes the roughen filter (feTurbulence + feDisplacementMap)", () => {
    const { container } = renderStamp();
    const filter = container.querySelector("filter");
    expect(filter).not.toBeNull();
    expect(filter?.querySelector("feTurbulence")).not.toBeNull();
    expect(filter?.querySelector("feDisplacementMap")).not.toBeNull();
  });

  it("accepts a custom size", () => {
    const { container } = renderStamp({ size: 148 });
    const svg = container.querySelector("svg");
    expect(svg?.getAttribute("width")).toBe("148");
    expect(svg?.getAttribute("height")).toBe("148");
  });
});
