#!/usr/bin/env bash
set -euo pipefail

forbidden='ClientsPage|ActivityPage|OverviewPage|TrackToggle|LedgerTable|RecoveryPanel|useAppsQuery|useActivityQuery|useDashboardQuery|useSettingsRecovery|route-share-only|tracking\.(start|stop|status)|proxy enable|proxy status|provider binding|OAuth bridge|API key|session token'

if rg -n "$forbidden" apps/gui/src \
  --glob '!**/*.snap' \
  --glob '!**/*test*'; then
  echo "Found stale Autoken/proxy GUI surface"
  exit 1
fi

# ── Desktop host capability checks ──────────────────────────────────
CAP="apps/gui/src-tauri/capabilities/default.json"
CONF="apps/gui/src-tauri/tauri.conf.json"

grep -q '"label": "main"' "$CONF"
! grep -q 'global-shortcut:allow-register' "$CAP"
! grep -q 'core:webview:allow-create-webview-window' "$CAP"
test -f apps/gui/src-tauri/icons/menu-bar-template.png