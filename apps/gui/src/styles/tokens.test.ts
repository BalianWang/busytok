import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { describe, expect, it } from "vitest";

// Read the CSS source as a string so we can assert against the literal token
// contract. Vitest transforms `.css?raw`/`.css` imports through Vite's CSS
// pipeline, so we read via node:fs at the source path instead.
const tokensCss = readFileSync(
  pathToFileURL("./src/styles/tokens.css"),
  "utf8",
);

describe("tokens.css contract", () => {
  it("defines the required public token API and exact accent scale", () => {
    const css = tokensCss;
    // Exact indigo scale values — light theme
    expect(css).toContain("--color-accent-50: #eef2ff;");
    expect(css).toContain("--color-accent-500: #4f46e5;");
    // Exact indigo scale values — dark theme override
    expect(css).toContain("--color-accent-50: #1e1b4b;");

    // Required roles from each of the five layers
    expect(css).toContain("--color-data-tertiary-soft:");
    expect(css).toContain("--material-glass-saturate:");

    // Other key roles that must exist
    expect(css).toContain("--color-canvas:");
    expect(css).toContain("--color-surface-elevated:");
    expect(css).toContain("--color-border-strong:");
    expect(css).toContain("--color-text-reverse:");
    expect(css).toContain("--color-status-success-soft:");
    expect(css).toContain("--color-data-attention:");
    expect(css).toContain("--color-data-live-primary:");
    expect(css).toContain("--color-data-live-primary-soft:");
    expect(css).toContain("--material-shadow-card:");
    expect(css).toContain("--toggle-track-active:");

    // The cyan secondary identity is removed rather than remapped
    expect(css).not.toContain("--app-secondary");

    // All legacy --app-* migration aliases are gone. This regex matches
    // any `--app-<kebab>:` declaration; the only acceptable appearance
    // of the substring "--app-secondary" is in this comment, not as a
    // CSS declaration.
    expect(css).not.toMatch(/--app-[a-z-]+:/);

    // Reverse/on-accent foreground is explicit, not literal #fff
    expect(css).toContain("--color-text-reverse: rgba(");

    // Radii / spacing / fonts are preserved from the prior token layer
    expect(css).toContain("--radius-pill: 999px;");
    expect(css).toContain("--space-6: 24px;");
    expect(css).toContain("--font-ui:");
  });

  it("defines a dark theme block that overrides key tokens", () => {
    expect(tokensCss).toContain(':root[data-theme="dark"]');
  });

  it("defines dedicated live-curve tokens for both light and dark themes", () => {
    expect(tokensCss).toContain("--color-data-live-primary:");
    expect(tokensCss).toContain("--color-data-live-primary-soft:");

    const darkBlockStart = tokensCss.indexOf(':root[data-theme="dark"]');
    expect(darkBlockStart).toBeGreaterThan(-1);
    const darkBlock = tokensCss.slice(darkBlockStart);
    expect(darkBlock).toContain("--color-data-live-primary:");
    expect(darkBlock).toContain("--color-data-live-primary-soft:");
  });

  it("defines dedicated heatmap tokens (neutral empty + indigo ramp) for both themes", () => {
    // Light theme: empty is a neutral substrate, levels are an indigo ramp
    // that darkens with intensity.
    expect(tokensCss).toContain("--color-heatmap-empty: #ebedf2;");
    expect(tokensCss).toContain("--color-heatmap-level-1: #a5b4fc;");
    expect(tokensCss).toContain("--color-heatmap-level-2: #6366f1;");
    expect(tokensCss).toContain("--color-heatmap-level-3: #4338ca;");
    expect(tokensCss).toContain("--color-heatmap-level-4: #312e81;");

    // Dark theme overrides: empty is a neutral dark slate, ramp brightens.
    const darkBlockStart = tokensCss.indexOf(':root[data-theme="dark"]');
    expect(darkBlockStart).toBeGreaterThan(-1);
    const darkBlock = tokensCss.slice(darkBlockStart);
    expect(darkBlock).toContain("--color-heatmap-empty: #232a35;");
    expect(darkBlock).toContain("--color-heatmap-level-1: #4338ca;");
    expect(darkBlock).toContain("--color-heatmap-level-2: #6366f1;");
    expect(darkBlock).toContain("--color-heatmap-level-3: #818cf8;");
    expect(darkBlock).toContain("--color-heatmap-level-4: #c7d2fe;");
  });

  it("keeps the analytical and live chart ramps distinct in both themes", () => {
    expect(tokensCss).toContain("--color-data-primary: #6671db;");
    expect(tokensCss).toContain("--color-data-primary-soft: rgba(102, 113, 219, 0.2);");
    expect(tokensCss).toContain("--color-data-live-primary: #4f63f6;");
    expect(tokensCss).toContain("--color-data-live-primary-soft: rgba(79, 99, 246, 0.22);");

    const darkBlockStart = tokensCss.indexOf(':root[data-theme="dark"]');
    expect(darkBlockStart).toBeGreaterThan(-1);
    const darkBlock = tokensCss.slice(darkBlockStart);
    expect(darkBlock).toContain("--color-data-primary: #8d9bff;");
    expect(darkBlock).toContain("--color-data-primary-soft: rgba(141, 155, 255, 0.24);");
    expect(darkBlock).toContain("--color-data-live-primary: #a7b8ff;");
    expect(darkBlock).toContain("--color-data-live-primary-soft: rgba(167, 184, 255, 0.3);");
  });

  it("zeroes glass blur across all three ladder tiers in dark theme", () => {
    // Spec: "dark theme replaces glass with surface ladder". The zero-override
    // is load-bearing — every `blur(var(--material-glass-blur*))` consumer
    // (surfaces.css, components.css) cascades through these declarations, so
    // dropping any of them re-introduces a glass blur in dark theme.
    const darkBlockStart = tokensCss.indexOf(':root[data-theme="dark"]');
    expect(darkBlockStart).toBeGreaterThan(-1);
    // The dark block runs to the closing brace; slice it for scoped assertions.
    const darkBlock = tokensCss.slice(darkBlockStart);
    expect(darkBlock).toContain("--material-glass-blur: 0px;");
    expect(darkBlock).toContain("--material-glass-blur-strong: 0px;");
    expect(darkBlock).toContain("--material-glass-blur-subtle: 0px;");
  });
});
