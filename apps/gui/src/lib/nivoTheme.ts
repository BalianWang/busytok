// Shared Nivo theme for all charts. Uses the Busytok semantic token contract
// (tokens.css) so colours adapt to the app theme automatically. All chart
// colors come from `chartTokens` — no chart component or theme file should
// hard-code hex/rgba values.

import { chartTokens } from "./chartTokens";

export const nivoTheme = {
  background: "transparent",

  text: {
    fill: "var(--color-text)",
    fontSize: 14,
    fontFamily: "var(--font-ui)",
  },

  axis: {
    ticks: {
      text: { fill: chartTokens.textMuted, fontSize: 12 },
      line: { stroke: "var(--color-border)", strokeWidth: 1 },
    },
    legend: {
      text: { fill: chartTokens.textMuted, fontSize: 14 },
    },
  },

  grid: {
    line: { stroke: chartTokens.borderSoft, strokeWidth: 1 },
  },

  tooltip: {
    container: {
      background: "var(--color-surface)",
      color: "var(--color-text)",
      fontSize: "16px",
      borderRadius: "12px",
      boxShadow: "var(--material-shadow-elevated)",
    },
  },
};

interface GradientDef {
  id: string;
  type: "linearGradient";
  colors: Array<{ offset: number; color: string }>;
}

/**
 * Default bar gradient — indigo data-primary ramp, full strength at the
 * top fading into a soft tint at the bottom. Replaces the legacy hard-coded
 * purple ramp (#5C58C3 / rgba(92, 88, 195, ...)).
 */
export const DEFAULT_BAR_GRADIENT: GradientDef = {
  id: "defaultBarGradient",
  type: "linearGradient",
  colors: [
    { offset: 0, color: chartTokens.linePrimary },
    { offset: 100, color: chartTokens.linePrimarySoft },
  ],
};

/**
 * Active bar gradient — used to highlight the currently-selected bar.
 * Slightly stronger indigo emphasis (deeper primary → primary) so the
 * selection reads as "more saturated" rather than switching hue family.
 * Replaces the legacy #3D3899 / #7C78E8 ramp.
 */
export const ACTIVE_BAR_GRADIENT: GradientDef = {
  id: "activeBarGradient",
  type: "linearGradient",
  colors: [
    { offset: 0, color: "var(--color-accent-700)" },
    { offset: 100, color: chartTokens.linePrimary },
  ],
};
