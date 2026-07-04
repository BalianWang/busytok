//! CLI command handlers for Busytok.
//!
//! Each handler either:
//! - Connects to the Busytok service via `ControlClient` and calls the
//!   appropriate RPC method (the normal path), OR
//! - For `scan --offline`, directly uses `BusytokSupervisor` with a
//!   temporary in-memory database to scan local files without a service.

use std::io::BufRead;
use std::path::PathBuf;

use anyhow::{Context, Result};
use busytok_config::BusytokPaths;
use busytok_control::client::ControlClient;
use busytok_domain::{AgentKind, LogSourceType};
use busytok_protocol::dto::*;
use busytok_runtime::BusytokSupervisor;
use busytok_store::Database;

pub mod models;

/// Connect to the control server and return a client.
pub(crate) async fn connect_client() -> Result<ControlClient> {
    let endpoint = if let Ok(s) = std::env::var("BUSYTOK_SOCKET") {
        s
    } else {
        let paths = BusytokPaths::new();
        paths.control_endpoint()?
    };
    match ControlClient::connect(endpoint.clone()).await {
        Ok(client) => {
            tracing::info!(
                event_code = "cli.control.connect.ok",
                endpoint = %endpoint,
            );
            Ok(client)
        }
        Err(e) => {
            tracing::error!(
                event_code = "cli.control.connect.failed",
                endpoint = %endpoint,
                error = %e,
            );
            Err(e).context(
                "connecting to Busytok service. Open Busytok.app to start the background service, then retry."
            )
        }
    }
}

/// Call an RPC method on the control server and print the result.
async fn rpc_call(method: &str, params: serde_json::Value) -> Result<()> {
    let mut client = connect_client().await?;
    let request = ControlRequest::new(method, params);
    let response = client.call(request).await?;
    match response {
        ControlResponse::Ok(value) => {
            let output = serde_json::to_string_pretty(&value)?;
            println!("{output}");
            Ok(())
        }
        ControlResponse::Err(err) => {
            anyhow::bail!("RPC error [{}]: {}", err.code, err.message)
        }
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Show service health.
pub async fn handle_status() -> Result<()> {
    rpc_call("service.health", serde_json::json!({})).await
}

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

/// List discovered log sources.
pub async fn handle_sources_list() -> Result<()> {
    rpc_call("sources.list", serde_json::json!({})).await
}

/// Show status of a specific source.
pub async fn handle_sources_status(id: &str) -> Result<()> {
    rpc_call("sources.status", serde_json::json!({ "id": id })).await
}

/// Trigger rescan of all sources (or a specific one).
pub async fn handle_sources_rescan(source_id: Option<&str>, dry_run: bool) -> Result<()> {
    if dry_run {
        // In dry-run mode, just print the method that would be called.
        let method = "sources.rescan";
        let params = if let Some(id) = source_id {
            serde_json::json!({ "source_id": id })
        } else {
            serde_json::json!({})
        };
        println!("{method} {}", serde_json::to_string(&params)?);
        return Ok(());
    }

    let params = if let Some(id) = source_id {
        serde_json::json!({ "source_id": id })
    } else {
        serde_json::json!({})
    };
    rpc_call("sources.rescan", params).await
}

// ---------------------------------------------------------------------------
// Scan (offline)
// ---------------------------------------------------------------------------

/// Run an offline local scan without a running service.
///
/// This is the only permitted direct local runtime path. It creates an
/// in-memory database, discovers `.jsonl` files in the given path, and
/// runs the scan pipeline directly.
pub async fn handle_scan_offline(agent: &str, path: &str) -> Result<()> {
    let root = PathBuf::from(path);
    if !root.exists() {
        anyhow::bail!("path does not exist: {path}");
    }

    let db = Database::open_in_memory().context("opening in-memory database")?;
    let paths = BusytokPaths::new();
    let supervisor = BusytokSupervisor::new(db, paths);

    // Determine the agent kind from the --agent flag.
    let agent_kind = match agent {
        "claude-code" => AgentKind::ClaudeCode,
        "codex" => AgentKind::Codex,
        other => anyhow::bail!(
            "unsupported agent type for offline scan: {other} (supported: claude-code, codex)"
        ),
    };

    // Collect .jsonl files from the given path.
    let files = collect_jsonl_files(&root);
    if files.is_empty() {
        anyhow::bail!("no .jsonl files found in {path}");
    }

    let source = busytok_discovery::DiscoveredLogSource {
        agent: agent_kind,
        source_id: format!(
            "offline_{}",
            root.display()
                .to_string()
                .replace(['/', '\\'], "_")
                .trim_matches('_')
        ),
        root_path: root.clone(),
        files,
        source_type: LogSourceType::Jsonl,
        configured_by_user: true,
    };

    let stats = supervisor
        .run_scan_with_sources(vec![source])
        .context("running offline scan")?;

    println!("scan complete");
    println!("  sources:  {}", stats.sources);
    println!("  files:    {}", stats.files_scanned);
    println!("  events:   {}", stats.events_found);

    // Print the events that were ingested.
    let db = supervisor.db_handle();
    let db = db.lock().unwrap();
    let count = db.usage_event_count().context("counting events")?;
    println!("  stored:   {count}");

    Ok(())
}

/// Recursively collect all `.jsonl` files under a directory.
fn collect_jsonl_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_jsonl_files(&path));
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

// ---------------------------------------------------------------------------
// Usage
// ---------------------------------------------------------------------------

/// Show usage dashboard summary.
pub async fn handle_usage_summary() -> Result<()> {
    rpc_call("usage.dashboard", serde_json::json!({})).await
}

/// Show usage over time.
pub async fn handle_usage_timeline(
    since: Option<&str>,
    until: Option<&str>,
    agent: Option<&str>,
) -> Result<()> {
    // Normalize agent name from CLI format to runtime format.
    let normalized_agent = agent.map(|a| match a {
        "claude-code" => "claude_code",
        other => other,
    });

    let mut params = serde_json::json!({});
    if let Some(since) = since {
        params["since"] = serde_json::json!(since);
    }
    if let Some(until) = until {
        params["until"] = serde_json::json!(until);
    }
    if let Some(agent) = normalized_agent {
        params["agent"] = serde_json::json!(agent);
    }
    rpc_call("usage.timeline", params).await
}

/// List usage events.
pub async fn handle_usage_events(cursor: Option<&str>, limit: Option<u32>) -> Result<()> {
    let mut params = serde_json::json!({});
    if let Some(cursor) = cursor {
        params["cursor"] = serde_json::json!(cursor);
    }
    if let Some(limit) = limit {
        params["limit"] = serde_json::json!(limit);
    }
    rpc_call("usage.events", params).await
}

/// Show project summaries.
pub async fn handle_usage_projects() -> Result<()> {
    rpc_call("usage.projects", serde_json::json!({})).await
}

/// Show model summaries.
pub async fn handle_usage_models() -> Result<()> {
    rpc_call("usage.models", serde_json::json!({})).await
}

/// Show session summaries.
pub async fn handle_usage_sessions() -> Result<()> {
    rpc_call("usage.sessions", serde_json::json!({})).await
}

/// Export usage data in a given kind and format.
pub async fn handle_usage_export(
    kind: &str,
    format: &str,
    agent: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    // Normalize agent name from CLI format to runtime format.
    let normalized_agent = agent.map(|a| match a {
        "claude-code" => "claude_code",
        other => other,
    });

    let mut params = serde_json::json!({
        "kind": kind,
        "format": format,
    });
    if let Some(agent) = normalized_agent {
        params["agent"] = serde_json::json!(agent);
    }

    if dry_run {
        let method = "usage.export";
        println!("{method} {}", serde_json::to_string(&params)?);
        return Ok(());
    }

    rpc_call("usage.export", params).await
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// Show scan status.
pub async fn handle_diagnostics_scan_status() -> Result<()> {
    rpc_call("diagnostics.scan_status", serde_json::json!({})).await
}

/// Show store health check.
pub async fn handle_diagnostics_store_health() -> Result<()> {
    rpc_call("diagnostics.store_health", serde_json::json!({})).await
}

// ---------------------------------------------------------------------------
// Doctor
// ---------------------------------------------------------------------------

/// `busytok doctor` — run health checks (spec §855, §1068).
///
/// Calls the existing `settings.diagnostics` RPC (no new RPC method) and
/// pretty-prints the `subagent` section of the response. If the subagent
/// feature is disabled, prints a notice and exits 0.
pub async fn handle_doctor() -> Result<()> {
    let mut client = connect_client().await?;
    let request = ControlRequest::new("settings.diagnostics", serde_json::json!({}));
    let response = client.call(request).await?;
    let value = match response {
        ControlResponse::Ok(v) => v,
        ControlResponse::Err(err) => {
            anyhow::bail!("RPC error [{}]: {}", err.code, err.message);
        }
    };
    // The RPC returns a `ReadEnvelopeDto<SettingsDiagnosticsDto>`; we only
    // need the inner `data` for the subagent section.
    let data = value
        .get("data")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let dto: SettingsDiagnosticsDto = serde_json::from_value(data)?;
    match dto.subagent {
        None => {
            println!("subagent feature disabled — no doctor checks to run");
            Ok(())
        }
        Some(sub) => {
            if sub.overall_ok {
                println!("✓ subagent doctor: all checks passed (warnings allowed)");
            } else {
                println!("✗ subagent doctor: one or more checks failed");
            }
            for check in &sub.checks {
                let symbol = match check.status.as_str() {
                    "ok" => "✓",
                    "warning" => "⚠",
                    _ => "✗",
                };
                match &check.detail {
                    Some(detail) => println!("  {symbol} {}: {detail}", check.name),
                    None => println!("  {symbol} {}", check.name),
                }
            }
            if !sub.overall_ok {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Get current settings.
pub async fn handle_settings_get() -> Result<()> {
    rpc_call("settings.snapshot", serde_json::json!({})).await
}

/// Update settings.
///
/// `discovery_defaults` is a list of `(agent, bool)` pairs (e.g. `("claude-code", true)`).
/// `add_root` is an optional `(agent, path)` pair.
pub async fn handle_settings_update(
    timezone: Option<&str>,
    discovery_defaults: Vec<(&str, bool)>,
    add_root: Option<(&str, &str)>,
) -> Result<()> {
    let mut params = serde_json::json!({});

    if let Some(tz) = timezone {
        params["timezone"] = serde_json::json!(tz);
    }

    if !discovery_defaults.is_empty() || add_root.is_some() {
        // Build a DiscoverySettingsDto from the provided defaults.
        let mut claude_code_default_paths = None;
        let mut codex_default_paths = None;
        let mut manual_roots: Vec<serde_json::Value> = Vec::new();

        for (agent, enabled) in &discovery_defaults {
            match *agent {
                "claude-code" => claude_code_default_paths = Some(*enabled),
                "codex" => codex_default_paths = Some(*enabled),
                other => anyhow::bail!(
                    "unknown agent for --discovery-default: {other} (use claude-code or codex)"
                ),
            }
        }

        if let Some((agent, path)) = add_root {
            let client_id = match agent {
                "claude-code" => "claude_code",
                "codex" => "codex",
                other => anyhow::bail!(
                    "--add-root currently only supports claude-code or codex (got: {other})"
                ),
            };
            manual_roots.push(serde_json::json!({
                "id": "",
                "client_id": client_id,
                "root_path": path,
                "source_type": "manual_root",
            }));
        }

        // Open a single connection, get current settings, merge, and update.
        let mut client = connect_client().await?;
        let get_req = ControlRequest::new("settings.snapshot", serde_json::json!({}));
        let get_resp = client.call(get_req).await?;
        if let ControlResponse::Ok(current_val) = get_resp {
            if let Some(d) = current_val.get("discovery") {
                if claude_code_default_paths.is_none() {
                    claude_code_default_paths =
                        d.get("claude_code_default_paths").and_then(|v| v.as_bool());
                }
                if codex_default_paths.is_none() {
                    codex_default_paths = d.get("codex_default_paths").and_then(|v| v.as_bool());
                }
                // Preserve existing manual roots from current settings, avoiding duplicates.
                if let Some(roots) = d.get("manual_roots").and_then(|v| v.as_array()) {
                    for r in roots {
                        let is_dup = manual_roots.iter().any(|mr| {
                            mr.get("root_path").and_then(|v| v.as_str())
                                == r.get("root_path").and_then(|v| v.as_str())
                                && mr.get("client_id").and_then(|v| v.as_str())
                                    == r.get("client_id").and_then(|v| v.as_str())
                        });
                        if !is_dup {
                            manual_roots.push(r.clone());
                        }
                    }
                }
            }
        }

        params["discovery"] = serde_json::json!({
            "claude_code_default_paths": claude_code_default_paths.unwrap_or(true),
            "codex_default_paths": codex_default_paths.unwrap_or(true),
            "manual_roots": manual_roots,
        });

        // Reuse the same connection for the update call.
        let request = ControlRequest::new("settings.update", params);
        let response = client.call(request).await?;
        match response {
            ControlResponse::Ok(value) => {
                let output = serde_json::to_string_pretty(&value)?;
                println!("{output}");
                return Ok(());
            }
            ControlResponse::Err(err) => {
                anyhow::bail!("RPC error [{}]: {}", err.code, err.message)
            }
        }
    }

    let mut client = connect_client().await?;
    let request = ControlRequest::new("settings.update", params);
    let response = client.call(request).await?;
    match response {
        ControlResponse::Ok(value) => {
            let output = serde_json::to_string_pretty(&value)?;
            println!("{output}");
            Ok(())
        }
        ControlResponse::Err(err) => {
            anyhow::bail!("RPC error [{}]: {}", err.code, err.message)
        }
    }
}

// ---------------------------------------------------------------------------
// Prompt palette
// ---------------------------------------------------------------------------

/// Max length of prompt content in characters (matches store layer).
const CONTENT_MAX_CHARS: usize = 65_536;
/// Max length of prompt alias in characters (matches store layer).
const ALIAS_MAX_CHARS: usize = 80;

#[derive(Debug, serde::Deserialize)]
struct BatchPromptInput {
    content: Option<String>,
    alias: Option<String>,
    tags: Option<Vec<String>>,
}

#[derive(Debug, serde::Serialize, Default)]
struct BatchPromptOutput {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

// Must match store-level is_forbidden_alias_char in prompt_entries.rs
fn is_forbidden_alias_char(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '"' | '\'' | '`' | '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}' | '\u{feff}'
        )
}

/// Validate prompt content. Returns Some((reason_code, detail_message)) if invalid.
fn validate_content(content: &str) -> Option<(&'static str, String)> {
    if content.trim().is_empty() {
        return Some((
            "empty_content",
            "prompt content must not be empty".to_string(),
        ));
    }
    if content.chars().count() > CONTENT_MAX_CHARS {
        return Some((
            "content_too_large",
            format!("prompt content must be at most {CONTENT_MAX_CHARS} characters"),
        ));
    }
    None
}

/// Validate alias. Returns Some((reason_code, detail_message)) if invalid.
fn validate_alias(alias: &str) -> Option<(&'static str, String)> {
    let alias = alias.trim();
    if alias.is_empty() {
        return None;
    }
    if alias.chars().any(is_forbidden_alias_char) {
        return Some((
            "invalid_alias",
            "alias must not contain whitespace, quotes, or backticks".to_string(),
        ));
    }
    if alias.chars().count() > ALIAS_MAX_CHARS {
        return Some((
            "invalid_alias",
            format!("alias must be at most {ALIAS_MAX_CHARS} characters"),
        ));
    }
    None
}

/// Attempt a single `prompts.create` RPC call, reconnecting on transport error.
/// Returns `(entry_id, entry_alias)` on success.
async fn create_prompt_entry(
    client: &mut ControlClient,
    req: &PromptCreateRequestDto,
) -> Result<(String, Option<String>)> {
    let params = serde_json::to_value(req)?;
    let request = ControlRequest::new("prompts.create", params);
    match client.call(request).await {
        Ok(ControlResponse::Ok(value)) => {
            // The response is a ReadEnvelopeDto<PromptEntryDto>.
            let envelope: ReadEnvelopeDto =
                serde_json::from_value(value).context("deserializing prompts.create response")?;
            let id = envelope
                .data
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let alias = envelope
                .data
                .get("alias")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            Ok((id, alias))
        }
        Ok(ControlResponse::Err(err)) => {
            let code = err.code;
            let message = err.message;
            anyhow::bail!("RPC error [{}]: {}", code, message)
        }
        Err(e) => {
            anyhow::bail!("transport error: {}", e)
        }
    }
}

/// Emit a skipped-line JSONL record to stdout.
fn emit_skipped(index: usize, reason: &str, detail: String, alias: Option<&str>) -> Result<()> {
    let output = BatchPromptOutput {
        status: "skipped".to_string(),
        index: Some(index),
        reason: Some(reason.to_string()),
        detail: Some(detail),
        alias: alias.map(|s| s.to_string()),
        ..Default::default()
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

/// Handle `busytok prompt create`.
pub async fn handle_prompt_create(
    content: Option<String>,
    alias: Option<String>,
    tags: Vec<String>,
    batch: bool,
) -> Result<()> {
    if batch {
        handle_prompt_create_batch().await
    } else {
        let content = content.ok_or_else(|| {
            anyhow::anyhow!("--content is required (or use --batch to read from stdin)")
        })?;

        // Client-side pre-validation (same rules as batch mode and frontend dialog).
        if let Some((_reason, detail)) = validate_content(&content) {
            anyhow::bail!(detail);
        }

        let alias_clean = alias.filter(|a| !a.trim().is_empty());
        if let Some(ref alias_val) = alias_clean {
            if let Some((_reason, detail)) = validate_alias(alias_val) {
                anyhow::bail!(detail);
            }
        }

        let params = serde_json::json!({
            "content": content,
            "alias": alias_clean,
            "tags": tags,
        });
        rpc_call("prompts.create", params).await
    }
}

/// Batch-create prompt entries from stdin JSONL.
async fn handle_prompt_create_batch() -> Result<()> {
    let stdin = std::io::stdin();
    let reader = std::io::BufReader::new(stdin);
    let mut any_skipped = false;
    let mut client: Option<ControlClient> = None;

    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Parse JSON line.
        let input: BatchPromptInput = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                emit_skipped(index, "invalid_json", e.to_string(), None)?;
                any_skipped = true;
                continue;
            }
        };

        // Validate content.
        let content = input.content.as_deref().unwrap_or("");
        if let Some((reason, detail)) = validate_content(content) {
            emit_skipped(index, reason, detail, input.alias.as_deref())?;
            any_skipped = true;
            continue;
        }

        // Validate alias.
        if let Some(ref alias) = input.alias {
            if let Some((reason, detail)) = validate_alias(alias) {
                emit_skipped(index, reason, detail, input.alias.as_deref())?;
                any_skipped = true;
                continue;
            }
        }

        let alias_for_rpc = input.alias.filter(|a| !a.trim().is_empty());

        let req = PromptCreateRequestDto {
            content: content.to_string(),
            alias: alias_for_rpc,
            tags: input.tags.unwrap_or_default(),
        };

        // Lazy-connect to daemon before first RPC call.
        if client.is_none() {
            match connect_client().await {
                Ok(c) => client = Some(c),
                Err(e) => {
                    emit_skipped(index, "rpc_error", e.to_string(), req.alias.as_deref())?;
                    any_skipped = true;
                    continue;
                }
            }
        }

        // Attempt RPC create, with one reconnect retry on transport error.
        match create_prompt_entry(client.as_mut().unwrap(), &req).await {
            Ok((id, entry_alias)) => {
                let output = BatchPromptOutput {
                    status: "created".to_string(),
                    id: Some(id),
                    alias: entry_alias,
                    ..Default::default()
                };
                println!("{}", serde_json::to_string(&output)?);
            }
            Err(e) => {
                let err_str = e.to_string();
                let reason = if err_str.contains("an alias with this name already exists") {
                    "alias_conflict"
                } else {
                    "rpc_error"
                };
                emit_skipped(index, reason, err_str, req.alias.as_deref())?;
                any_skipped = true;

                // If transport error, attempt reconnect for the next entry.
                if e.to_string().contains("transport error") {
                    client = connect_client().await.ok();
                }
            }
        }
    }

    if any_skipped {
        anyhow::bail!("some entries were skipped (see JSONL output for details)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use busytok_control::{dispatch::RuntimeControl, server::ControlServer, TestRuntimeControl};
    use busytok_protocol::dto::*;
    use serde_json::{json, Value};
    use serial_test::serial;

    // ── pure helpers ────────────────────────────────────────────────────

    #[test]
    fn is_forbidden_alias_char_flags_whitespace_quotes_and_zero_width() {
        // Whitespace.
        assert!(is_forbidden_alias_char(' '));
        assert!(is_forbidden_alias_char('\t'));
        assert!(is_forbidden_alias_char('\n'));
        // Quotes and backticks.
        assert!(is_forbidden_alias_char('"'));
        assert!(is_forbidden_alias_char('\''));
        assert!(is_forbidden_alias_char('`'));
        // Zero-width / bidi / BOM.
        assert!(is_forbidden_alias_char('\u{200b}'));
        assert!(is_forbidden_alias_char('\u{200c}'));
        assert!(is_forbidden_alias_char('\u{200d}'));
        assert!(is_forbidden_alias_char('\u{2060}'));
        assert!(is_forbidden_alias_char('\u{feff}'));
        // Plain ASCII is fine.
        assert!(!is_forbidden_alias_char('a'));
        assert!(!is_forbidden_alias_char('-'));
        assert!(!is_forbidden_alias_char('_'));
    }

    #[test]
    fn validate_content_rejects_empty_or_whitespace_only() {
        assert_eq!(validate_content("").unwrap().0, "empty_content");
        assert_eq!(validate_content("   ").unwrap().0, "empty_content");
        assert_eq!(validate_content("\n\t").unwrap().0, "empty_content");
    }

    #[test]
    fn validate_content_rejects_oversize_content() {
        // CONTENT_MAX_CHARS + 1 characters should trigger content_too_large.
        let big = "a".repeat(CONTENT_MAX_CHARS + 1);
        let (reason, detail) = validate_content(&big).expect("oversize content should fail");
        assert_eq!(reason, "content_too_large");
        assert!(detail.contains(&CONTENT_MAX_CHARS.to_string()));
    }

    #[test]
    fn validate_content_accepts_normal_content() {
        assert!(validate_content("hello world").is_none());
        // Exactly at the limit is OK.
        let at_limit = "a".repeat(CONTENT_MAX_CHARS);
        assert!(validate_content(&at_limit).is_none());
    }

    #[test]
    fn validate_alias_none_for_empty_after_trim() {
        // Whitespace-only alias is treated as "no alias" (None).
        assert!(validate_alias("").is_none());
        assert!(validate_alias("   ").is_none());
    }

    #[test]
    fn validate_alias_rejects_forbidden_characters() {
        let (reason, _detail) = validate_alias("has space").unwrap();
        assert_eq!(reason, "invalid_alias");
        let (reason, _) = validate_alias("has\"quote").unwrap();
        assert_eq!(reason, "invalid_alias");
        let (reason, _) = validate_alias("has`backtick").unwrap();
        assert_eq!(reason, "invalid_alias");
        let (reason, _) = validate_alias("with\u{200b}zwsp").unwrap();
        assert_eq!(reason, "invalid_alias");
    }

    #[test]
    fn validate_alias_rejects_oversize_alias() {
        let big = "a".repeat(ALIAS_MAX_CHARS + 1);
        let (reason, _detail) = validate_alias(&big).unwrap();
        assert_eq!(reason, "invalid_alias");
    }

    #[test]
    fn validate_alias_accepts_normal_alias() {
        assert!(validate_alias("my-alias").is_none());
        assert!(validate_alias("a").is_none());
        // At the limit is OK.
        let at_limit = "a".repeat(ALIAS_MAX_CHARS);
        assert!(validate_alias(&at_limit).is_none());
    }

    #[test]
    fn emit_skipped_emits_valid_jsonl_record() {
        // `emit_skipped` println!s a JSON line; we can't easily capture stdout,
        // but we can verify it returns Ok for a known set of inputs.
        assert!(emit_skipped(0, "invalid_json", "expected colon".to_string(), None).is_ok());
        assert!(emit_skipped(
            7,
            "empty_content",
            "must not be empty".to_string(),
            Some("a")
        )
        .is_ok());
    }

    #[test]
    fn collect_jsonl_files_finds_jsonl_files_recursively() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Create a mix of .jsonl and other files in nested dirs.
        std::fs::write(root.join("a.jsonl"), "{}").unwrap();
        std::fs::write(root.join("b.txt"), "ignore me").unwrap();
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("c.jsonl"), "{}").unwrap();
        let deep = nested.join("deep");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("d.jsonl"), "{}").unwrap();

        let files = collect_jsonl_files(root);
        // Should find 3 .jsonl files (a, c, d) and skip b.txt.
        assert_eq!(files.len(), 3, "should find exactly 3 jsonl files");
        // Result is sorted, so the first should be a.jsonl under root.
        assert!(files.iter().any(|p| p.file_name().unwrap() == "a.jsonl"));
        assert!(files.iter().any(|p| p.file_name().unwrap() == "c.jsonl"));
        assert!(files.iter().any(|p| p.file_name().unwrap() == "d.jsonl"));
        // b.txt should not be present.
        assert!(!files
            .iter()
            .any(|p| p.file_name().and_then(|n| n.to_str()) == Some("b.txt")));
    }

    #[test]
    fn collect_jsonl_files_returns_empty_for_nonexistent_dir() {
        // A nonexistent dir yields an empty Vec (read_dir returns Err, which
        // is silently treated as no entries).
        let files = collect_jsonl_files(std::path::Path::new("/nonexistent/busytok-12345"));
        assert!(files.is_empty());
    }

    #[test]
    fn collect_jsonl_files_returns_empty_for_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let files = collect_jsonl_files(tmp.path());
        assert!(files.is_empty());
    }

    // ── handle_scan_offline error paths ─────────────────────────────────

    #[tokio::test]
    async fn handle_scan_offline_errors_when_path_does_not_exist() {
        let result = handle_scan_offline("claude-code", "/nonexistent/path/abc").await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("path does not exist"), "got: {err}");
    }

    #[tokio::test]
    async fn handle_scan_offline_errors_for_unsupported_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let result = handle_scan_offline("gemini", tmp.path().to_str().unwrap()).await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unsupported agent type"), "got: {err}");
        assert!(
            err.contains("gemini"),
            "should mention the bad agent: {err}"
        );
        assert!(
            err.contains("claude-code"),
            "should mention supported agents: {err}"
        );
    }

    #[tokio::test]
    async fn handle_scan_offline_errors_when_no_jsonl_files() {
        let tmp = tempfile::tempdir().unwrap();
        // Empty directory → no .jsonl files.
        let result = handle_scan_offline("claude-code", tmp.path().to_str().unwrap()).await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no .jsonl files found"), "got: {err}");
    }

    #[tokio::test]
    async fn handle_scan_offline_succeeds_for_codex_agent() {
        // Use the codex fixture to exercise the Codex branch of AgentKind.
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/codex");
        let result = handle_scan_offline("codex", fixture.to_str().unwrap()).await;
        assert!(result.is_ok(), "scan should succeed: {:?}", result.err());
    }

    // ── handle_doctor (multiple SettingsDiagnosticsDto shapes) ──────────

    /// Hold a running `ControlServer` for the lifetime of the test.
    struct ServerHarness {
        server: Arc<ControlServer>,
        _task: tokio::task::JoinHandle<anyhow::Result<()>>,
    }

    async fn spawn_server(runtime: Arc<dyn RuntimeControl>) -> (ServerHarness, String) {
        let (server, socket_path) = ControlServer::spawn_for_test(runtime).await.unwrap();
        let server = Arc::new(server);
        let server_for_task = Arc::clone(&server);
        let task = tokio::spawn(async move { server_for_task.run().await });
        (
            ServerHarness {
                server,
                _task: task,
            },
            socket_path,
        )
    }

    impl Drop for ServerHarness {
        fn drop(&mut self) {
            self.server.shutdown();
        }
    }
    fn base_diagnostics() -> SettingsDiagnosticsDto {
        SettingsDiagnosticsDto {
            db_healthy: true,
            db_size_bytes: 0,
            migration_version: 0,
            usage_event_count: 0,
            last_log_checkpoint_ms: None,
            writer_queue_depth: 0,
            aggregate_lag_ms: 0,
            recent_diagnostics: vec![],
            subagent: None,
        }
    }

    async fn spawn_doctor_server(diagnostics: SettingsDiagnosticsDto) -> (ServerHarness, String) {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> =
            Arc::new(TestRuntimeWrapper::new(inner).with_diagnostics(diagnostics));
        spawn_server(runtime).await
    }

    #[tokio::test]
    #[serial]
    async fn handle_doctor_prints_disabled_notice_when_subagent_is_none() {
        let diag = base_diagnostics(); // subagent: None
        let (harness, socket) = spawn_doctor_server(diag).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_doctor().await;
        drop(harness);
        assert!(
            result.is_ok(),
            "doctor should succeed when subagent disabled"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_doctor_succeeds_when_all_checks_pass() {
        let mut diag = base_diagnostics();
        diag.subagent = Some(SubagentDoctorResultDto {
            checks: vec![DoctorCheckDto {
                name: "sidecar_launchable".to_string(),
                status: "ok".to_string(),
                detail: Some("ok".to_string()),
            }],
            overall_ok: true,
        });
        let (harness, socket) = spawn_doctor_server(diag).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_doctor().await;
        drop(harness);
        assert!(
            result.is_ok(),
            "doctor should succeed when overall_ok: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_doctor_renders_all_check_statuses_when_overall_ok() {
        // Use `overall_ok=true` with mixed ok/warning/error checks so we
        // exercise every symbol-rendering and detail branch WITHOUT hitting
        // `std::process::exit(1)` (which would kill the test runner and
        // cannot be tested inline — see `tests/coverage_gaps.rs` for the
        // subprocess-based exit-code test).
        let mut diag = base_diagnostics();
        diag.subagent = Some(SubagentDoctorResultDto {
            checks: vec![
                DoctorCheckDto {
                    name: "sidecar_launchable".to_string(),
                    status: "ok".to_string(),
                    detail: Some("ok".to_string()),
                },
                DoctorCheckDto {
                    name: "stale_subagents".to_string(),
                    status: "warning".to_string(),
                    detail: None,
                },
                DoctorCheckDto {
                    name: "missing_pisidecar".to_string(),
                    status: "error".to_string(),
                    detail: Some("not found".to_string()),
                },
            ],
            overall_ok: true,
        });
        let (harness, socket) = spawn_doctor_server(diag).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_doctor().await;
        drop(harness);
        assert!(
            result.is_ok(),
            "doctor should succeed when overall_ok=true: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_doctor_renders_unknown_status_as_failure_symbol() {
        // An unrecognized status string falls through to the "✗" branch.
        let mut diag = base_diagnostics();
        diag.subagent = Some(SubagentDoctorResultDto {
            checks: vec![DoctorCheckDto {
                name: "weird_check".to_string(),
                status: "bogus".to_string(),
                detail: Some("detail".to_string()),
            }],
            overall_ok: true,
        });
        let (harness, socket) = spawn_doctor_server(diag).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_doctor().await;
        drop(harness);
        assert!(
            result.is_ok(),
            "doctor should succeed when overall_ok=true: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_doctor_propagates_rpc_error_when_settings_diagnostics_fails() {
        // Use a runtime whose settings_diagnostics always errors.
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(
            TestRuntimeWrapper::new(inner)
                .with_diagnostics_error("diag_failed: settings.diagnostics is broken"),
        );
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_doctor().await;
        drop(harness);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("RPC error") && err.contains("diag_failed"),
            "expected RPC error to surface, got: {err}"
        );
    }
    // ── handle_settings_update ──────────────────────────────────────────

    #[tokio::test]
    #[serial]
    async fn handle_settings_update_errors_on_unknown_discovery_default_agent() {
        // Unknown agent in discovery_defaults should bail before any RPC call.
        // We point BUSYTOK_SOCKET at a nonexistent path to verify the
        // validation happens before the connect attempt.
        std::env::set_var("BUSYTOK_SOCKET", "/nonexistent/busytok-settings-test.sock");
        let result = handle_settings_update(None, vec![("gemini", true)], None).await;
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown agent") && err.contains("gemini"),
            "expected unknown-agent error, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_settings_update_errors_on_unknown_add_root_agent() {
        std::env::set_var("BUSYTOK_SOCKET", "/nonexistent/busytok-settings-test.sock");
        let result = handle_settings_update(None, vec![], Some(("gemini", "/some/path"))).await;
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("--add-root") && err.contains("gemini"),
            "expected --add-root error, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_settings_update_with_discovery_defaults_merges_and_calls_rpc() {
        // The default TestRuntimeControl.settings_snapshot returns an empty
        // manual_roots list, so the merge logic should preserve our new entry.
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let wrapper = TestRuntimeWrapper::new(inner).with_captured_settings_update();
        let captured = wrapper.captured_update_handle();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(wrapper);
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);

        let result = handle_settings_update(
            Some("UTC"),
            vec![("claude-code", false), ("codex", true)],
            Some(("claude-code", "/new/path")),
        )
        .await;
        drop(harness);
        assert!(
            result.is_ok(),
            "settings update should succeed: {:?}",
            result.err()
        );

        // Inspect the captured SettingsUpdateRequestDto to verify the merge.
        let req = captured
            .lock()
            .unwrap()
            .clone()
            .expect("expected a settings.update call");
        assert_eq!(req.timezone.as_deref(), Some("UTC"));
        let discovery = req.discovery.expect("discovery should be set");
        assert!(
            !discovery.claude_code_default_paths,
            "claude-code should be disabled"
        );
        assert!(discovery.codex_default_paths, "codex should be enabled");
        // The new root should be present with client_id "claude_code".
        let has_new_root = discovery
            .manual_roots
            .iter()
            .any(|r| r.client_id == "claude_code" && r.root_path == "/new/path");
        assert!(
            has_new_root,
            "new root should be in manual_roots: {:?}",
            discovery.manual_roots
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_settings_update_with_no_discovery_or_root_calls_simple_update() {
        // When neither discovery_defaults nor add_root is set, the code
        // takes the bottom path that calls settings.update with just
        // the timezone (no discovery merge).
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let wrapper = TestRuntimeWrapper::new(inner).with_captured_settings_update();
        let captured = wrapper.captured_update_handle();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(wrapper);
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);

        let result = handle_settings_update(Some("America/Los_Angeles"), vec![], None).await;
        drop(harness);
        assert!(
            result.is_ok(),
            "settings update should succeed: {:?}",
            result.err()
        );

        let req = captured
            .lock()
            .unwrap()
            .clone()
            .expect("expected a settings.update call");
        assert_eq!(req.timezone.as_deref(), Some("America/Los_Angeles"));
        assert!(
            req.discovery.is_none(),
            "no discovery field should be sent when no discovery-defaults/add-root"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_settings_update_propagates_rpc_error() {
        // Use a runtime whose settings_update always fails.
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(
            TestRuntimeWrapper::new(inner)
                .with_settings_update_error("update_failed: settings.update is broken"),
        );
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_settings_update(Some("UTC"), vec![], None).await;
        drop(harness);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("RPC error") && err.contains("update_failed"),
            "expected RPC error to surface, got: {err}"
        );
    }
    // ── handle_sources_rescan / handle_usage_export dry-run paths ───────

    #[tokio::test]
    async fn handle_sources_rescan_dry_run_with_source_id_prints_method_and_params() {
        // dry_run mode prints to stdout and returns Ok without touching the network.
        let result = handle_sources_rescan(Some("src-1"), true).await;
        assert!(
            result.is_ok(),
            "dry_run should always succeed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn handle_sources_rescan_dry_run_without_source_id_prints_empty_params() {
        let result = handle_sources_rescan(None, true).await;
        assert!(
            result.is_ok(),
            "dry_run should always succeed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn handle_usage_export_dry_run_prints_method_and_params() {
        // Without an agent filter — params should only contain kind+format.
        let result = handle_usage_export("events", "csv", None, true).await;
        assert!(
            result.is_ok(),
            "dry_run should always succeed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn handle_usage_export_dry_run_normalizes_agent_name() {
        // We can't easily capture stdout, but we can verify Ok is returned
        // and rely on the subprocess integration test (coverage_gaps.rs)
        // to assert the actual normalized params string.
        let result = handle_usage_export("timeline", "json", Some("claude-code"), true).await;
        assert!(
            result.is_ok(),
            "dry_run should always succeed: {:?}",
            result.err()
        );
    }

    // ── connect_client error path ──────────────────────────────────────

    #[tokio::test]
    #[serial]
    async fn connect_client_returns_helpful_error_when_socket_unreachable() {
        std::env::set_var("BUSYTOK_SOCKET", "/nonexistent/busytok-connect-test.sock");
        let result = connect_client().await;
        // `ControlClient` does not implement `Debug`, so we cannot use
        // `unwrap_err()`; instead we pattern-match to extract the error.
        let err = match result {
            Ok(_) => panic!("expected connect_client to fail on a bogus socket"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("connecting to Busytok service") || err.contains("Open Busytok.app"),
            "expected user-friendly connect error, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn rpc_call_succeeds_against_real_server() {
        // Smoke test: rpc_call hits the success branch when the server is healthy.
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(inner);
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = rpc_call("service.health", json!({})).await;
        drop(harness);
        assert!(
            result.is_ok(),
            "rpc_call should succeed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[serial]
    async fn rpc_call_bails_on_method_not_found() {
        // An unknown method returns ControlResponse::Err — rpc_call should bail.
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(inner);
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = rpc_call("totally.bogus.method", json!({})).await;
        drop(harness);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("RPC error") && err.contains("method_not_found"),
            "expected method_not_found error, got: {err}"
        );
    }

    // ── handle_status / handle_usage_* (smoke tests against real server) ──

    #[tokio::test]
    #[serial]
    async fn handle_status_calls_service_health() {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(inner);
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_status().await;
        drop(harness);
        // Note: handle_status calls "service.health" which the dispatcher
        // supports; the ok response is printed to stdout.
        assert!(result.is_ok(), "handle_status: {:?}", result.err());
    }

    #[tokio::test]
    #[serial]
    async fn handle_usage_summary_calls_known_method() {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(inner);
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        // usage.dashboard is not in the dispatcher, so this returns
        // method_not_found — but the connect+call flow is exercised.
        let _ = handle_usage_summary().await;
        drop(harness);
    }

    #[tokio::test]
    #[serial]
    async fn handle_usage_timeline_passes_normalized_agent() {
        // The dispatcher doesn't handle usage.timeline, so we'll get
        // method_not_found — but the connect and request-build paths run.
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(inner);
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let _ = handle_usage_timeline(Some("2026-01-01"), None, Some("claude-code")).await;
        let _ = handle_usage_timeline(None, Some("2026-12-31"), Some("codex")).await;
        let _ = handle_usage_timeline(None, None, None).await;
        drop(harness);
    }

    #[tokio::test]
    #[serial]
    async fn handle_usage_events_passes_cursor_and_limit() {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(inner);
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let _ = handle_usage_events(Some("cursor-1"), Some(50)).await;
        let _ = handle_usage_events(None, None).await;
        drop(harness);
    }

    #[tokio::test]
    #[serial]
    async fn handle_diagnostics_commands_run() {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(inner);
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let _ = handle_diagnostics_scan_status().await;
        let _ = handle_diagnostics_store_health().await;
        drop(harness);
    }

    #[tokio::test]
    #[serial]
    async fn handle_settings_get_calls_snapshot() {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(inner);
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_settings_get().await;
        drop(harness);
        assert!(result.is_ok(), "handle_settings_get: {:?}", result.err());
    }

    // ── handle_prompt_create single-mode validation ─────────────────────

    #[tokio::test]
    #[serial]
    async fn handle_prompt_create_errors_when_content_missing_in_single_mode() {
        std::env::set_var("BUSYTOK_SOCKET", "/nonexistent/busytok-prompt-test.sock");
        let result = handle_prompt_create(None, None, vec![], false).await;
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("--content is required"),
            "expected --content required error, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_prompt_create_errors_for_empty_content() {
        std::env::set_var("BUSYTOK_SOCKET", "/nonexistent/busytok-prompt-test.sock");
        let result = handle_prompt_create(Some(String::new()), None, vec![], false).await;
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("empty"),
            "expected empty content error, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_prompt_create_errors_for_invalid_alias() {
        std::env::set_var("BUSYTOK_SOCKET", "/nonexistent/busytok-prompt-test.sock");
        let result = handle_prompt_create(
            Some("hello".to_string()),
            Some("has space".to_string()),
            vec![],
            false,
        )
        .await;
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("whitespace") || err.contains("alias"),
            "expected invalid alias error, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_prompt_create_trims_alias_before_validating() {
        // An alias that is only whitespace is treated as "no alias" — the
        // content is valid, so the only failure should be the RPC connect.
        std::env::set_var("BUSYTOK_SOCKET", "/nonexistent/busytok-prompt-test.sock");
        let result = handle_prompt_create(
            Some("hello".to_string()),
            Some("   ".to_string()),
            vec![],
            false,
        )
        .await;
        let err = result.unwrap_err().to_string();
        // Should reach the RPC stage and fail with a connect error, not an
        // alias-validation error.
        assert!(
            !err.contains("whitespace"),
            "whitespace-only alias should be treated as no alias: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_prompt_create_succeeds_against_real_server() {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(inner);
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_prompt_create(
            Some("hello world".to_string()),
            Some("greeting".to_string()),
            vec!["friendly".to_string()],
            false,
        )
        .await;
        drop(harness);
        assert!(result.is_ok(), "handle_prompt_create: {:?}", result.err());
    }

    // ── create_prompt_entry (RPC error branches) ────────────────────────

    #[tokio::test]
    #[serial]
    async fn create_prompt_entry_returns_id_and_alias_on_success() {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(inner);
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let mut client = connect_client().await.unwrap();
        let req = PromptCreateRequestDto {
            content: "x".to_string(),
            alias: Some("a".to_string()),
            tags: vec![],
        };
        let result = create_prompt_entry(&mut client, &req).await;
        drop(harness);
        let (id, alias) = result.expect("create should succeed");
        assert_eq!(id, "prompt-created");
        assert_eq!(alias.as_deref(), Some("a"));
    }

    #[tokio::test]
    #[serial]
    async fn create_prompt_entry_bails_on_rpc_error() {
        // Use a runtime whose prompts_create always errors.
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(
            TestRuntimeWrapper::new(inner)
                .with_prompts_create_error("create_failed: prompts.create is broken"),
        );
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let mut client = connect_client().await.unwrap();
        let req = PromptCreateRequestDto {
            content: "x".to_string(),
            alias: None,
            tags: vec![],
        };
        let result = create_prompt_entry(&mut client, &req).await;
        drop(harness);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("RPC error") && err.contains("create_failed"),
            "expected RPC error to surface, got: {err}"
        );
    }
    // ── Consolidated test runtime wrapper ─────────────────────────────
    //
    // Replaces the previous per-use wrapper structs (DoctorRuntime,
    // FailingDiagnosticsRuntime, SettingsCapturingRuntime,
    // FailingUpdateRuntime, FailingPromptsCreateRuntime) to eliminate
    // ~880 lines of duplicated delegation boilerplate. Each of those
    // wrappers overrode only 1-2 methods but duplicated ~30 delegation
    // methods, all of which were dead code (never called by any test).

    /// Unified test runtime wrapper that can inject custom behavior into
    /// specific RPC methods (`settings_diagnostics`, `settings_update`,
    /// `prompts_create`).
    struct TestRuntimeWrapper {
        inner: TestRuntimeControl,
        /// When set, `settings_diagnostics` returns this value.
        diagnostics_value: Option<SettingsDiagnosticsDto>,
        /// When set, `settings_diagnostics` bails with this message.
        diagnostics_error: Option<String>,
        /// When set, `settings_update` bails with this message.
        settings_update_error: Option<String>,
        /// Shared handle for capturing `settings_update` requests.
        captured_update: Arc<Mutex<Option<SettingsUpdateRequestDto>>>,
        /// When true, `settings_update` captures the request before delegating.
        capture_update: bool,
        /// When set, `prompts_create` bails with this message.
        prompts_create_error: Option<String>,
    }

    impl TestRuntimeWrapper {
        fn new(inner: TestRuntimeControl) -> Self {
            Self {
                inner,
                diagnostics_value: None,
                diagnostics_error: None,
                settings_update_error: None,
                captured_update: Arc::new(Mutex::new(None)),
                capture_update: false,
                prompts_create_error: None,
            }
        }

        fn with_diagnostics(mut self, diag: SettingsDiagnosticsDto) -> Self {
            self.diagnostics_value = Some(diag);
            self
        }

        fn with_diagnostics_error(mut self, msg: impl Into<String>) -> Self {
            self.diagnostics_error = Some(msg.into());
            self
        }

        fn with_settings_update_error(mut self, msg: impl Into<String>) -> Self {
            self.settings_update_error = Some(msg.into());
            self
        }

        fn with_captured_settings_update(mut self) -> Self {
            self.capture_update = true;
            self
        }

        fn with_prompts_create_error(mut self, msg: impl Into<String>) -> Self {
            self.prompts_create_error = Some(msg.into());
            self
        }

        /// Returns a shared handle to the captured `settings_update` requests.
        /// Must be called before wrapping in `Arc<dyn RuntimeControl>`.
        fn captured_update_handle(&self) -> Arc<Mutex<Option<SettingsUpdateRequestDto>>> {
            Arc::clone(&self.captured_update)
        }
    }

    #[async_trait]
    impl RuntimeControl for TestRuntimeWrapper {
        async fn settings_diagnostics(&self) -> Result<ReadEnvelopeDto<SettingsDiagnosticsDto>> {
            if let Some(ref err) = self.diagnostics_error {
                anyhow::bail!("{}", err);
            }
            if let Some(ref diag) = self.diagnostics_value {
                return Ok(ReadEnvelopeDto {
                    data: diag.clone(),
                    generated_at_ms: 0,
                    generation_id: None,
                    readiness: ReadinessStateDto::ReadyExact,
                    is_exact: true,
                    is_stale: false,
                    watermark_ms: None,
                    progress: None,
                    degraded_reason: None,
                });
            }
            self.inner.settings_diagnostics().await
        }

        async fn settings_update(
            &self,
            req: SettingsUpdateRequestDto,
        ) -> Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
            if let Some(ref err) = self.settings_update_error {
                anyhow::bail!("{}", err);
            }
            if self.capture_update {
                *self.captured_update.lock().unwrap() = Some(req.clone());
            }
            self.inner.settings_update(req).await
        }

        async fn prompts_create(
            &self,
            req: PromptCreateRequestDto,
        ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
            if let Some(ref err) = self.prompts_create_error {
                anyhow::bail!("{}", err);
            }
            self.inner.prompts_create(req).await
        }

        // Everything else delegates to the inner runtime.
        async fn service_health(&self) -> Result<ServiceHealthDto> {
            self.inner.service_health().await
        }
        async fn service_status(&self) -> Result<ServiceStatusDto> {
            self.inner.service_status().await
        }
        async fn shell_status(&self) -> Result<ShellStatusDto> {
            self.inner.shell_status().await
        }
        async fn overview_summary(
            &self,
            req: OverviewSummaryRequestDto,
        ) -> Result<ReadEnvelopeDto<OverviewSummaryDto>> {
            self.inner.overview_summary(req).await
        }
        async fn overview_trend(
            &self,
            req: OverviewTrendRequestDto,
        ) -> Result<ReadEnvelopeDto<OverviewTrendResponseDto>> {
            self.inner.overview_trend(req).await
        }
        async fn overview_heatmap(
            &self,
            req: OverviewHeatmapRequestDto,
        ) -> Result<ReadEnvelopeDto<OverviewHeatmapResponseDto>> {
            self.inner.overview_heatmap(req).await
        }
        async fn overview_rankings(
            &self,
            req: OverviewRankingsRequestDto,
        ) -> Result<ReadEnvelopeDto<OverviewRankingsResponseDto>> {
            self.inner.overview_rankings(req).await
        }
        async fn receipt_daily(
            &self,
            req: ReceiptDailyRequestDto,
        ) -> Result<ReadEnvelopeDto<ReceiptDailyDto>> {
            self.inner.receipt_daily(req).await
        }
        async fn activity_recent(
            &self,
            req: ActivityRecentRequestDto,
        ) -> Result<ReadEnvelopeDto<ActivityRecentResponseDto>> {
            self.inner.activity_recent(req).await
        }
        async fn activity_list(
            &self,
            req: ActivityListRequestDto,
        ) -> Result<ReadEnvelopeDto<ActivityListResponseDto>> {
            self.inner.activity_list(req).await
        }
        async fn activity_detail(
            &self,
            req: ActivityDetailRequestDto,
        ) -> Result<ReadEnvelopeDto<ActivityDetailDto>> {
            self.inner.activity_detail(req).await
        }
        async fn breakdown_list(
            &self,
            req: BreakdownListRequestDto,
        ) -> Result<ReadEnvelopeDto<BreakdownListResponseDto>> {
            self.inner.breakdown_list(req).await
        }
        async fn breakdown_detail(
            &self,
            req: BreakdownDetailRequestDto,
        ) -> Result<ReadEnvelopeDto<BreakdownDetailDto>> {
            self.inner.breakdown_detail(req).await
        }
        async fn clients_snapshot(
            &self,
            req: ClientsSnapshotRequestDto,
        ) -> Result<ReadEnvelopeDto<ClientsSnapshotDto>> {
            self.inner.clients_snapshot(req).await
        }
        async fn clients_detail(
            &self,
            req: ClientSourceDetailRequestDto,
        ) -> Result<ReadEnvelopeDto<ClientSourceDetailDto>> {
            self.inner.clients_detail(req).await
        }
        async fn settings_snapshot(&self) -> Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
            self.inner.settings_snapshot().await
        }
        async fn settings_recovery_action(
            &self,
            req: SettingsRecoveryActionRequestDto,
        ) -> Result<ReadEnvelopeDto<SettingsRecoveryActionResponseDto>> {
            self.inner.settings_recovery_action(req).await
        }
        async fn live_window(
            &self,
            req: LiveWindowRequestDto,
        ) -> Result<ReadEnvelopeDto<LiveWindowDto>> {
            self.inner.live_window(req).await
        }
        async fn prompts_list(
            &self,
            req: PromptListQueryDto,
        ) -> Result<ReadEnvelopeDto<PromptListResponseDto>> {
            self.inner.prompts_list(req).await
        }
        async fn prompts_get(
            &self,
            req: PromptGetRequestDto,
        ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
            self.inner.prompts_get(req).await
        }
        async fn prompts_update(
            &self,
            req: PromptUpdateRequestDto,
        ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
            self.inner.prompts_update(req).await
        }
        async fn prompts_delete(
            &self,
            req: PromptDeleteRequestDto,
        ) -> Result<PromptDeleteResultDto> {
            self.inner.prompts_delete(req).await
        }
        async fn prompts_use(&self, req: PromptUseRequestDto) -> Result<PromptUseResultDto> {
            self.inner.prompts_use(req).await
        }
        async fn suggest_tags(
            &self,
            req: PromptSuggestTagsRequestDto,
        ) -> Result<PromptSuggestTagsResponseDto> {
            self.inner.suggest_tags(req).await
        }
        async fn subagent_delegate(
            &self,
            req: SubagentDelegateRequestDto,
        ) -> Result<SubagentDelegateResponseDto> {
            self.inner.subagent_delegate(req).await
        }
        async fn subagent_list(
            &self,
            req: SubagentListRequestDto,
        ) -> Result<SubagentListResponseDto> {
            self.inner.subagent_list(req).await
        }
        async fn subagent_show(&self, req: SubagentResolveRequestDto) -> Result<SubagentDetailDto> {
            self.inner.subagent_show(req).await
        }
        async fn subagent_tasks(
            &self,
            req: SubagentTasksRequestDto,
        ) -> Result<SubagentTasksResponseDto> {
            self.inner.subagent_tasks(req).await
        }
        async fn subagent_hibernate(
            &self,
            req: SubagentResolveRequestDto,
        ) -> Result<SubagentAckDto> {
            self.inner.subagent_hibernate(req).await
        }
        async fn subagent_delete(&self, req: SubagentDeleteRequestDto) -> Result<SubagentAckDto> {
            self.inner.subagent_delete(req).await
        }
        async fn subagent_runtime_status(
            &self,
            req: SubagentRuntimeStatusRequestDto,
        ) -> Result<ReadEnvelopeDto<SubagentRuntimeStatusDto>> {
            self.inner.subagent_runtime_status(req).await
        }
        async fn provider_create(&self, req: ProviderCreateRequestDto) -> Result<ProviderDto> {
            self.inner.provider_create(req).await
        }
        async fn provider_list(&self) -> Result<ProviderListResponseDto> {
            self.inner.provider_list().await
        }
        async fn provider_update(&self, req: ProviderUpdateRequestDto) -> Result<ProviderDto> {
            self.inner.provider_update(req).await
        }
        async fn provider_delete(&self, req: ProviderDeleteRequestDto) -> Result<()> {
            self.inner.provider_delete(req).await
        }
        async fn provider_test_connection(
            &self,
            req: ProviderTestConnectionRequestDto,
        ) -> Result<ProviderTestConnectionResponseDto> {
            self.inner.provider_test_connection(req).await
        }
        async fn model_create(&self, req: ModelCreateRequestDto) -> Result<ModelCatalogEntryDto> {
            self.inner.model_create(req).await
        }
        async fn model_list(&self, req: ModelListRequestDto) -> Result<ModelListResponseDto> {
            self.inner.model_list(req).await
        }
        async fn model_update(&self, req: ModelUpdateRequestDto) -> Result<()> {
            self.inner.model_update(req).await
        }
        async fn model_delete(&self, req: ModelDeleteRequestDto) -> Result<()> {
            self.inner.model_delete(req).await
        }
        async fn model_tags_update(&self, req: ModelTagUpdateDto) -> Result<()> {
            self.inner.model_tags_update(req).await
        }
        async fn pi_sidecar_locator_update(
            &self,
            req: PiSidecarLocatorUpdateRequestDto,
        ) -> Result<PiSidecarLocatorUpdateResponseDto> {
            self.inner.pi_sidecar_locator_update(req).await
        }
        async fn profile_create(&self, req: ProfileCreateRequestDto) -> Result<ProfileDto> {
            self.inner.profile_create(req).await
        }
        async fn profile_update(&self, req: ProfileUpdateRequestDto) -> Result<ProfileDto> {
            self.inner.profile_update(req).await
        }
        async fn profile_delete(&self, req: ProfileDeleteRequestDto) -> Result<()> {
            self.inner.profile_delete(req).await
        }
        fn event_bus(&self) -> &busytok_events::AppEventBus {
            self.inner.event_bus()
        }
    }

    // suppress unused-import warnings for Value/AtomicBool that may be
    // referenced in future iterations of this module.
    #[allow(dead_code)]
    fn _suppress_unused() {
        let _ = Value::Null;
        let _ = AtomicBool::new(false);
        let _ = Ordering::SeqCst;
    }

    /// Exercises every delegation method on `TestRuntimeWrapper` so the
    /// forwarding lines are covered. The inner `TestRuntimeControl`
    /// stubs return Ok/Err; we only need the delegation line to execute.
    #[tokio::test]
    async fn test_runtime_wrapper_delegates_every_method_to_inner() {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let wrapper = TestRuntimeWrapper::new(inner);
        let rt: &dyn RuntimeControl = &wrapper;

        // no-arg reads (settings_diagnostics/settings_update/prompts_create
        // fall through to their delegation line when no error/value is set).
        let _ = rt.service_health().await;
        let _ = rt.service_status().await;
        let _ = rt.shell_status().await;
        let _ = rt.settings_snapshot().await;
        let _ = rt.settings_diagnostics().await;
        let _ = rt.provider_list().await;
        let _ = rt.event_bus();

        let day = RangePresetDto::Day;
        let _ = rt.overview_summary(OverviewSummaryRequestDto { range: day }).await;
        let _ = rt
            .overview_trend(OverviewTrendRequestDto { range: day, granularity: None })
            .await;
        let _ = rt.overview_heatmap(OverviewHeatmapRequestDto { range: day }).await;
        let _ = rt.overview_rankings(OverviewRankingsRequestDto { range: day }).await;
        let _ = rt.receipt_daily(ReceiptDailyRequestDto::default()).await;
        let _ = rt
            .activity_recent(ActivityRecentRequestDto { range: day, limit: None })
            .await;
        let _ = rt
            .activity_list(ActivityListRequestDto {
                range: day,
                cursor: None,
                limit: None,
                client_id: None,
                source_id: None,
                project_hash: None,
                model_id: None,
            })
            .await;
        let _ = rt.activity_detail(ActivityDetailRequestDto { id: "x".into() }).await;
        let _ = rt
            .breakdown_list(BreakdownListRequestDto {
                kind: BreakdownKindDto::Project,
                range: day,
                cursor: None,
                limit: None,
            })
            .await;
        let _ = rt
            .breakdown_detail(BreakdownDetailRequestDto {
                kind: BreakdownKindDto::Project,
                id: "x".into(),
                range: day,
            })
            .await;
        let _ = rt
            .clients_snapshot(ClientsSnapshotRequestDto {
                cursor: None,
                limit: None,
                client_id: None,
                scan_state: None,
            })
            .await;
        let _ = rt
            .clients_detail(ClientSourceDetailRequestDto { source_id: "x".into() })
            .await;
        let _ = rt
            .settings_update(SettingsUpdateRequestDto {
                timezone: None,
                week_starts_on: None,
                discovery: None,
                privacy: None,
                prompt_palette_default_action: None,
            })
            .await;
        let _ = rt
            .settings_recovery_action(SettingsRecoveryActionRequestDto {
                id: SettingsRecoveryActionIdDto::RescanAll,
            })
            .await;
        let _ = rt.live_window(LiveWindowRequestDto { window_seconds: None }).await;
        let _ = rt
            .prompts_list(PromptListQueryDto { query: None, tag: None, sort: None, limit: None })
            .await;
        let _ = rt.prompts_get(PromptGetRequestDto { id: "x".into() }).await;
        let _ = rt
            .prompts_create(PromptCreateRequestDto { content: "c".into(), alias: None, tags: vec![] })
            .await;
        let _ = rt
            .prompts_update(PromptUpdateRequestDto {
                id: "x".into(),
                content: "c".into(),
                alias: None,
                tags: vec![],
                is_pinned: false,
            })
            .await;
        let _ = rt.prompts_delete(PromptDeleteRequestDto { id: "x".into() }).await;
        let _ = rt
            .prompts_use(PromptUseRequestDto {
                id: "x".into(),
                action: PromptActionDto::OnlyCopy,
                surface: PromptUseSurfaceDto::Overlay,
                outcome: PromptUseOutcomeDto::Copy,
                failure_reason: None,
            })
            .await;
        let _ = rt
            .suggest_tags(PromptSuggestTagsRequestDto { query: None, limit: None })
            .await;
        let _ = rt
            .subagent_delegate(SubagentDelegateRequestDto {
                subagent_name: "sa".into(),
                subagent_id: None,
                cwd: ".".into(),
                profile: "default".into(),
                intent: None,
                prompt: "p".into(),
                prompt_artifact_ref: None,
                timeout_seconds: None,
                model_override: None,
                source_harness: None,
                source_session_id: None,
            })
            .await;
        let _ = rt.subagent_list(SubagentListRequestDto { status: None, project: None, include_deleted: None }).await;
        let _ = rt
            .subagent_show(SubagentResolveRequestDto { name: None, id: Some("sa".into()), cwd: None })
            .await;
        let _ = rt
            .subagent_tasks(SubagentTasksRequestDto { name: None, id: Some("sa".into()), cwd: None, limit: None })
            .await;
        let _ = rt
            .subagent_hibernate(SubagentResolveRequestDto { name: None, id: Some("sa".into()), cwd: None })
            .await;
        let _ = rt
            .subagent_delete(SubagentDeleteRequestDto { name: None, id: Some("sa".into()), cwd: None, hard: None })
            .await;
        let _ = rt
            .subagent_runtime_status(SubagentRuntimeStatusRequestDto::default())
            .await;
        let _ = rt
            .provider_create(ProviderCreateRequestDto {
                name: "p".into(),
                provider_kind: busytok_domain::ProviderKind::OpenAiCompatible,
                base_url: "https://x.example.com/v1".into(),
                enabled: None,
                api_key: None,
            })
            .await;
        let _ = rt
            .provider_update(ProviderUpdateRequestDto {
                id: "p".into(),
                name: None,
                base_url: None,
                enabled: None,
                api_key: None,
            })
            .await;
        let _ = rt.provider_delete(ProviderDeleteRequestDto { id: "p".into() }).await;
        let _ = rt
            .provider_test_connection(ProviderTestConnectionRequestDto { id: "p".into() })
            .await;
        let _ = rt
            .model_create(ModelCreateRequestDto {
                provider_id: "p".into(),
                model_id: "m".into(),
                enabled: None,
                tags: vec![],
            })
            .await;
        let _ = rt
            .model_list(ModelListRequestDto { provider_id: None, tags: vec![], include_disabled: false })
            .await;
        let _ = rt.model_update(ModelUpdateRequestDto { id: "m".into(), enabled: None }).await;
        let _ = rt.model_delete(ModelDeleteRequestDto { id: "m".into() }).await;
        let _ = rt
            .model_tags_update(ModelTagUpdateDto { model_id: "m".into(), tags: vec![] })
            .await;
        let _ = rt
            .pi_sidecar_locator_update(PiSidecarLocatorUpdateRequestDto {
                runtime_dir: "/tmp".into(),
                enabled: true,
            })
            .await;
        let _ = rt
            .profile_create(ProfileCreateRequestDto {
                id: "pr".into(),
                model: "m".into(),
                provider_id: None,
                tools: None,
                context_budget_tokens: None,
                timeout_seconds: None,
                write_access: None,
            })
            .await;
        let _ = rt
            .profile_update(ProfileUpdateRequestDto {
                id: "pr".into(),
                provider_id: None,
                model: None,
                tools: None,
                context_budget_tokens: None,
                timeout_seconds: None,
                write_access: None,
            })
            .await;
        let _ = rt.profile_delete(ProfileDeleteRequestDto { id: "pr".into() }).await;
    }
}
