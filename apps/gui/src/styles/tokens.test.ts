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
    expect(css).toContain("--color-border-strong:");
    expect(css).toContain("--color-text-reverse:");
    expect(css).toContain("--color-status-success-soft:");
    expect(css).toContain("--color-data-attention:");
    expect(css).toContain("--color-data-live-primary:");
    expect(css).toContain("--color-data-live-primary-soft:");
    expect(css).toContain("--material-shadow-card:");
    expect(css).toContain("--toggle-track-active:");
    // Navigation token: dedicated semantic for primary-nav rest state,
    // distinct from secondary/helper copy (--color-text-muted).
    expect(css).toContain("--color-nav-text:");

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

    // Phase 2 Task 5: section-gap rhythm token for the overview shell
    expect(css).toContain("--space-section-gap: 24px;");

    // Phase 1 rename: new vocabulary exists, old names are gone.
    expect(css).toContain("--color-border-subtle:");
    expect(css).toContain("--color-surface-subtle:");
    expect(css).toContain("--color-chrome:");
    expect(css).toContain("--color-hover:");
    expect(css).toContain("--color-hover-strong:");
    expect(css).not.toContain("--color-border-soft:");
    expect(css).not.toContain("--color-canvas-subtle:");
    expect(css).not.toContain("--color-sidebar:");
  });

  it("defines a dark theme block that overrides key tokens", () => {
    expect(tokensCss).toContain(':root[data-theme="dark"]');
    const darkStart = tokensCss.indexOf(':root[data-theme="dark"]');
    expect(darkStart).toBeGreaterThan(-1);
    const dark = tokensCss.slice(darkStart);
    expect(dark).toContain("--color-nav-text:");
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
    expect(tokensCss).toContain("--color-heatmap-empty: #EDF0F3;");
    expect(tokensCss).toContain("--color-heatmap-level-1: #a5b4fc;");
    expect(tokensCss).toContain("--color-heatmap-level-2: #6366f1;");
    expect(tokensCss).toContain("--color-heatmap-level-3: #4338ca;");
    expect(tokensCss).toContain("--color-heatmap-level-4: #312e81;");

    // Dark theme overrides: empty is a neutral dark slate, ramp brightens.
    const darkBlockStart = tokensCss.indexOf(':root[data-theme="dark"]');
    expect(darkBlockStart).toBeGreaterThan(-1);
    const darkBlock = tokensCss.slice(darkBlockStart);
    expect(darkBlock).toContain("--color-heatmap-empty: #202732;");
    expect(darkBlock).toContain("--color-heatmap-level-1: #4338ca;");
    expect(darkBlock).toContain("--color-heatmap-level-2: #6366f1;");
    expect(darkBlock).toContain("--color-heatmap-level-3: #818cf8;");
    expect(darkBlock).toContain("--color-heatmap-level-4: #c7d2fe;");
  });

  it("keeps the analytical and live chart ramps distinct in both themes", () => {
    expect(tokensCss).toContain("--color-data-primary: #6671db;");
    expect(tokensCss).toContain("--color-data-primary-soft: rgba(102, 113, 219, 0.2);");
    expect(tokensCss).toContain("--color-data-live-primary: #4f63f6;");
    expect(tokensCss).toContain("--color-data-live-primary-soft: rgba(79, 99, 246, 0.08);");

    const darkBlockStart = tokensCss.indexOf(':root[data-theme="dark"]');
    expect(darkBlockStart).toBeGreaterThan(-1);
    const darkBlock = tokensCss.slice(darkBlockStart);
    expect(darkBlock).toContain("--color-data-primary: #8d9bff;");
    expect(darkBlock).toContain("--color-data-primary-soft: rgba(141, 155, 255, 0.24);");
    expect(darkBlock).toContain("--color-data-live-primary: #a7b8ff;");
    expect(darkBlock).toContain("--color-data-live-primary-soft: rgba(167, 184, 255, 0.10);");
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

  it("light theme: opaque surfaces, chrome vibrancy, Geist shadow (Phase 1)", () => {
    // content surface is opaque white, not translucent
    expect(tokensCss).toContain("--color-surface: #FFFFFF;");
    expect(tokensCss).toContain("--color-surface-subtle: #F7F8FA;");
    expect(tokensCss).toContain("--color-canvas: #F4F5F7;");
    expect(tokensCss).toContain("--color-chrome: rgba(255, 255, 255, 0.94);");
    // collapsed tiers are gone
    expect(tokensCss).not.toContain("--color-surface-strong:");
    expect(tokensCss).not.toContain("--color-surface-elevated:");
    // chrome blur only; subtle (content/scrim) blur is 0 per spec §4.1 —
    // modal-backdrop blur becomes a Phase 2 per-component concern
    expect(tokensCss).toContain("--material-glass-blur: 8px;");
    expect(tokensCss).toContain("--material-glass-blur-strong: 8px;");
    expect(tokensCss).toContain("--material-glass-blur-subtle: 0px;");
    // dead translucent-alpha tokens removed (no consumers)
    expect(tokensCss).not.toContain("--material-surface-alpha:");
    expect(tokensCss).not.toContain("--material-surface-strong-alpha:");
    // Geist raised-card shadow
    expect(tokensCss).toContain(
      "--material-shadow-card: 0 2px 2px rgba(15, 23, 42, 0.04);",
    );
    // text de-blued
    expect(tokensCss).toContain("--color-text: #1A1D23;");
  });

  it("dark theme: opaque surfaces, Geist shadow, blur stays zero (Phase 1)", () => {
    const start = tokensCss.indexOf(':root[data-theme="dark"]');
    expect(start).toBeGreaterThan(-1);
    const dark = tokensCss.slice(start);
    expect(dark).toContain("--color-surface: #171C24;");
    expect(dark).toContain("--color-surface-subtle: #202732;");
    expect(dark).toContain("--color-chrome: rgba(22, 27, 34, 0.96);");
    expect(dark).toContain("--material-shadow-card: 0 1px 2px rgba(0, 0, 0, 0.16);");
    // dark blur remains zero (supporting-only → 0 for maximum calm)
    expect(dark).toContain("--material-glass-blur: 0px;");
    expect(dark).toContain("--material-glass-blur-strong: 0px;");
    expect(dark).toContain("--material-glass-blur-subtle: 0px;");
    // collapsed tiers gone from dark block too
    expect(dark).not.toContain("--color-surface-strong:");
    expect(dark).not.toContain("--color-surface-elevated:");
  });

  it("radius role map: 6/12/16/pill, xs and xl removed (Phase 1)", () => {
    expect(tokensCss).toContain("--radius-sm: 6px;");
    expect(tokensCss).toContain("--radius-md: 12px;");
    expect(tokensCss).toContain("--radius-lg: 16px;");
    expect(tokensCss).toContain("--radius-pill: 999px;");
    expect(tokensCss).not.toContain("--radius-xs:");
    expect(tokensCss).not.toContain("--radius-xl:");
  });

  it("elevated shadow is the Geist popover stack (floating layers only)", () => {
    const popover =
      "0 1px 1px rgba(0, 0, 0, 0.02), 0 4px 8px -4px rgba(0, 0, 0, 0.04), 0 16px 24px -8px rgba(0, 0, 0, 0.06)";
    expect(tokensCss).toContain(`--material-shadow-elevated: ${popover};`);
  });
});
