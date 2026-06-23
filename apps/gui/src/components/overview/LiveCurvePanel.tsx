import { useEffect, useMemo, useRef } from "react";
import {
  createChart,
  AreaSeries,
  type IChartApi,
  type ISeriesApi,
  type AreaData,
  type Time,
  ColorType,
  CrosshairMode,
} from "lightweight-charts";
import * as LightweightCharts from "lightweight-charts";
import { useLiveSamples } from "../../api/useLiveSamples";
import { useEventSubscription } from "../../api/useEventSubscription";
import { buildDisplayLiveCurveSamples } from "../../api/liveSmoothing";
import { chartTokens } from "../../lib/chartTokens";

function formatMinuteLabel(bucketStartMs: number): string {
  const d = new Date(bucketStartMs);
  return d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

function resolveCssColor(name: string, fallback: string): string {
  const el = document.documentElement;
  const computed = getComputedStyle(el).getPropertyValue(name).trim();
  return computed || fallback;
}

function resolveLiveCurveThemeColors() {
  // Chart stroke + fill both derive from the live-primary token; the fill is
  // kept ≤8% so the line reads as a system readout, not a marketing glow.
  const line = resolveCssColor("--color-data-live-primary", "#4f63f6");
  return {
    lineColor: line,
    topColor: resolveCssColor("--color-data-live-primary-soft", "rgba(79, 99, 246, 0.08)"),
    textColor: resolveCssColor("--color-text-muted", "#6e7480"),
    gridColor: resolveCssColor("--color-border-subtle", "rgba(17, 24, 39, 0.06)"),
  };
}

const AREA_BOTTOM_COLOR = "transparent";
const LIVE_WINDOW_SECONDS = 15 * 60;
const CURVED_LINE_TYPE =
  "LineType" in LightweightCharts ? LightweightCharts.LineType.Curved : 2;
const DOTTED_LINE_STYLE =
  "LineStyle" in LightweightCharts ? LightweightCharts.LineStyle.Dotted : 1;

function liveVisibleRange(): { from: Time; to: Time } {
  const to = Math.floor(Date.now() / 1000);
  return {
    from: (to - LIVE_WINDOW_SECONDS) as Time,
    to: to as Time,
  };
}

function lockToLiveWindow(chart: IChartApi): void {
  chart.timeScale().setVisibleRange(liveVisibleRange());
}

export function LiveCurvePanel() {
  const containerRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<IChartApi | null>(null);
  const tokensSeriesRef = useRef<ISeriesApi<"Area"> | null>(null);
  const { samples, smoothedSamples, isLoading, hasTransient } = useLiveSamples();
  const { connectionStatus } = useEventSubscription();
  const displaySamples = useMemo(
    () => buildDisplayLiveCurveSamples(smoothedSamples),
    [smoothedSamples],
  );

  // Initialize chart once.
  useEffect(() => {
    if (!containerRef.current || chartRef.current) return;

    const themeColors = resolveLiveCurveThemeColors();
    const chart = createChart(containerRef.current, {
      autoSize: true,
      height: 220,
      handleScroll: false,
      handleScale: false,
      layout: {
        background: { type: ColorType.Solid, color: "transparent" },
        textColor: themeColors.textColor,
        fontSize: 11,
        attributionLogo: false,
      },
      // Vertical grid disabled; horizontal grid reduced to ~4 subtle reference
      // lines (lightweight-charts auto-distributes horizontals; a dotted style
      // reads them as discrete reference marks rather than a full grid).
      grid: {
        vertLines: { visible: false },
        horzLines: { color: themeColors.gridColor, style: DOTTED_LINE_STYLE },
      },
      crosshair: {
        mode: CrosshairMode.Magnet,
        horzLine: { visible: false },
        vertLine: { labelVisible: false },
      },
      timeScale: {
        timeVisible: true,
        secondsVisible: false,
        uniformDistribution: true,
        tickMarkFormatter: (time: number) => formatMinuteLabel((time as number) * 1000),
      },
      rightPriceScale: {
        borderColor: themeColors.gridColor,
        scaleMargins: { top: 0.05, bottom: 0.05 },
      },
    });

    const tokensSeries = chart.addSeries(AreaSeries, {
      lineColor: themeColors.lineColor,
      topColor: themeColors.topColor,
      bottomColor: AREA_BOTTOM_COLOR,
      lineWidth: 2,
      lineType: CURVED_LINE_TYPE,
      priceFormat: { type: "volume", precision: 1 },
      title: "tokens/min",
    });

    chartRef.current = chart;
    tokensSeriesRef.current = tokensSeries;

    return () => {
      chart.remove();
      chartRef.current = null;
    };
  }, []);

  // lightweight-charts caches parsed colors internally, so theme switches
  // need explicit re-application with freshly resolved concrete colors.
  useEffect(() => {
    if (!chartRef.current || !tokensSeriesRef.current) return;

    const applyThemeColors = () => {
      const themeColors = resolveLiveCurveThemeColors();
      chartRef.current?.applyOptions({
        layout: { textColor: themeColors.textColor },
        grid: {
          vertLines: { visible: false },
          horzLines: { color: themeColors.gridColor, style: DOTTED_LINE_STYLE },
        },
        rightPriceScale: { borderColor: themeColors.gridColor },
      });
      tokensSeriesRef.current?.applyOptions({
        lineColor: themeColors.lineColor,
        topColor: themeColors.topColor,
        bottomColor: AREA_BOTTOM_COLOR,
      });
    };

    const observer = new MutationObserver((mutations) => {
      for (const mutation of mutations) {
        if (mutation.type === "attributes" && mutation.attributeName === "data-theme") {
          applyThemeColors();
          break;
        }
      }
    });

    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });

    return () => observer.disconnect();
  }, []);

  // Update chart data when samples change.
  useEffect(() => {
    if (!tokensSeriesRef.current) return;

    const tokenData: AreaData[] = [];
    for (const sample of displaySamples) {
      const time = (sample.bucket_start_ms / 1000) as Time;
      tokenData.push({ time, value: sample.display_tokens_per_min });
    }

    tokensSeriesRef.current.setData(tokenData);
    if (chartRef.current && displaySamples.length > 0) {
      lockToLiveWindow(chartRef.current);
    }
  }, [displaySamples]);

  // ── Connection pill (Live / transient / reconnecting) ──────────────────
  // Steady-state Live is the chart's dedicated live-series color so the pill
  // matches the throughput line. Transient state describes in-progress
  // analytical data — it uses data.attention, not status.warning, because the
  // UI is not warning about a product/system problem. Disconnected falls back
  // to the muted text token so the pill recedes without signaling failure.
  const connectionLabel =
    connectionStatus === "connected"
      ? hasTransient
        ? "Live (partial)"
        : "● Live"
      : `○ ${connectionStatus}`;

  const connectionColor =
    connectionStatus === "connected"
      ? hasTransient
        ? chartTokens.lineAttention
        : chartTokens.livePrimary
      : chartTokens.textMuted;

  return (
    <div className="live-curve-panel">
      <div className="live-curve-panel__header">
        <h2>Real-time Throughput</h2>
        <span className="live-curve-panel__status" style={{ color: connectionColor }}>
          {connectionLabel}
        </span>
      </div>

      {hasTransient && (
        <div
          className="live-curve-panel__transient-banner"
          role="status"
          aria-label="Live data includes transient samples"
          style={{
            fontSize: 11,
            color: chartTokens.lineAttention,
            background: "color-mix(in srgb, var(--color-data-attention) 12%, transparent)",
            borderRadius: 8,
            padding: "4px 12px",
            marginBottom: 12,
          }}
        >
          Chart includes transient samples — points beyond the exact watermark
          are live estimates that will be replaced by exact aggregates.
        </div>
      )}

      <div
        className="live-curve-panel__chart-frame"
        data-testid="live-curve-chart-frame"
        style={{ position: "relative", flex: 1, minHeight: 220 }}
      >
        <div ref={containerRef} style={{ width: "100%", height: "100%" }} />
        {isLoading && (
          <div
            style={{
              position: "absolute",
              inset: 0,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              color: chartTokens.textMuted,
              background: "color-mix(in srgb, var(--color-surface) 60%, transparent)",
            }}
          >
            Loading...
          </div>
        )}
        {!isLoading && samples.length === 0 && (
          <div
            style={{
              position: "absolute",
              inset: 0,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              color: chartTokens.textMuted,
              fontSize: 12,
              pointerEvents: "none",
            }}
          >
            Waiting for recent token activity...
          </div>
        )}
      </div>
    </div>
  );
}
