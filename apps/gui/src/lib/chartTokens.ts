// ──────────────────────────────────────────────────────────────────────────
// chartTokens — single source of truth for chart colors.
//
// All chart colors must reference semantic CSS custom properties via this
// module. No chart component should hard-code hex/rgba values; doing so
// breaks theme adaptation and reintroduces one-off series colors.
//
// Note on chart-library consumers:
// - CSS/native SVG consumers (e.g. Nivo) should receive the raw `var(--token)`
//   strings from this module so theme switching stays declarative.
// - lightweight-charts consumers must resolve these vars to concrete colour
//   values at runtime and re-apply them on theme changes. The library caches
//   parsed colors internally, so passing raw `var(--token)` strings can leave
//   a chart stuck on stale theme values.
//
// The palette is frozen at module load: being the single source of truth, a
// stray assignment would silently corrupt every chart. `Object.freeze` makes
// such mutation throw (in strict mode) or no-op (in sloppy mode) at runtime,
// and `as const` narrows the values to their literal types at compile time.
// ──────────────────────────────────────────────────────────────────────────

/** Wrap a CSS custom property name in the `var()` function. Local helper. */
const cssVar = (name: string) => `var(${name})`;

/**
 * Shared chart color palette.
 *
 * Maps analytical roles to semantic CSS custom properties:
 * - `linePrimary`: generic trend, ranking bars, overview emphasis.
 * - `livePrimary`: high-signal real-time throughput curve.
 * - `lineSecondary` / `lineTertiary`: comparison and supporting series,
 *   visually distinct from the primary series and NOT reading as
 *   semantic success/warning.
 * - `lineAttention`: transient, partial, and estimated data. Use this for
 *   in-progress analytical states; reserve `status.warning` for genuine
 *   system-health indicators.
 * - `lineNeutral`: inactive / no-data series.
 * - Soft variants: low-opacity fills and gradient stops.
 * - `textMuted` / `borderSubtle`: neutral surface tokens for axis labels,
 *   grid lines, and tooltip text.
 */
export const chartTokens = Object.freeze({
  linePrimary: cssVar("--color-data-primary"),
  linePrimarySoft: cssVar("--color-data-primary-soft"),
  livePrimary: cssVar("--color-data-live-primary"),
  livePrimarySoft: cssVar("--color-data-live-primary-soft"),
  lineSecondary: cssVar("--color-data-secondary"),
  lineSecondarySoft: cssVar("--color-data-secondary-soft"),
  lineTertiary: cssVar("--color-data-tertiary"),
  lineTertiarySoft: cssVar("--color-data-tertiary-soft"),
  lineAttention: cssVar("--color-data-attention"),
  lineNeutral: cssVar("--color-data-neutral"),
  textMuted: cssVar("--color-text-muted"),
  borderSubtle: cssVar("--color-border-subtle"),
} as const);
