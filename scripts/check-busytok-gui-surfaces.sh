#!/usr/bin/env bash
set -euo pipefail

forbidden='ClientsPage|ActivityPage|OverviewPage|TrackToggle|LedgerTable|RecoveryPanel|useAppsQuery|useActivityQuery|useDashboardQuery|useSettingsRecovery|route-share-only|tracking\.(start|stop|status)|proxy enable|proxy status|provider binding|OAuth bridge|API key|session token'

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
