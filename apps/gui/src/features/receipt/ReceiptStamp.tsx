import { useId } from "react";

/**
 * Variant of the stamp's outer-ring caption pair.
 * - `localFirst`  → "LOCAL-FIRST" / "TOKEN AUDIT"  (default — matches Busytok's
 *   local-audit positioning)
 * - `openSource`  → "OPEN SOURCE" / "TOKEN AUDIT"
 * - `generated`   → "GENERATED" / "BY BUSYTOK"
 */
export type ReceiptStampVariant = "localFirst" | "openSource" | "generated";

export interface ReceiptStampProps {
  /** Edge length of the square SVG. Default 136 (~32% of the 420px paper). */
  size?: number;
  /** Overall opacity. Keep ≤ 0.32 so the stamp never outweighs TOTAL. */
  opacity?: number;
  /** Ink color (Busytok stamp red). */
  color?: string;
  /** Rotation in degrees. Negative = counter-clockwise tilt. */
  rotate?: number;
  /** CSS class applied to the <svg> (positioning via .receipt-stamp). */
  className?: string;
  /** Outer-ring caption pair. */
  variant?: ReceiptStampVariant;
}

const VARIANT_TEXT: Record<ReceiptStampVariant, { top: string; bottom: string }> = {
  localFirst: { top: "LOCAL-FIRST", bottom: "TOKEN AUDIT" },
  openSource: { top: "OPEN SOURCE", bottom: "TOKEN AUDIT" },
  generated: { top: "GENERATED", bottom: "BY BUSYTOK" },
};

/**
 * Decorative red circular "BUSYTOK" brand stamp overlaid on the receipt's
 * items area. Implemented as inline SVG so it scales crisply and survives
 * modern-screenshot's foreignObject PNG capture.
 *
 * Capture notes:
 * - The SVG <mask> (distress spots / scratches) survives capture — it is a
 *   core SVG feature, not a CSS mask (which foreignObject drops).
 * - The feDisplacementMap "roughen" filter MAY be dropped during capture;
 *   the stamp is designed to look acceptable without it (mask alone
 *   carries the rubber-stamp distress).
 * - mix-blend-mode: multiply is applied via the .receipt-stamp CSS class,
 *   not inline — same approach as .receipt-paper::after which is known
 *   to survive capture.
 *
 * The top arc is drawn LEFT-TO-RIGHT so text reads naturally. The bottom
 * arc is drawn RIGHT-TO-LEFT so the text baseline faces outward (text
 * appears upright at the bottom of the circle). Because textPath places
 * characters along the path direction, the bottom string is reversed
 * so the visual reading order is left-to-right.
 */
function reverseForBottomArc(s: string): string {
  // textPath follows the path direction; on the right-to-left bottom arc,
  // characters would otherwise appear in reverse reading order.
  return [...s].reverse().join("");
}

export function ReceiptStamp({
  size = 136,
  opacity = 0.24,
  color = "#A94438",
  rotate = -11,
  className,
  variant = "localFirst",
}: ReceiptStampProps) {
  const id = useId();
  const text = VARIANT_TEXT[variant];
  const topArcId = `${id}-topArc`;
  const bottomArcId = `${id}-bottomArc`;
  const roughenId = `${id}-roughen`;
  const distressId = `${id}-distress`;

  return (
    <svg
      className={className}
      width={size}
      height={size}
      viewBox="0 0 200 200"
      style={{
        opacity,
        transform: `rotate(${rotate}deg)`,
      }}
      aria-hidden="true"
      focusable="false"
    >
      <defs>
        {/* Top arc: left-to-right, curving up (sweep=1 in screen coords). */}
        <path id={topArcId} d="M 42 100 A 58 58 0 0 1 158 100" fill="none" />
        {/* Bottom arc: right-to-left, curving down (sweep=1). Path direction
            is reversed so the text baseline faces outward — text appears
            upright at the bottom of the circle. The string is reversed via
            reverseForBottomArc so reading order is left-to-right. */}
        <path id={bottomArcId} d="M 158 100 A 58 58 0 0 1 42 100" fill="none" />

        {/* Light edge roughening — may be dropped in PNG capture; the mask
            alone provides sufficient distress if this filter is lost. */}
        <filter id={roughenId}>
          <feTurbulence
            type="fractalNoise"
            baseFrequency="0.85"
            numOctaves={1}
            seed="12"
            result="noise"
          />
          <feDisplacementMap
            in="SourceGraphic"
            in2="noise"
            scale="0.7"
            xChannelSelector="R"
            yChannelSelector="G"
          />
        </filter>

        {/* Distress mask: white = visible, black = holes (missing ink). */}
        <mask id={distressId}>
          <rect width="200" height="200" fill="white" />
          <g opacity={0.18}>
            <circle cx="62" cy="56" r="3" fill="black" />
            <circle cx="126" cy="70" r="2" fill="black" />
            <circle cx="92" cy="134" r="2.5" fill="black" />
            <rect x="46" y="118" width="28" height="3" rx="1.5" fill="black" />
            <rect x="112" y="92" width="22" height="2.5" rx="1.2" fill="black" />
          </g>
        </mask>
      </defs>

      {/* Stamp rings + divider line (stroke group). */}
      <g
        fill="none"
        stroke={color}
        mask={`url(#${distressId})`}
        filter={`url(#${roughenId})`}
      >
        <circle cx="100" cy="100" r="78" strokeWidth="5" />
        <circle cx="100" cy="100" r="62" strokeWidth="2" />
        <line x1="48" y1="116" x2="152" y2="116" strokeWidth="3" />
      </g>

      {/* Center wordmark + ring captions (fill group). */}
      <g
        fill={color}
        mask={`url(#${distressId})`}
        filter={`url(#${roughenId})`}
        style={{
          fontFamily: '"BusytokStamp", Oswald, sans-serif',
          textTransform: "uppercase",
        }}
      >
        <text
          x="100"
          y="103"
          textAnchor="middle"
          fontSize="34"
          fontWeight="700"
          letterSpacing="3"
        >
          BUSYTOK
        </text>
        <text fontSize="12" fontWeight="500" letterSpacing="2.2">
          <textPath href={`#${topArcId}`} startOffset="50%" textAnchor="middle">
            {text.top}
          </textPath>
        </text>
        <text fontSize="12" fontWeight="500" letterSpacing="2.2">
          <textPath href={`#${bottomArcId}`} startOffset="50%" textAnchor="middle">
            {reverseForBottomArc(text.bottom)}
          </textPath>
        </text>
      </g>
    </svg>
  );
}
