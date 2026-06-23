#!/usr/bin/env bash
set -euo pipefail

# Stale Autoken/proxy-era surface names that must NOT reappear in the Busytok
# GUI. NOTE: current Busytok surfaces (OverviewPage, ActivityPage, LedgerTable,
# useSettingsRecoveryAction) were previously listed here and are REMOVED — they
# are live code, so forbidding them made this gate permanently red. Only
# genuinely-absent (removed) names remain.
forbidden='ClientsPage|TrackToggle|RecoveryPanel|useAppsQuery|useActivityQuery|useDashboardQuery|route-share-only|tracking\.(start|stop|status)|proxy enable|proxy status|provider binding|OAuth bridge|API key|session token'

if rg -n "$forbidden" apps/gui/src \
  --glob '!**/*.snap' \
  --glob '!**/*test*'; then
  echo "Found stale Autoken/proxy GUI surface"
  exit 1
fi

# ── Geist refactor Phase 1: radius outliers forbidden ───────────────
if rg -n -e 'border-radius:[[:space:]]*(18|20|22|24|26|32)px' apps/gui/src --glob '*.css'; then
  echo "Forbidden radius outlier (18/20/22/24/26/32) in CSS — use --radius-sm/md/lg"
  exit 1
fi

# ── Desktop host capability checks ──────────────────────────────────
CAP="apps/gui/src-tauri/capabilities/default.json"
CONF="apps/gui/src-tauri/tauri.conf.json"

grep -q '"label": "main"' "$CONF"
! grep -q 'global-shortcut:allow-register' "$CAP"
! grep -q 'core:webview:allow-create-webview-window' "$CAP"
test -f apps/gui/src-tauri/icons/menu-bar-template.png
# ── Geist refactor Phase 1: shadow-elevated is floating-only ────────
# Resting overview panels must not carry the elevated (popover/dialog)
# shadow. The pattern `selector [^{]*\{ [^}]* shadow-elevated` bounds the
# match to a single CSS rule block, correlating the selector with its own
# box-shadow. `\s*\{` anchors the selector name so `.overview-heatmap`
# does not prefix-match `.overview-heatmap__tooltip` further down.
if rg -nU -e '(\.overview-console__trend|\.live-curve-panel|\.overview-heatmap)\s*\{[^}]*--material-shadow-elevated' apps/gui/src/styles/pages.css; then
  echo "Resting overview panel uses --material-shadow-elevated (floating-only)"
  exit 1
fi

# ── Geist refactor Phase 1: stale token names forbidden (CSS + TS) ───
# Scans all of apps/gui/src, not just CSS — chartTokens.ts / nivoTheme.ts /
# LiveCurvePanel.tsx consume tokens at runtime and must migrate too.
if rg -n -e '--color-surface-strong|--color-surface-elevated|--color-canvas-subtle|--color-border-soft|--color-sidebar|--radius-xs|--radius-xl' \
  apps/gui/src --glob '!**/tokens.css' --glob '!**/tokens.test.ts'; then
  echo "Found stale/removed token name"
  exit 1
fi

# ── Geist refactor Phase 1: backdrop-filter is chrome/modal-only ─────
# Positive allowlist (spec §3): backdrop-filter may appear ONLY inside a
# rule whose selector is .desktop-sidebar / .desktop-titlebar /
# .prompt-dialog__overlay / .confirm-dialog__overlay / .prompt-overlay__backdrop.
# The awk tracks the current selector (set on `{`, reset on `}`) so each
# backdrop-filter is correlated with its own rule — a new content component
# added to components.css that sneaks in a blur is caught, not just
# surfaces.css/pages.css.
# Assumption: selector and `{` are on one line (true for current CSS style);
# if CSS later moves to multi-line selectors, extend this script to track
# the full selector across lines.
if ! awk '
  /\}/ { sel = ""; next }
  /\{/ { sel = $0; sub(/\{.*/, "", sel); next }
  /backdrop-filter/ {
    if (sel ~ /\.desktop-sidebar/ || sel ~ /\.desktop-titlebar/ || sel ~ /\.prompt-dialog__overlay/ || sel ~ /\.confirm-dialog__overlay/ || sel ~ /\.prompt-overlay__backdrop/) next
    print FILENAME ": backdrop-filter outside chrome/modal allowlist: " sel; bad = 1
  }
  END { exit bad ? 1 : 0 }
' apps/gui/src/styles/surfaces.css apps/gui/src/styles/components.css apps/gui/src/styles/pages.css; then
  echo "backdrop-filter outside chrome/modal allowlist (spec §3)"
  exit 1
fi

# ── Geist refactor Phase 1: no raw hex in CSS consumer files ─────────
# Scope: CSS consumer layer only (spec §8.3). TS chart-runtime fallback
# colors — e.g. LiveCurvePanel.tsx resolveCssColor("--color-data-live-
# primary", "#4f63f6") — are the spec §8.3 "third-party chart-lib inline
# fallback" whitelist case and are intentionally OUT of this guard's scope.
# If a CSS consumer needs a color, consume a token.
if rg -n --glob '*.css' --glob '!tokens.css' -e '#[0-9a-fA-F]{3,8}' apps/gui/src/styles; then
  echo "Raw hex in CSS consumer file — consume a token"
  exit 1
fi
