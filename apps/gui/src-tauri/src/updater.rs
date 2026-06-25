//! Tauri 2 updater — Rust-side bootstrap.
//!
//! The plugin itself is registered in `lib.rs` via
//! `.plugin(tauri_plugin_updater::Builder::new().build())`.
//!
//! Tauri 2 removed the Tauri 1 auto-poll behavior. The actual check /
//! download / install / relaunch flow is driven from the frontend: the
//! `UpdaterProvider` in `apps/gui/src/api/UpdaterProvider.tsx` polls via
//! `checkForUpdate()` in `apps/gui/src/lib/updaterClient.ts`.
//!
//! This module has three responsibilities:
//!
//! 1. Emit a startup tracing event so logs confirm the plugin loaded.
//!    The plugin's own check/download activity emits its own tracing
//!    events, captured by the subscriber initialized in `logging.rs`.
//! 2. The `install_version` command — the R1 downgrade/reinstall path.
//!    It points the updater at a chosen tag's per-tag `latest.json` and
//!    forces acceptance via a `true` version comparator, reusing the
//!    proven macOS install pipeline.
//! 3. The `list_available_versions` command — fetches the published versions
//!    manifest (versions.json) from the release CDN via native-tls (no CORS)
//!    so the version-history panel can list earlier releases for downgrade.
//!    The network call runs inside the Rust runtime; its pure parser
//!    (`parse_versions_manifest`) is unit-tested.

/// Called once from `lib.rs` Tauri setup hook. Emits a tracing::info!
/// so logs confirm the updater plugin loaded at startup.
pub(crate) fn init_updater_logging() {
    tracing::info!(
        "Tauri updater plugin loaded; checks are driven by the frontend UpdaterProvider polling"
    );
}

use tauri::{AppHandle, Runtime};
use url::Url;

/// Outcome of a user-initiated version install (downgrade or reinstall).
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum InstallVersionOutcome {
    Installed { version: String },
    Failed { message: String },
}

/// URL of the published versions manifest (versions.json), served from the
/// latest GitHub release. Fetched from Rust (not the webview) because the
/// release-asset CDN serves no `Access-Control-Allow-Origin` header — a
/// browser fetch is CORS-blocked, which surfaced in the UI as "Unavailable".
const VERSIONS_MANIFEST_URL: &str =
    "https://github.com/BalianWang/busytok/releases/latest/download/versions.json";

/// One entry in the published versions manifest. Serialized to the frontend
/// verbatim (snake_case keys match versions.json).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VersionHistoryEntryDto {
    pub version: String,
    pub date: String,
    pub notes: String,
    pub manifest_url: String,
}

/// Pure helper: parse the chosen version's manifest URL into the endpoint vec
/// the updater consumes. Unit-tested in isolation (the only pure logic here;
/// the rest of the command is Tauri-coupled and covered by the manual verify gate).
pub(crate) fn parse_manifest_endpoint(manifest_url: &str) -> Result<Vec<Url>, String> {
    let url: Url = manifest_url
        .parse()
        .map_err(|e: url::ParseError| e.to_string())?;
    Ok(vec![url])
}

/// Pure helper: parse the versions.json body into entries. Unit-tested in
/// isolation — the only pure logic in `list_available_versions` (the network
/// fetch is covered by the manual verify gate, same split as
/// `parse_manifest_endpoint`).
pub(crate) fn parse_versions_manifest(body: &str) -> Result<Vec<VersionHistoryEntryDto>, String> {
    #[derive(serde::Deserialize)]
    struct Wrapper {
        #[serde(default)]
        versions: Vec<VersionHistoryEntryDto>,
    }
    let wrapped: Wrapper =
        serde_json::from_str(body).map_err(|e| format!("invalid versions.json: {e}"))?;
    Ok(wrapped.versions)
}

/// Fetch the published versions manifest (versions.json) from the release CDN.
/// Done in Rust rather than the webview so it is not subject to browser CORS —
/// the release-asset CDN serves no `Access-Control-Allow-Origin` header, which
/// is what made the equivalent webview `fetch()` fail with "Unavailable".
#[tauri::command]
pub async fn list_available_versions() -> Result<Vec<VersionHistoryEntryDto>, String> {
    tracing::info!(
        event_code = "tauri.list_available_versions",
        url = %VERSIONS_MANIFEST_URL,
        "fetching published versions manifest"
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let body = client
        .get(VERSIONS_MANIFEST_URL)
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(
                event_code = "tauri.list_available_versions.request_failed",
                error = %e,
                "versions.json request failed"
            );
            format!("versions.json request failed: {e}")
        })?
        .error_for_status()
        .map_err(|e| {
            tracing::warn!(
                event_code = "tauri.list_available_versions.http_error",
                status = %e,
                "versions.json returned a non-2xx response"
            );
            format!("versions.json HTTP error: {e}")
        })?
        .text()
        .await
        .map_err(|e| {
            tracing::warn!(
                event_code = "tauri.list_available_versions.body_read_failed",
                error = %e,
                "versions.json body read failed"
            );
            format!("versions.json body read failed: {e}")
        })?;

    let entries = parse_versions_manifest(&body)?;
    tracing::info!(
        event_code = "tauri.list_available_versions",
        entry_count = entries.len(),
        "fetched published versions manifest"
    );
    Ok(entries)
}

/// Install a user-selected version (downgrade/reinstall) by pointing the
/// updater at that tag's per-tag latest.json and forcing acceptance via a
/// `true` version comparator (R1-verified path — reuses the proven macOS
/// install pipeline). After install, request a restart via Tauri core.
#[tauri::command]
pub async fn install_version<R: Runtime>(
    app: AppHandle<R>,
    manifest_url: String,
) -> Result<InstallVersionOutcome, String> {
    use tauri_plugin_updater::UpdaterExt;

    let endpoints = parse_manifest_endpoint(&manifest_url)?;
    let update = app
        .updater_builder()
        .endpoints(endpoints)
        .map_err(|e| e.to_string())?
        // Intentionally override any global comparator seeded by updater_builder()
        // (UpdaterState) — this is what makes an older version installable.
        .version_comparator(move |_current, _remote| true)
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no update resolved for the target version".to_string())?;

    let version = update.version.clone();
    tracing::info!(
        event_code = "tauri.install_version",
        target_version = %version,
        manifest_url = %manifest_url,
        "installing user-selected version via the updater pipeline"
    );

    match update
        .download_and_install(|_chunk_len, _content_length| {}, || {})
        .await
    {
        Ok(()) => {
            // The updater has swapped the .app in place; restart into it.
            app.request_restart();
            Ok(InstallVersionOutcome::Installed { version })
        }
        Err(e) => Ok(InstallVersionOutcome::Failed {
            message: e.to_string(),
        }),
    }
}
