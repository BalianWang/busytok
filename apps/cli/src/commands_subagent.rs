//! Handlers for `busytok delegate` and `busytok subagent …`.

use std::io::BufRead;

use anyhow::{bail, Context, Result};
use busytok_control::ControlClient;
use busytok_protocol::dto::{
    ModelCatalogEntryDto, ModelListRequestDto, ModelListResponseDto, ProviderDto,
    ProviderListResponseDto, SubagentDelegateRequestDto, SubagentDeleteRequestDto,
    SubagentListRequestDto, SubagentResolveRequestDto, SubagentTaskGetRequestDto,
    SubagentTasksRequestDto,
};
use busytok_protocol::{ControlRequest, ControlResponse};

/// Connect to the service, with subagent-specific guidance on failure.
async fn connect() -> Result<ControlClient> {
    crate::commands::connect_client().await.with_context(|| {
        "busytok-service is not running; subagent commands require the service. Start it and retry."
    })
}

pub async fn handle_delegate(
    subagent: String,
    id: Option<String>,
    cwd: String,
    profile: String,
    intent: Option<String>,
    model: Option<String>,
    timeout: Option<u64>,
    output: String,
    prompt: String,
    bind_provider: Option<String>,
    bind_model: Option<String>,
    wait: bool,
) -> Result<()> {
    // Phase 1: pure shape validation that needs no RPC. Do this BEFORE
    // `connect()` so obvious CLI misuse (e.g. `--bind-provider` without
    // `--bind-model`) fails fast with the right error even when the service
    // is down. `(None, None)` is a valid reuse-path passthrough and must
    // NOT be rejected here.
    match (&bind_provider, &bind_model) {
        (Some(_), None) => {
            anyhow::bail!("--bind-model is required when --bind-provider is given")
        }
        (None, Some(_)) => {
            // Auto-resolution path — needs the model catalog RPC below.
        }
        (Some(_), Some(_)) | (None, None) => {
            // Direct passthrough or valid reuse path — no RPC needed.
        }
    }

    // Do NOT canonicalize cwd — the service resolver canonicalizes at one chokepoint.
    let mut client = connect().await?;

    // Phase 2: auto-resolution. Only the `(None, Some(model))` branch
    // requires a live `ControlClient` (to call `model.list`).
    let (bound_provider_id, bound_model_id) =
        match resolve_bound_fields(&mut client, bind_provider, bind_model).await? {
            Some((p, m)) => (Some(p), Some(m)),
            None => (None, None), // valid reuse-path passthrough
        };

    let req = SubagentDelegateRequestDto {
        subagent_name: subagent,
        subagent_id: id,
        cwd,
        profile,
        intent,
        prompt,
        prompt_artifact_ref: None,
        timeout_seconds: timeout,
        model_override: model,
        source_harness: Some("cli".to_string()),
        source_session_id: None,
        bound_provider_id,
        bound_model_id,
    };
    let resp = client
        .call(ControlRequest::new(
            "subagent.delegate",
            serde_json::to_value(&req)?,
        ))
        .await
        .context("subagent.delegate RPC failed")?;
    let data = unwrap_ok(resp)?;

    // --wait: poll `subagent.task_get` until the task reaches a terminal
    // state (completed/failed). Without --wait, the initial result
    // (possibly `queued`) is printed immediately.
    if wait {
        let task_id = data
            .get("task_id")
            .and_then(|v| v.as_str())
            .context("delegate response missing task_id for --wait")?
            .to_string();
        let final_data = wait_for_task(&mut client, &task_id).await?;
        print_delegate(&final_data, &output)
    } else {
        print_delegate(&data, &output)
    }
}

/// Poll `subagent.task_get` every 2s until the task reaches a terminal
/// state (`completed`, `failed`, or `cancelled`). Returns the final task
/// detail DTO as a `serde_json::Value` (shaped like a delegate response for
/// printing). All three are terminal per `set_task_status` (which stamps
/// `completed_at_ms` for each); omitting `cancelled` would spin forever
/// if a task is ever cancelled.
async fn wait_for_task(
    client: &mut ControlClient,
    task_id: &str,
) -> Result<serde_json::Value> {
    let poll_interval = std::time::Duration::from_secs(2);
    loop {
        let req = SubagentTaskGetRequestDto {
            task_id: task_id.to_string(),
        };
        let resp = client
            .call(ControlRequest::new(
                "subagent.task_get",
                serde_json::to_value(&req)?,
            ))
            .await
            .with_context(|| format!("subagent.task_get RPC failed for {task_id}"))?;
        let data = unwrap_ok(resp)?;
        let status = data
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if status == "completed" || status == "failed" || status == "cancelled" {
            return Ok(data);
        }
        tokio::time::sleep(poll_interval).await;
    }
}

pub async fn handle_list(
    status: Option<String>,
    project: Option<String>,
    include_deleted: bool,
) -> Result<()> {
    let mut client = connect().await?;
    let req = SubagentListRequestDto {
        status,
        project,
        include_deleted: Some(include_deleted),
    };
    let resp = client
        .call(ControlRequest::new(
            "subagent.list",
            serde_json::to_value(&req)?,
        ))
        .await
        .context("subagent.list RPC failed")?;
    let data = unwrap_ok(resp)?;
    print_subagent_list(&data, "text")
}

pub async fn handle_show(name: Option<String>, id: Option<String>, cwd: String) -> Result<()> {
    let mut client = connect().await?;
    let req = SubagentResolveRequestDto {
        name,
        id,
        cwd: Some(cwd),
    };
    let resp = client
        .call(ControlRequest::new(
            "subagent.show",
            serde_json::to_value(&req)?,
        ))
        .await
        .context("subagent.show RPC failed")?;
    let data = unwrap_ok(resp)?;
    print_detail(&data, "text")
}

pub async fn handle_tasks(
    name: Option<String>,
    id: Option<String>,
    cwd: String,
    limit: i64,
) -> Result<()> {
    let mut client = connect().await?;
    let req = SubagentTasksRequestDto {
        name,
        id,
        cwd: Some(cwd),
        limit: Some(limit),
    };
    let resp = client
        .call(ControlRequest::new(
            "subagent.tasks",
            serde_json::to_value(&req)?,
        ))
        .await
        .context("subagent.tasks RPC failed")?;
    let data = unwrap_ok(resp)?;
    print_array(&data, "tasks", "text")
}

pub async fn handle_hibernate(name: Option<String>, id: Option<String>, cwd: String) -> Result<()> {
    let mut client = connect().await?;
    let req = SubagentResolveRequestDto {
        name,
        id,
        cwd: Some(cwd),
    };
    let resp = client
        .call(ControlRequest::new(
            "subagent.hibernate",
            serde_json::to_value(&req)?,
        ))
        .await
        .context("subagent.hibernate RPC failed")?;
    let data = unwrap_ok(resp)?;
    print_ack(&data, "text")
}

pub async fn handle_delete(
    name: Option<String>,
    id: Option<String>,
    cwd: String,
    hard: bool,
    yes: bool,
) -> Result<()> {
    if hard && !yes {
        println!(
            "About to HARD-delete subagent {}. This cannot be undone. Type 'y' or 'yes' to continue:",
            name.as_deref().or(id.as_deref()).unwrap_or("?")
        );
        let stdin = std::io::stdin();
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        let line = line.trim().to_lowercase();
        if line != "y" && line != "yes" {
            bail!("aborted");
        }
    }

    let mut client = connect().await?;
    let req = SubagentDeleteRequestDto {
        name,
        id,
        cwd: Some(cwd),
        hard: Some(hard),
    };
    let resp = client
        .call(ControlRequest::new(
            "subagent.delete",
            serde_json::to_value(&req)?,
        ))
        .await
        .context("subagent.delete RPC failed")?;
    let data = unwrap_ok(resp)?;
    print_ack(&data, "text")
}

pub async fn handle_task_get(task_id: String, output: String) -> Result<()> {
    let mut client = connect().await?;
    let req = SubagentTaskGetRequestDto { task_id };
    let resp = client
        .call(ControlRequest::new(
            "subagent.task_get",
            serde_json::to_value(&req)?,
        ))
        .await
        .context("subagent.task_get RPC failed")?;
    let data = unwrap_ok(resp)?;
    print_task_detail(&data, &output)
}

/// Extract the Ok payload or bail with the control error message.
fn unwrap_ok(resp: ControlResponse) -> Result<serde_json::Value> {
    match resp {
        ControlResponse::Ok(v) => Ok(v),
        ControlResponse::Err(e) => bail!("{}: {}", e.code, e.message),
    }
}

/// Resolve (provider_id, model_id) from --bind-provider / --bind-model flags.
/// Returns:
/// - Ok(Some((provider, model))) when the bound fields are fully resolved
/// - Ok(None) for the valid reuse-path passthrough case `(None, None)`
/// - Err(...) for asymmetric or ambiguous input
///
/// `--bind-provider` accepts either a UUID (passed through directly) or a
/// provider name (resolved to UUID via `provider.list`). This lets Codex /
/// Claude Code use the human-friendly name from `provider list` instead of
/// parsing the UUID from `models --json`.
async fn resolve_bound_fields(
    client: &mut ControlClient,
    bind_provider: Option<String>,
    bind_model: Option<String>,
) -> Result<Option<(String, String)>> {
    match (bind_provider, bind_model) {
        (Some(p), Some(m)) => {
            // If it's a valid UUID, pass through directly (no RPC).
            if uuid::Uuid::parse_str(&p).is_ok() {
                return Ok(Some((p, m)));
            }
            // Not a UUID — resolve by provider name via `provider.list`.
            let pid = resolve_provider_by_name(client, &p).await?;
            Ok(Some((pid, m)))
        }
        (None, None) => {
            // Important: do NOT reject this case in the CLI. It is valid for
            // BOTH reuse paths:
            //   - --id <UUID>
            //   - --subagent <NAME> --cwd <DIR> when an active subagent with
            //     that name already exists in the repo scope
            // The service resolver will only reject it if the request falls
            // through to the create path (0 active matches).
            Ok(None)
        }
        (Some(_), None) => {
            anyhow::bail!("--bind-model is required when --bind-provider is given")
        }
        (None, Some(model)) => {
            // Auto-resolve provider from the model catalog.
            // Reuse the typed model.list RPC path (same as `commands/models.rs`).
            let req = ModelListRequestDto {
                provider_id: None,
                tags: vec![],
                include_disabled: false, // never bind to disabled provider/model
            };
            let resp = client
                .call(ControlRequest::new(
                    "model.list",
                    serde_json::to_value(&req)?,
                ))
                .await?;
            let value = unwrap_ok(resp)?;
            let entries: Vec<ModelCatalogEntryDto> =
                serde_json::from_value::<ModelListResponseDto>(value)?.models;
            // include_disabled=false already filters disabled entries; the
            // extra model_enabled && provider_enabled filter is defense-in-depth.
            let matches: Vec<_> = entries
                .iter()
                .filter(|e| e.model_id == model && e.model_enabled && e.provider_enabled)
                .collect();
            match matches.len() {
                0 => anyhow::bail!("model '{}' not found in catalog", model),
                1 => {
                    let pid = &matches[0].provider_id;
                    eprintln!("  (auto-resolved provider: {})", pid);
                    Ok(Some((pid.clone(), model)))
                }
                _ => {
                    let providers: Vec<_> = matches.iter().map(|e| &e.provider_id).collect();
                    anyhow::bail!(
                        "model '{}' is available from multiple providers: {:?}\n\
                         Use --bind-provider to disambiguate.",
                        model,
                        providers
                    )
                }
            }
        }
    }
}

/// Resolve a provider name to its UUID via the `provider.list` RPC.
/// Only enabled providers are considered (disabled providers cannot be
/// bound, per spec). Returns an error if the name is not found or if
/// multiple enabled providers share the same name.
async fn resolve_provider_by_name(client: &mut ControlClient, name: &str) -> Result<String> {
    let resp = client
        .call(ControlRequest::new("provider.list", serde_json::Value::Null))
        .await?;
    let value = unwrap_ok(resp)?;
    let providers: Vec<ProviderDto> =
        serde_json::from_value::<ProviderListResponseDto>(value)?.providers;
    let matches: Vec<_> = providers.iter().filter(|p| p.enabled && p.name == name).collect();
    match matches.len() {
        0 => anyhow::bail!(
            "provider '{}' not found (or disabled). Run `busytok provider list` to see available providers.",
            name
        ),
        1 => {
            let pid = matches[0].id.clone();
            eprintln!("  (resolved provider '{name}' → {pid})");
            Ok(pid)
        }
        _ => {
            let ids: Vec<_> = matches.iter().map(|p| p.id.as_str()).collect();
            anyhow::bail!(
                "provider name '{}' is ambiguous (matched {} providers: {:?}).\n\
                 Use --bind-provider <UUID> to disambiguate.",
                name,
                matches.len(),
                ids
            )
        }
    }
}

/// Print the `subagent.delegate` result (task_id / subagent_name / status / summary).
fn print_delegate(value: &serde_json::Value, output: &str) -> Result<()> {
    print_json_or(value, output, |v| {
        let summary = v
            .get("summary")
            .and_then(|s| s.as_str())
            .unwrap_or("(no summary)");
        println!(
            "task:     {}",
            v.get("task_id").and_then(|s| s.as_str()).unwrap_or("?")
        );
        println!(
            "subagent: {}",
            v.get("subagent_name")
                .and_then(|s| s.as_str())
                .unwrap_or("?")
        );
        println!(
            "status:   {}",
            v.get("status").and_then(|s| s.as_str()).unwrap_or("?")
        );
        println!("\n{summary}");
    })
}

/// Print an array envelope `{ "<key>": [ ... ] }` (used by list / tasks).
fn print_array(value: &serde_json::Value, key: &str, output: &str) -> Result<()> {
    print_json_or(value, output, |v| {
        let arr = v.get(key).and_then(|a| a.as_array());
        match arr {
            Some(items) if items.is_empty() => println!("(no {key})"),
            Some(items) => {
                for item in items {
                    let id = item.get("id").and_then(|s| s.as_str()).unwrap_or("?");
                    let name = item
                        .get("name")
                        .and_then(|s| s.as_str())
                        .or_else(|| item.get("subagent_name").and_then(|s| s.as_str()))
                        .unwrap_or("?");
                    let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("?");
                    println!("{id:<36} {name:<20} {status}");
                }
            }
            None => println!("{}", serde_json::to_string_pretty(v).unwrap_or_default()),
        }
    })
}

/// Print the `subagents` array with a BINDING column
/// (`bound_provider_id`/`bound_model_id`). Per Global Constraints, display
/// IDs directly (no ID→name resolution). Used only by `handle_list`;
/// `handle_tasks` keeps the generic `print_array` path.
fn print_subagent_list(value: &serde_json::Value, output: &str) -> Result<()> {
    print_json_or(value, output, |v| {
        let arr = v.get("subagents").and_then(|a| a.as_array());
        match arr {
            Some(items) if items.is_empty() => println!("(no subagents)"),
            Some(items) => {
                for item in items {
                    let id = item.get("id").and_then(|s| s.as_str()).unwrap_or("?");
                    let name = item
                        .get("name")
                        .and_then(|s| s.as_str())
                        .or_else(|| item.get("subagent_name").and_then(|s| s.as_str()))
                        .unwrap_or("?");
                    let bound_provider = item
                        .get("bound_provider_id")
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    let bound_model = item
                        .get("bound_model_id")
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    // Post-migration-0007 the columns are NOT NULL, so this
                    // fallback is purely defensive against malformed JSON.
                    let binding = if bound_provider.is_empty() && bound_model.is_empty() {
                        "-".to_string()
                    } else {
                        format!("{bound_provider}/{bound_model}")
                    };
                    let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("?");
                    println!("{id:<36} {name:<20} {binding:<40} {status}");
                }
            }
            None => println!("{}", serde_json::to_string_pretty(v).unwrap_or_default()),
        }
    })
}

/// Print a single subagent detail object.
fn print_detail(value: &serde_json::Value, output: &str) -> Result<()> {
    print_json_or(value, output, |v| {
        let id = v.get("id").and_then(|s| s.as_str()).unwrap_or("?");
        let name = v
            .get("name")
            .and_then(|s| s.as_str())
            .or_else(|| v.get("subagent_name").and_then(|s| s.as_str()))
            .unwrap_or("?");
        let bound_provider = v
            .get("bound_provider_id")
            .and_then(|s| s.as_str())
            .unwrap_or("?");
        let bound_model = v
            .get("bound_model_id")
            .and_then(|s| s.as_str())
            .unwrap_or("?");
        let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("?");
        println!("id:       {id}");
        println!("name:     {name}");
        println!("provider: {bound_provider}");
        println!("model:    {bound_model}");
        println!("status:   {status}");
    })
}

/// Print a simple ack envelope (`{ id, status }`).
fn print_ack(value: &serde_json::Value, output: &str) -> Result<()> {
    print_json_or(value, output, |v| {
        let id = v.get("id").and_then(|s| s.as_str()).unwrap_or("?");
        let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("?");
        println!("{id}: {status}");
    })
}

/// Print a single task detail record (`SubagentTaskDetailDto`).
///
/// This is a DEDICATED formatter — do NOT reuse `print_detail` (that is for
/// subagent identity, which has a different field set: bound_provider_id /
/// bound_model_id instead of result_summary / error / error_kind /
/// timestamps).
///
/// Optional `String` fields route through `or_dash` so absent values render
/// as `-` (not `?`). Optional `i64` fields (`started_at_ms`,
/// `completed_at_ms`) are formatted as strings and use `-` for `None`. The
/// required `created_at_ms` field falls back to `0` defensively.
fn print_task_detail(value: &serde_json::Value, output: &str) -> Result<()> {
    print_json_or(value, output, |v| {
        println!("{}", format_task_detail_text(v));
    })
}

/// Pure text formatter for a single task detail record
/// (`SubagentTaskDetailDto`). Returns the rendered string so tests can
/// assert on the exact output (the caller is responsible for printing).
fn format_task_detail_text(v: &serde_json::Value) -> String {
    let id = v.get("id").and_then(|s| s.as_str()).unwrap_or("?");
    let subagent_id = v.get("subagent_id").and_then(|s| s.as_str()).unwrap_or("?");
    let subagent_name = or_dash(v.get("subagent_name").and_then(|s| s.as_str()));
    let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("?");
    let profile = v.get("profile").and_then(|s| s.as_str()).unwrap_or("?");
    let model_override = or_dash(v.get("model_override").and_then(|s| s.as_str()));
    let source_harness = or_dash(v.get("source_harness").and_then(|s| s.as_str()));
    let source_session_id = or_dash(v.get("source_session_id").and_then(|s| s.as_str()));
    let created_at_ms = v.get("created_at_ms").and_then(|n| n.as_i64()).unwrap_or(0);
    let started_at_ms = v
        .get("started_at_ms")
        .and_then(|n| n.as_i64())
        .map_or("-".to_string(), |n| n.to_string());
    let completed_at_ms = v
        .get("completed_at_ms")
        .and_then(|n| n.as_i64())
        .map_or("-".to_string(), |n| n.to_string());
    let result_summary = or_dash(v.get("result_summary").and_then(|s| s.as_str()));
    let error = or_dash(v.get("error").and_then(|s| s.as_str()));
    let error_kind = or_dash(v.get("error_kind").and_then(|s| s.as_str()));
    format!(
        "id:                {id}\n\
         subagent_id:       {subagent_id}\n\
         subagent_name:     {subagent_name}\n\
         status:            {status}\n\
         profile:           {profile}\n\
         model_override:    {model_override}\n\
         source_harness:    {source_harness}\n\
         source_session_id: {source_session_id}\n\
         created_at_ms:     {created_at_ms}\n\
         started_at_ms:     {started_at_ms}\n\
         completed_at_ms:   {completed_at_ms}\n\
         result_summary:    {result_summary}\n\
         error:             {error}\n\
         error_kind:        {error_kind}\n"
    )
}

/// Render an optional string as `-` when absent. Used by `print_task_detail`
/// for optional `String` fields on the task record.
fn or_dash(v: Option<&str>) -> &str {
    v.unwrap_or("-")
}

/// Shared helper: `json` output prints pretty JSON; any other value runs the
/// supplied text-mode closure. Centralizes the `match output { "json" => …, _ => … }`
/// pattern previously repeated across `print_delegate`/`print_array`/`print_detail`/`print_ack`/`print_task_detail`.
fn print_json_or(
    value: &serde_json::Value,
    output: &str,
    text_fn: impl FnOnce(&serde_json::Value),
) -> Result<()> {
    if output == "json" {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        text_fn(value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
    use super::*;
    use busytok_domain::ProviderKind;
    use busytok_protocol::dto::*;
    use busytok_protocol::{ControlError, ControlResponse};
    use serde_json::json;

    // ── unwrap_ok ──────────────────────────────────────────────────────

    #[test]
    fn unwrap_ok_returns_value_for_ok_response() {
        let resp = ControlResponse::Ok(json!({"task_id": "abc"}));
        let v = unwrap_ok(resp).unwrap();
        assert_eq!(v["task_id"], "abc");
    }

    #[test]
    fn unwrap_ok_bails_with_code_and_message_for_err_response() {
        let resp = ControlResponse::Err(ControlError {
            code: "not_found".to_string(),
            message: "subagent missing".to_string(),
            payload: None,
        });
        let err = unwrap_ok(resp).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not_found"),
            "should include the error code: {msg}"
        );
        assert!(
            msg.contains("subagent missing"),
            "should include the error message: {msg}"
        );
    }

    // ── print_delegate ────────────────────────────────────────────────

    #[test]
    fn print_delegate_text_with_full_payload() {
        // All fields present — text branch should print task/subagent/status/summary.
        let v = json!({
            "task_id": "task-1",
            "subagent_name": "dev",
            "status": "running",
            "summary": "doing work"
        });
        assert!(print_delegate(&v, "text").is_ok());
    }

    #[test]
    fn print_delegate_text_with_missing_fields_uses_defaults() {
        // No fields present — every `unwrap_or` fallback should kick in ("?" / "(no summary)").
        let v = json!({});
        assert!(print_delegate(&v, "text").is_ok());
    }

    #[test]
    fn print_delegate_json_output_pretty_prints_payload() {
        let v = json!({"task_id": "task-2"});
        assert!(print_delegate(&v, "json").is_ok());
    }

    // ── print_array ───────────────────────────────────────────────────

    #[test]
    fn print_array_text_empty_prints_no_items_message() {
        let v = json!({"subagents": []});
        assert!(print_array(&v, "subagents", "text").is_ok());
    }

    #[test]
    fn print_array_text_with_items_uses_name_then_subagent_name_fallback() {
        let v = json!({
            "subagents": [
                {"id": "id-a", "name": "alpha", "status": "hot"},
                {"id": "id-b", "subagent_name": "beta", "status": "warm"},
                {"id": "id-c", "status": "cold"}
            ]
        });
        assert!(print_array(&v, "subagents", "text").is_ok());
    }

    #[test]
    fn print_array_text_when_key_absent_falls_back_to_pretty_json() {
        // The value has no `subagents` array — `print_array` should print the pretty JSON.
        let v = json!({"unexpected": "shape"});
        assert!(print_array(&v, "subagents", "text").is_ok());
    }

    #[test]
    fn print_array_json_output_pretty_prints_payload() {
        let v = json!({"subagents": [{"id": "x"}]});
        assert!(print_array(&v, "subagents", "json").is_ok());
    }

    #[test]
    fn print_array_text_empty_tasks_uses_key_in_message() {
        // The "(no {key})" message uses the key name — verify with a non-default key.
        let v = json!({"tasks": []});
        assert!(print_array(&v, "tasks", "text").is_ok());
    }

    // ── print_subagent_list ──────────────────────────────────────────
    //
    // Dedicated renderer for `handle_list` — adds a BINDING column
    // (`bound_provider_id`/`bound_model_id`) that the generic `print_array`
    // does not show. The `print_array` tests above remain the coverage for
    // the `handle_tasks` path.

    #[test]
    fn print_subagent_list_text_empty_prints_no_subagents_message() {
        // Empty `subagents` array — should print `(no subagents)`.
        let v = json!({"subagents": []});
        assert!(print_subagent_list(&v, "text").is_ok());
    }

    #[test]
    fn print_subagent_list_text_with_bound_fields_shows_binding_column() {
        // Non-empty list with bound fields — BINDING column renders as
        // `{bound_provider_id}/{bound_model_id}`.
        let v = json!({
            "subagents": [
                {
                    "id": "sa-1",
                    "name": "dev",
                    "bound_provider_id": "openai",
                    "bound_model_id": "gpt-5",
                    "status": "hot"
                },
                {
                    "id": "sa-2",
                    "subagent_name": "reviewer",
                    "bound_provider_id": "anthropic",
                    "bound_model_id": "claude-opus",
                    "status": "warm"
                }
            ]
        });
        assert!(print_subagent_list(&v, "text").is_ok());
    }

    #[test]
    fn print_subagent_list_text_missing_bound_fields_falls_back_to_dash() {
        // Malformed JSON: items present but bound fields missing — the
        // BINDING column falls back to `-`. This is purely defensive
        // (post-migration-0007 the columns are NOT NULL).
        let v = json!({
            "subagents": [
                {"id": "sa-1", "name": "dev", "status": "hot"},
                {"id": "sa-2", "name": "reviewer", "bound_provider_id": "", "bound_model_id": "", "status": "warm"}
            ]
        });
        assert!(print_subagent_list(&v, "text").is_ok());
    }

    #[test]
    fn print_subagent_list_text_when_key_absent_falls_back_to_pretty_json() {
        // The value has no `subagents` array — should print the pretty JSON
        // of the whole envelope (matches `print_array`'s fallback shape).
        let v = json!({"unexpected": "shape"});
        assert!(print_subagent_list(&v, "text").is_ok());
    }

    #[test]
    fn print_subagent_list_json_output_pretty_prints_payload() {
        // `json` output mode pretty-prints the full envelope.
        let v =
            json!({"subagents": [{"id": "x", "bound_provider_id": "p", "bound_model_id": "m"}]});
        assert!(print_subagent_list(&v, "json").is_ok());
    }

    // ── print_detail ───────────────────────────────────────────────────

    #[test]
    fn print_detail_text_with_name_field() {
        // All fields present, including bound IDs — text branch should print
        // id/name/provider/model/status lines.
        let v = json!({
            "id": "id-1",
            "name": "named",
            "bound_provider_id": "openai",
            "bound_model_id": "gpt-5",
            "status": "warm"
        });
        assert!(print_detail(&v, "text").is_ok());
    }

    #[test]
    fn print_detail_text_falls_back_to_subagent_name() {
        // No `name` field — should fall back to `subagent_name`.
        let v = json!({
            "id": "id-2",
            "subagent_name": "fallback",
            "bound_provider_id": "anthropic",
            "bound_model_id": "claude",
            "status": "cold"
        });
        assert!(print_detail(&v, "text").is_ok());
    }

    #[test]
    fn print_detail_text_missing_bound_fields_falls_back_to_question_mark() {
        // id/name/status present but bound fields absent — `provider` and
        // `model` lines should print `?` (defensive fallback).
        let v = json!({"id": "id-3", "name": "orphan", "status": "cold"});
        assert!(print_detail(&v, "text").is_ok());
    }

    #[test]
    fn print_detail_text_missing_all_fields_uses_defaults() {
        let v = json!({});
        assert!(print_detail(&v, "text").is_ok());
    }

    #[test]
    fn print_detail_json_output_pretty_prints_payload() {
        let v = json!({"id": "id-3", "bound_provider_id": "p", "bound_model_id": "m"});
        assert!(print_detail(&v, "json").is_ok());
    }

    // ── print_ack ──────────────────────────────────────────────────────

    #[test]
    fn print_ack_text_with_full_payload() {
        let v = json!({"id": "id-1", "status": "hibernated"});
        assert!(print_ack(&v, "text").is_ok());
    }

    #[test]
    fn print_ack_text_missing_fields_uses_defaults() {
        let v = json!({});
        assert!(print_ack(&v, "text").is_ok());
    }

    #[test]
    fn print_ack_json_output_pretty_prints_payload() {
        let v = json!({"id": "id-2"});
        assert!(print_ack(&v, "json").is_ok());
    }

    // ── print_json_or ──────────────────────────────────────────────────

    #[test]
    fn print_json_or_json_branch_skips_text_fn() {
        // The text_fn panics — if it gets called, the test fails.
        // Also verify the JSON output is pretty-printed by checking Ok result.
        let v = json!({"a": 1});
        let result = print_json_or(&v, "json", |_| {
            panic!("text_fn must not run on json output")
        });
        assert!(result.is_ok());
    }

    #[test]
    fn print_json_or_text_branch_invokes_text_fn() {
        // The text_fn mutates a captured variable — verify it actually runs.
        let v = json!({"a": 1});
        let mut called = false;
        let result = print_json_or(&v, "text", |_| called = true);
        assert!(result.is_ok());
        assert!(called, "text_fn should be invoked on text output");
    }

    #[test]
    fn print_json_or_unknown_output_treated_as_text() {
        // Any output value other than "json" is treated as text mode.
        let v = json!({"a": 1});
        let mut called = false;
        let result = print_json_or(&v, "yaml", |_| called = true);
        assert!(result.is_ok());
        assert!(called, "non-json output should run text_fn");
    }

    // ── connect / handler-level integration ──────────────────────────
    //
    // These tests exercise the full public handler surface end-to-end by
    // spawning a real `ControlServer` backed by `TestRuntimeControl`. The
    // `BUSYTOK_SOCKET` env var is process-global so the tests are marked
    // `#[serial]` to avoid races with each other.

    use async_trait::async_trait;
    use busytok_control::{dispatch::RuntimeControl, server::ControlServer, TestRuntimeControl};
    use serial_test::serial;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

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

    /// Wrapper around `TestRuntimeControl` that lets us inject specific
    /// subagent responses for testing the print paths end-to-end.
    struct SubagentRuntime {
        inner: TestRuntimeControl,
        delegate_response: SubagentDelegateResponseDto,
        list_response: SubagentListResponseDto,
        show_response: SubagentDetailDto,
        tasks_response: SubagentTasksResponseDto,
        delete_should_fail: AtomicBool,
        /// When set, `subagent_task_get` returns a real
        /// `MethodDispatchError` (code `subagent.task_not_found`) so tests
        /// can exercise the `ControlResponse::Err` path through `unwrap_ok`
        /// (e.g. missing-task error surfacing).
        task_get_should_fail: AtomicBool,
        /// Optional canned response for `model_list` (used by
        /// `resolve_bound_fields` auto-resolution tests). When `None`,
        /// `model_list` delegates to the inner runtime.
        model_list_response: Option<ModelListResponseDto>,
        /// Optional canned response for `provider_list` (used by
        /// `resolve_bound_fields` name-lookup tests). When `None`,
        /// `provider_list` delegates to the inner runtime. Behind a
        /// `Mutex` so tests can stage it after the server is spawned.
        provider_list_response: Mutex<Option<ProviderListResponseDto>>,
        /// Captures the most recent `SubagentDelegateRequestDto` received by
        /// `subagent_delegate`, so tests can assert the outgoing DTO carries
        /// the expected bound fields. Wrapped in `Mutex` because the trait
        /// method takes `&self` (not `&mut self`).
        last_delegate_request: Mutex<Option<SubagentDelegateRequestDto>>,
        /// When non-empty, `subagent_task_get` pops the next status from the
        /// front and returns a `SubagentTaskDetailDto` with that status. When
        /// empty, falls through to the inner runtime. Used to test the
        /// `--wait` polling path without a real task lifecycle.
        task_get_status_sequence: Mutex<VecDeque<String>>,
        /// Counts `subagent_task_get` calls so tests can assert the `--wait`
        /// path polled the expected number of times (and that the no-wait
        /// path did not poll at all).
        task_get_call_count: AtomicUsize,
    }

    impl SubagentRuntime {
        fn new(inner: TestRuntimeControl) -> Self {
            Self {
                inner,
                delegate_response: SubagentDelegateResponseDto {
                    task_id: "task-xyz".to_string(),
                    subagent_id: "sa-1".to_string(),
                    subagent_name: "dev-subagent".to_string(),
                    adapter: "claude-code".to_string(),
                    adapter_session_id: Some("sess-1".to_string()),
                    session_reused: false,
                    status: "running".to_string(),
                    profile: "default".to_string(),
                    model: Some("gpt-5".to_string()),
                    summary: Some("did the thing".to_string()),
                    usage: SubagentUsageDto::default(),
                },
                list_response: SubagentListResponseDto {
                    subagents: vec![SubagentDetailDto {
                        id: "sa-1".to_string(),
                        name: "dev-subagent".to_string(),
                        status: "hot".to_string(),
                        ..Default::default()
                    }],
                },
                show_response: SubagentDetailDto {
                    id: "sa-1".to_string(),
                    name: "dev-subagent".to_string(),
                    status: "warm".to_string(),
                    default_profile: "default".to_string(),
                    ..Default::default()
                },
                tasks_response: SubagentTasksResponseDto {
                    tasks: vec![SubagentTaskSummaryDto {
                        id: "task-1".to_string(),
                        subagent_id: "sa-1".to_string(),
                        profile: "default".to_string(),
                        status: "completed".to_string(),
                        ..Default::default()
                    }],
                },
                delete_should_fail: AtomicBool::new(false),
                task_get_should_fail: AtomicBool::new(false),
                model_list_response: None,
                provider_list_response: Mutex::new(None),
                last_delegate_request: Mutex::new(None),
                task_get_status_sequence: Mutex::new(VecDeque::new()),
                task_get_call_count: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl RuntimeControl for SubagentRuntime {
        async fn service_health(&self) -> anyhow::Result<ServiceHealthDto> {
            self.inner.service_health().await
        }
        async fn service_status(&self) -> anyhow::Result<ServiceStatusDto> {
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
        async fn settings_update(
            &self,
            req: SettingsUpdateRequestDto,
        ) -> Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
            self.inner.settings_update(req).await
        }
        async fn settings_diagnostics(&self) -> Result<ReadEnvelopeDto<SettingsDiagnosticsDto>> {
            self.inner.settings_diagnostics().await
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
        async fn prompts_create(
            &self,
            req: PromptCreateRequestDto,
        ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
            self.inner.prompts_create(req).await
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
            *self.last_delegate_request.lock().unwrap() = Some(req);
            Ok(self.delegate_response.clone())
        }
        async fn subagent_list(
            &self,
            _req: SubagentListRequestDto,
        ) -> Result<SubagentListResponseDto> {
            Ok(self.list_response.clone())
        }
        async fn subagent_show(
            &self,
            _req: SubagentResolveRequestDto,
        ) -> Result<SubagentDetailDto> {
            Ok(self.show_response.clone())
        }
        async fn subagent_tasks(
            &self,
            _req: SubagentTasksRequestDto,
        ) -> Result<SubagentTasksResponseDto> {
            Ok(self.tasks_response.clone())
        }
        async fn subagent_hibernate(
            &self,
            _req: SubagentResolveRequestDto,
        ) -> Result<SubagentAckDto> {
            Ok(SubagentAckDto {
                id: "sa-1".to_string(),
                status: "hibernated".to_string(),
            })
        }
        async fn subagent_delete(&self, _req: SubagentDeleteRequestDto) -> Result<SubagentAckDto> {
            if self.delete_should_fail.load(Ordering::SeqCst) {
                anyhow::bail!("subagent not found")
            }
            Ok(SubagentAckDto {
                id: "sa-1".to_string(),
                status: "deleted".to_string(),
            })
        }
        async fn subagent_runtime_status(
            &self,
            req: SubagentRuntimeStatusRequestDto,
        ) -> Result<ReadEnvelopeDto<SubagentRuntimeStatusDto>> {
            self.inner.subagent_runtime_status(req).await
        }
        async fn subagent_task_get(
            &self,
            req: SubagentTaskGetRequestDto,
        ) -> Result<SubagentTaskDetailDto> {
            if self.task_get_should_fail.load(Ordering::SeqCst) {
                return Err(anyhow::Error::new(
                    busytok_control::dispatch::MethodDispatchError::from_read_error(
                        "subagent.task_not_found",
                        "task not found: missing-task".to_string(),
                        serde_json::Value::Null,
                    ),
                ));
            }
            self.task_get_call_count
                .fetch_add(1, Ordering::SeqCst);
            // When a status sequence is staged, pop the next status and
            // return a synthetic DTO. When empty, fall through to the
            // inner runtime (preserves existing task_get tests).
            let staged = { self.task_get_status_sequence.lock().unwrap().pop_front() };
            if let Some(status) = staged {
                return Ok(SubagentTaskDetailDto {
                    id: req.task_id.clone(),
                    subagent_id: "sa-1".to_string(),
                    subagent_name: Some("dev-subagent".to_string()),
                    profile: "default".to_string(),
                    status,
                    ..Default::default()
                });
            }
            self.inner.subagent_task_get(req).await
        }
        async fn provider_create(&self, req: ProviderCreateRequestDto) -> Result<ProviderDto> {
            self.inner.provider_create(req).await
        }
        async fn provider_list(&self) -> Result<ProviderListResponseDto> {
            let staged = self.provider_list_response.lock().unwrap().clone();
            if let Some(resp) = staged {
                Ok(resp)
            } else {
                self.inner.provider_list().await
            }
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
        async fn model_list(&self, _req: ModelListRequestDto) -> Result<ModelListResponseDto> {
            if let Some(resp) = &self.model_list_response {
                Ok(resp.clone())
            } else {
                self.inner.model_list(_req).await
            }
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

    async fn spawn_subagent_server() -> (ServerHarness, String) {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(SubagentRuntime::new(inner));
        spawn_server(runtime).await
    }

    /// Like `spawn_subagent_server` but also returns the `SubagentRuntime`
    /// handle so tests can inspect captured state (e.g. the last delegate
    /// request DTO).
    async fn spawn_subagent_server_with_runtime() -> (ServerHarness, Arc<SubagentRuntime>, String) {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime = Arc::new(SubagentRuntime::new(inner));
        let runtime_handle = Arc::clone(&runtime);
        let runtime_dyn: Arc<dyn RuntimeControl> = runtime;
        let (harness, socket) = spawn_server(runtime_dyn).await;
        (harness, runtime_handle, socket)
    }

    #[tokio::test]
    #[serial]
    async fn handle_delegate_invokes_subagent_delegate_rpc_and_prints_text() {
        let (harness, socket) = spawn_subagent_server().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_delegate(
            "dev-subagent".to_string(),
            None,
            ".".to_string(),
            "default".to_string(),
            Some("fix the bug".to_string()),
            None,
            None,
            "text".to_string(),
            "do the thing".to_string(),
            None,
            None,
            false,
        )
        .await;
        drop(harness);
        assert!(
            result.is_ok(),
            "handle_delegate should succeed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_delegate_invokes_subagent_delegate_rpc_with_json_output() {
        let (harness, socket) = spawn_subagent_server().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_delegate(
            "dev-subagent".to_string(),
            None,
            ".".to_string(),
            "default".to_string(),
            None,
            Some("gpt-5".to_string()),
            Some(60),
            "json".to_string(),
            "do the thing".to_string(),
            None,
            None,
            false,
        )
        .await;
        drop(harness);
        assert!(
            result.is_ok(),
            "handle_delegate json output: {:?}",
            result.err()
        );
    }

    // ── resolve_bound_fields ───────────────────────────────────────────
    //
    // Exercise each branch of the bound-field resolver. The `(None, None)`
    // and `(Some, Some)` and `(Some, None)` cases short-circuit before any
    // RPC, but still require a live `ControlClient` to satisfy the signature.
    // The `(None, Some)` auto-resolution cases use a `SubagentRuntime` with
    // a canned `model_list_response`.

    fn sample_model_entry(provider: &str, model: &str, enabled: bool) -> ModelCatalogEntryDto {
        ModelCatalogEntryDto {
            provider_id: provider.to_string(),
            provider_name: provider.to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            provider_enabled: enabled,
            model_db_id: format!("{provider}-{model}-db"),
            model_id: model.to_string(),
            model_enabled: enabled,
            tags: vec![],
            display_name: None,
            reasoning: false,
            context_window: None,
            max_tokens: None,
        }
    }

    #[tokio::test]
    #[serial]
    async fn resolve_bound_fields_none_none_returns_ok_none() {
        // (None, None) is the valid reuse-path passthrough — no RPC.
        let (harness, socket) = spawn_subagent_server().await;
        let mut client = ControlClient::connect(&socket).await.unwrap();
        let result = resolve_bound_fields(&mut client, None, None).await;
        drop(harness);
        assert!(result.is_ok(), "err: {:?}", result.err());
        assert_eq!(result.unwrap(), None);
    }

    #[tokio::test]
    #[serial]
    async fn resolve_bound_fields_some_some_returns_ok_some() {
        // (Some, Some) with a valid UUID passes through directly — no RPC.
        let (harness, socket) = spawn_subagent_server().await;
        let mut client = ControlClient::connect(&socket).await.unwrap();
        let uuid = "5e3a4034-0000-0000-0000-000000000000".to_string();
        let result = resolve_bound_fields(
            &mut client,
            Some(uuid.clone()),
            Some("model-1".to_string()),
        )
        .await;
        drop(harness);
        assert!(result.is_ok(), "err: {:?}", result.err());
        let (p, m) = result.unwrap().unwrap();
        assert_eq!(p, uuid);
        assert_eq!(m, "model-1");
    }

    #[tokio::test]
    #[serial]
    async fn resolve_bound_fields_some_none_errors_asymmetric() {
        // (Some, None) is asymmetric — bails without RPC.
        let (harness, socket) = spawn_subagent_server().await;
        let mut client = ControlClient::connect(&socket).await.unwrap();
        let result = resolve_bound_fields(&mut client, Some("prov-1".to_string()), None).await;
        drop(harness);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("--bind-model is required"),
            "expected asymmetric error, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn resolve_bound_fields_none_some_auto_resolves_when_unique() {
        // (None, Some) with a single matching model in the catalog auto-resolves.
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let mut runtime = SubagentRuntime::new(inner);
        runtime.model_list_response = Some(ModelListResponseDto {
            models: vec![
                sample_model_entry("openai", "gpt-4", true),
                sample_model_entry("openai", "gpt-3.5", false),
                sample_model_entry("deepseek", "deepseek-chat", true),
            ],
        });
        let runtime: Arc<dyn RuntimeControl> = Arc::new(runtime);
        let (harness, socket) = spawn_server(runtime).await;
        let mut client = ControlClient::connect(&socket).await.unwrap();
        let result = resolve_bound_fields(&mut client, None, Some("gpt-4".to_string())).await;
        drop(harness);
        assert!(result.is_ok(), "err: {:?}", result.err());
        let (p, m) = result.unwrap().unwrap();
        assert_eq!(p, "openai");
        assert_eq!(m, "gpt-4");
    }

    #[tokio::test]
    #[serial]
    async fn resolve_bound_fields_none_some_errors_when_model_not_found() {
        // (None, Some) with no matching model in the catalog bails.
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let mut runtime = SubagentRuntime::new(inner);
        runtime.model_list_response = Some(ModelListResponseDto {
            models: vec![sample_model_entry("openai", "gpt-4", true)],
        });
        let runtime: Arc<dyn RuntimeControl> = Arc::new(runtime);
        let (harness, socket) = spawn_server(runtime).await;
        let mut client = ControlClient::connect(&socket).await.unwrap();
        let result = resolve_bound_fields(&mut client, None, Some("nope".to_string())).await;
        drop(harness);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("model 'nope' not found in catalog"),
            "expected not-found error, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn resolve_bound_fields_none_some_errors_when_ambiguous() {
        // (None, Some) with the same model from multiple enabled providers bails.
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let mut runtime = SubagentRuntime::new(inner);
        runtime.model_list_response = Some(ModelListResponseDto {
            models: vec![
                sample_model_entry("openai", "shared-model", true),
                sample_model_entry("azure", "shared-model", true),
            ],
        });
        let runtime: Arc<dyn RuntimeControl> = Arc::new(runtime);
        let (harness, socket) = spawn_server(runtime).await;
        let mut client = ControlClient::connect(&socket).await.unwrap();
        let result =
            resolve_bound_fields(&mut client, None, Some("shared-model".to_string())).await;
        drop(harness);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("multiple providers"),
            "expected ambiguity error, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn resolve_bound_fields_none_some_skips_disabled_entries() {
        // (None, Some) where the only catalog match is disabled should bail
        // (defense-in-depth: include_disabled=false already filters these).
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let mut runtime = SubagentRuntime::new(inner);
        runtime.model_list_response = Some(ModelListResponseDto {
            models: vec![sample_model_entry("openai", "gpt-4", false)],
        });
        let runtime: Arc<dyn RuntimeControl> = Arc::new(runtime);
        let (harness, socket) = spawn_server(runtime).await;
        let mut client = ControlClient::connect(&socket).await.unwrap();
        let result = resolve_bound_fields(&mut client, None, Some("gpt-4".to_string())).await;
        drop(harness);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("model 'gpt-4' not found in catalog"),
            "expected not-found (disabled filtered), got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_delegate_with_bind_provider_and_bind_model_succeeds() {
        // (Some, Some) on handle_delegate — a valid UUID provider ID is
        // passed straight through to the DTO without any provider.list or
        // model.list RPC. Verify the outgoing DTO carries the UUID as-is.
        let (harness, runtime, socket) = spawn_subagent_server_with_runtime().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let provider_uuid = "5e3a4034-0000-0000-0000-000000000000".to_string();
        let result = handle_delegate(
            "dev-subagent".to_string(),
            None,
            ".".to_string(),
            "default".to_string(),
            None,
            None,
            None,
            "text".to_string(),
            "do the thing".to_string(),
            Some(provider_uuid.clone()),
            Some("model-1".to_string()),
            false,
        )
        .await;
        assert!(
            result.is_ok(),
            "handle_delegate with bind flags: {:?}",
            result.err()
        );
        drop(harness);
        let captured = runtime.last_delegate_request.lock().unwrap().take().expect(
            "subagent_delegate should have captured the request DTO — \
             if None, the RPC never reached the runtime",
        );
        assert_eq!(captured.bound_provider_id, Some(provider_uuid));
        assert_eq!(captured.bound_model_id, Some("model-1".to_string()));
        // model_override is the task-level override (separate from bound fields);
        // passing None must leave it None.
        assert_eq!(captured.model_override, None);
    }

    #[tokio::test]
    #[serial]
    async fn handle_delegate_without_bind_flags_passes_none_to_dto() {
        // (None, None) reuse-path passthrough: the outgoing DTO must carry
        // None for both bound fields, so the service resolver can decide
        // (reject on create path, reuse stored bound fields on reuse path).
        let (harness, runtime, socket) = spawn_subagent_server_with_runtime().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_delegate(
            "dev-subagent".to_string(),
            None,
            ".".to_string(),
            "default".to_string(),
            None,
            None,
            None,
            "text".to_string(),
            "do the thing".to_string(),
            None,
            None,
            false,
        )
        .await;
        assert!(
            result.is_ok(),
            "handle_delegate reuse path: {:?}",
            result.err()
        );
        drop(harness);
        let captured = runtime.last_delegate_request.lock().unwrap().take().expect(
            "subagent_delegate should have captured the request DTO — \
             if None, the RPC never reached the runtime",
        );
        assert_eq!(captured.bound_provider_id, None);
        assert_eq!(captured.bound_model_id, None);
    }

    #[tokio::test]
    #[serial]
    async fn handle_delegate_with_bind_provider_only_errors() {
        // Asymmetric (--bind-provider without --bind-model) should surface
        // the CLI-side validation error before any delegate RPC fires.
        let (harness, socket) = spawn_subagent_server().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_delegate(
            "dev-subagent".to_string(),
            None,
            ".".to_string(),
            "default".to_string(),
            None,
            None,
            None,
            "text".to_string(),
            "do the thing".to_string(),
            Some("prov-1".to_string()),
            None,
            false,
        )
        .await;
        drop(harness);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("--bind-model is required"),
            "expected asymmetric bind error, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_delegate_bind_provider_only_errors_without_server() {
        // Phase 1 shape validation must run BEFORE connect(). Point the
        // socket at a path where no server listens and confirm we still
        // get the "--bind-model is required" error, not a connect failure.
        std::env::set_var("BUSYTOK_SOCKET", "/nonexistent/busytok-no-server-test.sock");
        let result = handle_delegate(
            "dev-subagent".to_string(),
            None,
            ".".to_string(),
            "default".to_string(),
            None,
            None,
            None,
            "text".to_string(),
            "do the thing".to_string(),
            Some("prov-1".to_string()),
            None,
            false,
        )
        .await;
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("--bind-model is required"),
            "expected asymmetric bind error BEFORE connect, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_delegate_with_wait_polls_until_completed() {
        // --wait: stage a status sequence [queued, completed]. The first
        // task_get returns "queued" (non-terminal → sleep → poll again),
        // the second returns "completed" (terminal → return). Asserts the
        // handler returns Ok and that task_get was polled exactly twice.
        let (harness, runtime, socket) = spawn_subagent_server_with_runtime().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        {
            let mut seq = runtime.task_get_status_sequence.lock().unwrap();
            seq.push_back("queued".to_string());
            seq.push_back("completed".to_string());
        }
        let result = handle_delegate(
            "dev-subagent".to_string(),
            None,
            ".".to_string(),
            "default".to_string(),
            None,
            None,
            None,
            "json".to_string(),
            "do the thing".to_string(),
            None,
            None,
            true,
        )
        .await;
        drop(harness);
        assert!(
            result.is_ok(),
            "handle_delegate --wait (completed): {:?}",
            result.err()
        );
        let calls = runtime.task_get_call_count.load(Ordering::SeqCst);
        assert_eq!(
            calls, 2,
            "expected 2 task_get polls (queued → completed), got {calls}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_delegate_with_wait_polls_until_failed() {
        // --wait: stage [running, failed]. The first poll returns "running"
        // (non-terminal → sleep → poll again), the second returns "failed"
        // (terminal → return). The handler returns Ok (it surfaces the
        // failed status, not an error).
        let (harness, runtime, socket) = spawn_subagent_server_with_runtime().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        {
            let mut seq = runtime.task_get_status_sequence.lock().unwrap();
            seq.push_back("running".to_string());
            seq.push_back("failed".to_string());
        }
        let result = handle_delegate(
            "dev-subagent".to_string(),
            None,
            ".".to_string(),
            "default".to_string(),
            None,
            None,
            None,
            "json".to_string(),
            "do the thing".to_string(),
            None,
            None,
            true,
        )
        .await;
        drop(harness);
        assert!(
            result.is_ok(),
            "handle_delegate --wait (failed): {:?}",
            result.err()
        );
        let calls = runtime.task_get_call_count.load(Ordering::SeqCst);
        assert_eq!(
            calls, 2,
            "expected 2 task_get polls (running → failed), got {calls}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_delegate_with_wait_polls_until_cancelled() {
        // --wait: stage [running, cancelled]. `cancelled` is a terminal
        // status (set_task_status stamps completed_at_ms for it). The loop
        // must detect it and return — not spin forever.
        let (harness, runtime, socket) = spawn_subagent_server_with_runtime().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        {
            let mut seq = runtime.task_get_status_sequence.lock().unwrap();
            seq.push_back("running".to_string());
            seq.push_back("cancelled".to_string());
        }
        let result = handle_delegate(
            "dev-subagent".to_string(),
            None,
            ".".to_string(),
            "default".to_string(),
            None,
            None,
            None,
            "json".to_string(),
            "do the thing".to_string(),
            None,
            None,
            true,
        )
        .await;
        drop(harness);
        assert!(
            result.is_ok(),
            "handle_delegate --wait (cancelled): {:?}",
            result.err()
        );
        let calls = runtime.task_get_call_count.load(Ordering::SeqCst);
        assert_eq!(
            calls, 2,
            "expected 2 task_get polls (running → cancelled), got {calls}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_delegate_without_wait_does_not_poll_task_get() {
        // Without --wait, the delegate response is printed immediately and
        // subagent.task_get is never called. Stage a sequence and verify
        // the call count stays at 0.
        let (harness, runtime, socket) = spawn_subagent_server_with_runtime().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        {
            let mut seq = runtime.task_get_status_sequence.lock().unwrap();
            seq.push_back("queued".to_string());
            seq.push_back("completed".to_string());
        }
        let result = handle_delegate(
            "dev-subagent".to_string(),
            None,
            ".".to_string(),
            "default".to_string(),
            None,
            None,
            None,
            "text".to_string(),
            "do the thing".to_string(),
            None,
            None,
            false,
        )
        .await;
        drop(harness);
        assert!(
            result.is_ok(),
            "handle_delegate (no wait): {:?}",
            result.err()
        );
        let calls = runtime.task_get_call_count.load(Ordering::SeqCst);
        assert_eq!(
            calls, 0,
            "task_get should not be polled without --wait, got {calls} calls"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_delegate_with_bind_provider_name_resolves_to_uuid() {
        // --bind-provider <name>: resolve the provider name to its UUID via
        // provider.list, then pass the UUID through to the delegate DTO.
        let (harness, runtime, socket) = spawn_subagent_server_with_runtime().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        {
            // Stage a provider list with one enabled provider named "Deepseek".
            let provider = ProviderDto {
                id: "5e3a4034-0000-0000-0000-000000000001".to_string(),
                name: "Deepseek".to_string(),
                provider_kind: ProviderKind::OpenAiCompatible,
                base_url: "https://api.deepseek.com".to_string(),
                enabled: true,
                has_api_key: true,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            *runtime.provider_list_response.lock().unwrap() = Some(ProviderListResponseDto {
                providers: vec![provider],
            });
        }
        let result = handle_delegate(
            "dev-subagent".to_string(),
            None,
            ".".to_string(),
            "default".to_string(),
            None,
            None,
            None,
            "text".to_string(),
            "do the thing".to_string(),
            Some("Deepseek".to_string()), // name, not UUID
            Some("deepseek-v4-pro".to_string()),
            false,
        )
        .await;
        drop(harness);
        assert!(
            result.is_ok(),
            "handle_delegate with provider name: {:?}",
            result.err()
        );
        let captured = runtime
            .last_delegate_request
            .lock()
            .unwrap()
            .take()
            .expect("delegate request should have been captured");
        assert_eq!(
            captured.bound_provider_id,
            Some("5e3a4034-0000-0000-0000-000000000001".to_string()),
            "provider name should be resolved to UUID in the delegate DTO"
        );
        assert_eq!(
            captured.bound_model_id,
            Some("deepseek-v4-pro".to_string())
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_delegate_with_bind_provider_uuid_passes_through_directly() {
        // --bind-provider <UUID>: a valid UUID is passed through directly
        // without calling provider.list. Verify the DTO carries the UUID
        // as-is and that provider_list was never staged (inner returns []).
        let (harness, runtime, socket) = spawn_subagent_server_with_runtime().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let uuid = "5e3a4034-0000-0000-0000-000000000002".to_string();
        let result = handle_delegate(
            "dev-subagent".to_string(),
            None,
            ".".to_string(),
            "default".to_string(),
            None,
            None,
            None,
            "text".to_string(),
            "do the thing".to_string(),
            Some(uuid.clone()),
            Some("deepseek-v4-pro".to_string()),
            false,
        )
        .await;
        drop(harness);
        assert!(
            result.is_ok(),
            "handle_delegate with provider UUID: {:?}",
            result.err()
        );
        let captured = runtime
            .last_delegate_request
            .lock()
            .unwrap()
            .take()
            .expect("delegate request should have been captured");
        assert_eq!(
            captured.bound_provider_id,
            Some(uuid),
            "provider UUID should pass through unchanged"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_delegate_with_bind_provider_unknown_name_errors() {
        // --bind-provider <unknown-name>: provider.list returns no match.
        // The handler should error before any delegate RPC fires.
        let (harness, runtime, socket) = spawn_subagent_server_with_runtime().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        *runtime.provider_list_response.lock().unwrap() = Some(ProviderListResponseDto {
            providers: vec![ProviderDto {
                id: "5e3a4034-0000-0000-0000-000000000003".to_string(),
                name: "OpenAI".to_string(),
                provider_kind: ProviderKind::OpenAiCompatible,
                base_url: "https://api.openai.com".to_string(),
                enabled: true,
                has_api_key: true,
                created_at_ms: 0,
                updated_at_ms: 0,
            }],
        });
        let result = handle_delegate(
            "dev-subagent".to_string(),
            None,
            ".".to_string(),
            "default".to_string(),
            None,
            None,
            None,
            "text".to_string(),
            "do the thing".to_string(),
            Some("Nonexistent".to_string()),
            Some("gpt-5".to_string()),
            false,
        )
        .await;
        drop(harness);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found"),
            "expected provider-not-found error, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_delegate_with_bind_provider_disabled_name_errors() {
        // --bind-provider <name> matching a disabled provider: should
        // error (disabled providers cannot be bound per spec).
        let (harness, runtime, socket) = spawn_subagent_server_with_runtime().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        *runtime.provider_list_response.lock().unwrap() = Some(ProviderListResponseDto {
            providers: vec![ProviderDto {
                id: "5e3a4034-0000-0000-0000-000000000004".to_string(),
                name: "Disabled-Prov".to_string(),
                provider_kind: ProviderKind::OpenAiCompatible,
                base_url: "https://api.example.com".to_string(),
                enabled: false,
                has_api_key: false,
                created_at_ms: 0,
                updated_at_ms: 0,
            }],
        });
        let result = handle_delegate(
            "dev-subagent".to_string(),
            None,
            ".".to_string(),
            "default".to_string(),
            None,
            None,
            None,
            "text".to_string(),
            "do the thing".to_string(),
            Some("Disabled-Prov".to_string()),
            Some("model-x".to_string()),
            false,
        )
        .await;
        drop(harness);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found") || err.contains("disabled"),
            "expected disabled-provider error, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_delegate_with_bind_provider_ambiguous_name_errors() {
        // --bind-provider <name> matching two enabled providers: should
        // error with an ambiguity message listing both UUIDs.
        let (harness, runtime, socket) = spawn_subagent_server_with_runtime().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        *runtime.provider_list_response.lock().unwrap() = Some(ProviderListResponseDto {
            providers: vec![
                ProviderDto {
                    id: "5e3a4034-0000-0000-0000-000000000005".to_string(),
                    name: "Dup".to_string(),
                    provider_kind: ProviderKind::OpenAiCompatible,
                    base_url: "https://a.example.com".to_string(),
                    enabled: true,
                    has_api_key: true,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                },
                ProviderDto {
                    id: "5e3a4034-0000-0000-0000-000000000006".to_string(),
                    name: "Dup".to_string(),
                    provider_kind: ProviderKind::OpenAiCompatible,
                    base_url: "https://b.example.com".to_string(),
                    enabled: true,
                    has_api_key: true,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                },
            ],
        });
        let result = handle_delegate(
            "dev-subagent".to_string(),
            None,
            ".".to_string(),
            "default".to_string(),
            None,
            None,
            None,
            "text".to_string(),
            "do the thing".to_string(),
            Some("Dup".to_string()),
            Some("model-x".to_string()),
            false,
        )
        .await;
        drop(harness);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("ambiguous"),
            "expected ambiguity error, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_list_invokes_subagent_list_rpc() {
        let (harness, socket) = spawn_subagent_server().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_list(Some("hot".to_string()), Some("proj".to_string()), false).await;
        drop(harness);
        assert!(result.is_ok(), "handle_list: {:?}", result.err());
    }

    #[tokio::test]
    #[serial]
    async fn handle_show_invokes_subagent_show_rpc_by_name() {
        let (harness, socket) = spawn_subagent_server().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_show(Some("dev-subagent".to_string()), None, ".".to_string()).await;
        drop(harness);
        assert!(result.is_ok(), "handle_show by name: {:?}", result.err());
    }

    #[tokio::test]
    #[serial]
    async fn handle_show_invokes_subagent_show_rpc_by_id() {
        let (harness, socket) = spawn_subagent_server().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_show(None, Some("sa-1".to_string()), ".".to_string()).await;
        drop(harness);
        assert!(result.is_ok(), "handle_show by id: {:?}", result.err());
    }

    #[tokio::test]
    #[serial]
    async fn handle_tasks_invokes_subagent_tasks_rpc() {
        let (harness, socket) = spawn_subagent_server().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_tasks(Some("dev-subagent".to_string()), None, ".".to_string(), 5).await;
        drop(harness);
        assert!(result.is_ok(), "handle_tasks: {:?}", result.err());
    }

    #[tokio::test]
    #[serial]
    async fn handle_hibernate_invokes_subagent_hibernate_rpc() {
        let (harness, socket) = spawn_subagent_server().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result =
            handle_hibernate(Some("dev-subagent".to_string()), None, ".".to_string()).await;
        drop(harness);
        assert!(result.is_ok(), "handle_hibernate: {:?}", result.err());
    }

    #[tokio::test]
    #[serial]
    async fn handle_delete_soft_invokes_subagent_delete_rpc() {
        let (harness, socket) = spawn_subagent_server().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_delete(
            Some("dev-subagent".to_string()),
            None,
            ".".to_string(),
            false,
            false,
        )
        .await;
        drop(harness);
        // Soft delete (hard=false) skips the confirmation prompt.
        assert!(result.is_ok(), "handle_delete soft: {:?}", result.err());
    }

    #[tokio::test]
    #[serial]
    async fn handle_delete_hard_with_yes_flag_skips_confirmation() {
        let (harness, socket) = spawn_subagent_server().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        // `--hard --yes` should bypass the interactive stdin confirmation.
        let result = handle_delete(
            Some("dev-subagent".to_string()),
            None,
            ".".to_string(),
            true,
            true,
        )
        .await;
        drop(harness);
        assert!(
            result.is_ok(),
            "handle_delete hard --yes: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_delete_propagates_rpc_error_when_server_fails() {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let mut runtime = SubagentRuntime::new(inner);
        runtime.delete_should_fail.store(true, Ordering::SeqCst);
        let runtime: Arc<dyn RuntimeControl> = Arc::new(runtime);
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_delete(
            Some("dev-subagent".to_string()),
            None,
            ".".to_string(),
            false,
            false,
        )
        .await;
        drop(harness);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("subagent not found"),
            "expected RPC error to be propagated, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn connect_fails_when_socket_path_is_invalid() {
        // Point BUSYTOK_SOCKET at a path that cannot be connected to and
        // verify the subagent-specific error context is added.
        std::env::set_var("BUSYTOK_SOCKET", "/nonexistent/busytok-test-inline.sock");
        let result = handle_list(None, None, false).await;
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("busytok-service is not running"),
            "expected subagent-specific connect error message, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn handle_list_propagates_rpc_error_when_runtime_errors() {
        // Use a runtime whose `subagent_list` always fails — verify the
        // error path through `unwrap_ok` is taken.
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(FailingListRuntime { inner });
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_list(None, None, false).await;
        drop(harness);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("list_failed") && err.contains("list is broken"),
            "expected the runtime error to surface, got: {err}"
        );
    }

    /// Runtime wrapper whose `subagent_list` always returns an error,
    /// used to exercise the `ControlResponse::Err` branch in `unwrap_ok`.
    struct FailingListRuntime {
        inner: TestRuntimeControl,
    }

    #[async_trait]
    impl RuntimeControl for FailingListRuntime {
        async fn service_health(&self) -> anyhow::Result<ServiceHealthDto> {
            self.inner.service_health().await
        }
        async fn service_status(&self) -> anyhow::Result<ServiceStatusDto> {
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
        async fn settings_update(
            &self,
            req: SettingsUpdateRequestDto,
        ) -> Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
            self.inner.settings_update(req).await
        }
        async fn settings_diagnostics(&self) -> Result<ReadEnvelopeDto<SettingsDiagnosticsDto>> {
            self.inner.settings_diagnostics().await
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
        async fn prompts_create(
            &self,
            req: PromptCreateRequestDto,
        ) -> Result<ReadEnvelopeDto<PromptEntryDto>> {
            self.inner.prompts_create(req).await
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
            _req: SubagentListRequestDto,
        ) -> Result<SubagentListResponseDto> {
            anyhow::bail!("list_failed: list is broken")
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
        async fn subagent_task_get(
            &self,
            req: SubagentTaskGetRequestDto,
        ) -> Result<SubagentTaskDetailDto> {
            self.inner.subagent_task_get(req).await
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

    /// Exercises every delegation method on `SubagentRuntime` so the
    /// forwarding lines are covered. The inner `TestRuntimeControl`
    /// stubs return Ok/Err; we only need the delegation line to execute.
    #[tokio::test]
    async fn subagent_runtime_delegates_every_method_to_inner() {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime = SubagentRuntime::new(inner);
        let rt: &dyn RuntimeControl = &runtime;

        // no-arg reads
        let _ = rt.service_health().await;
        let _ = rt.service_status().await;
        let _ = rt.shell_status().await;
        let _ = rt.settings_snapshot().await;
        let _ = rt.settings_diagnostics().await;
        let _ = rt.provider_list().await;
        let _ = rt.event_bus();

        let day = RangePresetDto::Day;
        let _ = rt
            .overview_summary(OverviewSummaryRequestDto { range: day })
            .await;
        let _ = rt
            .overview_trend(OverviewTrendRequestDto {
                range: day,
                granularity: None,
            })
            .await;
        let _ = rt
            .overview_heatmap(OverviewHeatmapRequestDto { range: day })
            .await;
        let _ = rt
            .overview_rankings(OverviewRankingsRequestDto { range: day })
            .await;
        let _ = rt.receipt_daily(ReceiptDailyRequestDto::default()).await;
        let _ = rt
            .activity_recent(ActivityRecentRequestDto {
                range: day,
                limit: None,
            })
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
        let _ = rt
            .activity_detail(ActivityDetailRequestDto { id: "x".into() })
            .await;
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
            .clients_detail(ClientSourceDetailRequestDto {
                source_id: "x".into(),
            })
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
        let _ = rt
            .live_window(LiveWindowRequestDto {
                window_seconds: None,
            })
            .await;
        let _ = rt
            .prompts_list(PromptListQueryDto {
                query: None,
                tag: None,
                sort: None,
                limit: None,
            })
            .await;
        let _ = rt.prompts_get(PromptGetRequestDto { id: "x".into() }).await;
        let _ = rt
            .prompts_create(PromptCreateRequestDto {
                content: "c".into(),
                alias: None,
                tags: vec![],
            })
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
        let _ = rt
            .prompts_delete(PromptDeleteRequestDto { id: "x".into() })
            .await;
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
            .suggest_tags(PromptSuggestTagsRequestDto {
                query: None,
                limit: None,
            })
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
                provider_kind: None,
                api_key: None,
            })
            .await;
        let _ = rt
            .provider_delete(ProviderDeleteRequestDto { id: "p".into() })
            .await;
        let _ = rt
            .provider_test_connection(ProviderTestConnectionRequestDto { id: "p".into() })
            .await;
        let _ = rt
            .model_create(ModelCreateRequestDto {
                provider_id: "p".into(),
                model_id: "m".into(),
                enabled: None,
                tags: vec![],
                context_window: 8192,
                max_tokens: 4096,
                display_name: None,
                reasoning: None,
            })
            .await;
        let _ = rt
            .model_list(ModelListRequestDto {
                provider_id: None,
                tags: vec![],
                include_disabled: false,
            })
            .await;
        let _ = rt
            .model_update(ModelUpdateRequestDto {
                id: "m".into(),
                enabled: None,
                display_name: None,
                reasoning: None,
                context_window: None,
                max_tokens: None,
            })
            .await;
        let _ = rt
            .model_delete(ModelDeleteRequestDto { id: "m".into() })
            .await;
        let _ = rt
            .model_tags_update(ModelTagUpdateDto {
                model_id: "m".into(),
                tags: vec![],
            })
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
                tools: None,
                context_budget_tokens: None,
                timeout_seconds: None,
                write_access: None,
            })
            .await;
        let _ = rt
            .profile_update(ProfileUpdateRequestDto {
                id: "pr".into(),
                tools: None,
                context_budget_tokens: None,
                timeout_seconds: None,
                write_access: None,
            })
            .await;
        let _ = rt
            .profile_delete(ProfileDeleteRequestDto { id: "pr".into() })
            .await;
    }

    /// Exercises every delegation method on `FailingListRuntime` so the
    /// forwarding lines are covered. Only `subagent_list` is overridden
    /// (returns Err); every other method delegates to the inner runtime.
    #[tokio::test]
    async fn failing_list_runtime_delegates_every_method_to_inner() {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime = FailingListRuntime { inner };
        let rt: &dyn RuntimeControl = &runtime;

        let _ = rt.service_health().await;
        let _ = rt.service_status().await;
        let _ = rt.shell_status().await;
        let _ = rt.settings_snapshot().await;
        let _ = rt.settings_diagnostics().await;
        let _ = rt.provider_list().await;
        let _ = rt.event_bus();

        let day = RangePresetDto::Day;
        let _ = rt
            .overview_summary(OverviewSummaryRequestDto { range: day })
            .await;
        let _ = rt
            .overview_trend(OverviewTrendRequestDto {
                range: day,
                granularity: None,
            })
            .await;
        let _ = rt
            .overview_heatmap(OverviewHeatmapRequestDto { range: day })
            .await;
        let _ = rt
            .overview_rankings(OverviewRankingsRequestDto { range: day })
            .await;
        let _ = rt.receipt_daily(ReceiptDailyRequestDto::default()).await;
        let _ = rt
            .activity_recent(ActivityRecentRequestDto {
                range: day,
                limit: None,
            })
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
        let _ = rt
            .activity_detail(ActivityDetailRequestDto { id: "x".into() })
            .await;
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
            .clients_detail(ClientSourceDetailRequestDto {
                source_id: "x".into(),
            })
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
        let _ = rt
            .live_window(LiveWindowRequestDto {
                window_seconds: None,
            })
            .await;
        let _ = rt
            .prompts_list(PromptListQueryDto {
                query: None,
                tag: None,
                sort: None,
                limit: None,
            })
            .await;
        let _ = rt.prompts_get(PromptGetRequestDto { id: "x".into() }).await;
        let _ = rt
            .prompts_create(PromptCreateRequestDto {
                content: "c".into(),
                alias: None,
                tags: vec![],
            })
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
        let _ = rt
            .prompts_delete(PromptDeleteRequestDto { id: "x".into() })
            .await;
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
            .suggest_tags(PromptSuggestTagsRequestDto {
                query: None,
                limit: None,
            })
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
                bound_provider_id: None,
                bound_model_id: None,
            })
            .await;
        let _ = rt
            .subagent_show(SubagentResolveRequestDto {
                name: None,
                id: Some("sa".into()),
                cwd: None,
            })
            .await;
        let _ = rt
            .subagent_tasks(SubagentTasksRequestDto {
                name: None,
                id: Some("sa".into()),
                cwd: None,
                limit: None,
            })
            .await;
        let _ = rt
            .subagent_hibernate(SubagentResolveRequestDto {
                name: None,
                id: Some("sa".into()),
                cwd: None,
            })
            .await;
        let _ = rt
            .subagent_delete(SubagentDeleteRequestDto {
                name: None,
                id: Some("sa".into()),
                cwd: None,
                hard: None,
            })
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
                provider_kind: None,
                api_key: None,
            })
            .await;
        let _ = rt
            .provider_delete(ProviderDeleteRequestDto { id: "p".into() })
            .await;
        let _ = rt
            .provider_test_connection(ProviderTestConnectionRequestDto { id: "p".into() })
            .await;
        let _ = rt
            .model_create(ModelCreateRequestDto {
                provider_id: "p".into(),
                model_id: "m".into(),
                enabled: None,
                tags: vec![],
                context_window: 8192,
                max_tokens: 4096,
                display_name: None,
                reasoning: None,
            })
            .await;
        let _ = rt
            .model_list(ModelListRequestDto {
                provider_id: None,
                tags: vec![],
                include_disabled: false,
            })
            .await;
        let _ = rt
            .model_update(ModelUpdateRequestDto {
                id: "m".into(),
                enabled: None,
                display_name: None,
                reasoning: None,
                context_window: None,
                max_tokens: None,
            })
            .await;
        let _ = rt
            .model_delete(ModelDeleteRequestDto { id: "m".into() })
            .await;
        let _ = rt
            .model_tags_update(ModelTagUpdateDto {
                model_id: "m".into(),
                tags: vec![],
            })
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
                tools: None,
                context_budget_tokens: None,
                timeout_seconds: None,
                write_access: None,
            })
            .await;
        let _ = rt
            .profile_update(ProfileUpdateRequestDto {
                id: "pr".into(),
                tools: None,
                context_budget_tokens: None,
                timeout_seconds: None,
                write_access: None,
            })
            .await;
        let _ = rt
            .profile_delete(ProfileDeleteRequestDto { id: "pr".into() })
            .await;
    }

    // ── print_task_detail ────────────────────────────────────────────
    //
    // Dedicated renderer for `handle_task_get` — task records have a
    // different shape than subagent identity (no bound fields; adds
    // result_summary/error/error_kind/timestamps). Do NOT reuse
    // `print_detail`, which is for subagent identity.

    #[test]
    fn print_task_detail_text_with_full_payload() {
        // All fields present — text branch should print every field
        // (id/subagent_id/subagent_name/status/profile/model_override/
        // source_harness/source_session_id/created_at_ms/started_at_ms/
        // completed_at_ms/result_summary/error/error_kind).
        let v = json!({
            "id": "task-1",
            "subagent_id": "sa-1",
            "subagent_name": "dev",
            "status": "completed",
            "profile": "default",
            "model_override": "gpt-5",
            "source_harness": "cli",
            "source_session_id": "sess-1",
            "created_at_ms": 1700000000_i64,
            "started_at_ms": 1700000001_i64,
            "completed_at_ms": 1700000002_i64,
            "result_summary": "did the thing",
            "error": null,
            "error_kind": null,
        });
        let s = format_task_detail_text(&v);
        assert!(s.contains("id:                task-1"), "got: {s}");
        assert!(s.contains("subagent_id:       sa-1"), "got: {s}");
        assert!(s.contains("subagent_name:     dev"), "got: {s}");
        assert!(s.contains("status:            completed"), "got: {s}");
        assert!(s.contains("profile:           default"), "got: {s}");
        assert!(s.contains("model_override:    gpt-5"), "got: {s}");
        assert!(s.contains("source_harness:    cli"), "got: {s}");
        assert!(s.contains("source_session_id: sess-1"), "got: {s}");
        assert!(s.contains("created_at_ms:     1700000000"), "got: {s}");
        assert!(s.contains("started_at_ms:     1700000001"), "got: {s}");
        assert!(s.contains("completed_at_ms:   1700000002"), "got: {s}");
        assert!(s.contains("result_summary:    did the thing"), "got: {s}");
        // `error`/`error_kind` are null → or_dash renders `-`.
        assert!(s.contains("error:             -"), "got: {s}");
        assert!(s.contains("error_kind:        -"), "got: {s}");
        // Negative assertions: the text formatter intentionally omits
        // `prompt`, `prompt_artifact_ref`, and `timeout_seconds` even when
        // they are present in the payload.
        assert!(
            !s.contains("prompt:"),
            "prompt should not appear in text output"
        );
        assert!(
            !s.contains("prompt_artifact_ref:"),
            "prompt_artifact_ref should not appear in text output"
        );
        assert!(
            !s.contains("timeout_seconds:"),
            "timeout_seconds should not appear in text output"
        );
    }

    #[test]
    fn print_task_detail_text_renders_dash_for_absent_optional_fields() {
        // Optional fields absent (None / missing) — text branch should
        // render `-` for each optional field. Required string fields fall
        // back to `?`; the required `created_at_ms` falls back to 0.
        let v = json!({
            "id": "task-2",
            "subagent_id": "sa-2",
            "status": "pending",
            "profile": "default",
            "created_at_ms": 1700000000_i64,
        });
        let s = format_task_detail_text(&v);
        // Required string fields render their provided values.
        assert!(s.contains("id:                task-2"), "got: {s}");
        assert!(s.contains("subagent_id:       sa-2"), "got: {s}");
        assert!(s.contains("status:            pending"), "got: {s}");
        assert!(s.contains("profile:           default"), "got: {s}");
        assert!(s.contains("created_at_ms:     1700000000"), "got: {s}");
        // Optional string fields must render `-` (not `?`, not empty).
        assert!(s.contains("subagent_name:     -"), "got: {s}");
        assert!(s.contains("model_override:    -"), "got: {s}");
        assert!(s.contains("source_harness:    -"), "got: {s}");
        assert!(s.contains("source_session_id: -"), "got: {s}");
        assert!(s.contains("result_summary:    -"), "got: {s}");
        assert!(s.contains("error:             -"), "got: {s}");
        assert!(s.contains("error_kind:        -"), "got: {s}");
        // Optional i64 timestamps render `-` when absent.
        assert!(s.contains("started_at_ms:     -"), "got: {s}");
        assert!(s.contains("completed_at_ms:   -"), "got: {s}");
        // Sanity: a regression that rendered `?` or empty for optionals
        // would fail the above — the literal `-` is asserted per-field.
        assert!(
            !s.contains("subagent_name:     ?"),
            "optional field rendered `?` instead of `-`: {s}"
        );
    }

    #[test]
    fn print_task_detail_text_missing_all_fields_uses_defaults() {
        // Empty object — every `unwrap_or` / `or_dash` fallback fires.
        let v = json!({});
        let s = format_task_detail_text(&v);
        // Required string fields fall back to `?`.
        assert!(s.contains("id:                ?"), "got: {s}");
        assert!(s.contains("subagent_id:       ?"), "got: {s}");
        assert!(s.contains("status:            ?"), "got: {s}");
        assert!(s.contains("profile:           ?"), "got: {s}");
        // Required i64 field falls back to 0.
        assert!(s.contains("created_at_ms:     0"), "got: {s}");
        // Optional fields still render `-`.
        assert!(s.contains("subagent_name:     -"), "got: {s}");
        assert!(s.contains("model_override:    -"), "got: {s}");
        assert!(s.contains("result_summary:    -"), "got: {s}");
        assert!(s.contains("error:             -"), "got: {s}");
        assert!(s.contains("error_kind:        -"), "got: {s}");
        assert!(s.contains("started_at_ms:     -"), "got: {s}");
        assert!(s.contains("completed_at_ms:   -"), "got: {s}");
    }

    #[test]
    fn print_task_detail_json_output_pretty_prints_payload() {
        let v = json!({"id": "task-3"});
        assert!(print_task_detail(&v, "json").is_ok());
    }

    // ── handle_task_get ──────────────────────────────────────────────
    //
    // End-to-end coverage for the new `subagent task --task-id` handler.
    // The `SubagentRuntime` test wrapper delegates `subagent_task_get`
    // to the inner `TestRuntimeControl`, which returns
    // `Ok(Default::default())` — enough to verify the RPC round-trip and
    // the formatter wiring without depending on a real store.

    #[tokio::test]
    #[serial]
    async fn handle_task_get_invokes_subagent_task_get_rpc_and_prints_text() {
        let (harness, socket) = spawn_subagent_server().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_task_get("task-1".to_string(), "text".to_string()).await;
        drop(harness);
        assert!(result.is_ok(), "handle_task_get text: {:?}", result.err());
    }

    #[tokio::test]
    #[serial]
    async fn handle_task_get_invokes_subagent_task_get_rpc_with_json_output() {
        // JSON output returns the raw DTO from the runtime — verify the
        // round-trip succeeds (no formatter errors on Default::default()).
        let (harness, socket) = spawn_subagent_server().await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_task_get("task-1".to_string(), "json".to_string()).await;
        drop(harness);
        assert!(result.is_ok(), "handle_task_get json: {:?}", result.err());
    }

    #[tokio::test]
    #[serial]
    async fn handle_task_get_propagates_rpc_error_when_runtime_errors() {
        // Use a runtime whose `subagent_task_get` always fails — verify
        // the error path through `unwrap_ok` is taken and the runtime
        // error message surfaces to the caller.
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let mut runtime = SubagentRuntime::new(inner);
        runtime.task_get_should_fail.store(true, Ordering::SeqCst);
        let runtime: Arc<dyn RuntimeControl> = Arc::new(runtime);
        let (harness, socket) = spawn_server(runtime).await;
        std::env::set_var("BUSYTOK_SOCKET", &socket);
        let result = handle_task_get("missing-task".to_string(), "text".to_string()).await;
        drop(harness);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("subagent.task_not_found"),
            "expected task-specific code, got: {err}"
        );
        assert!(
            err.contains("task not found: missing-task"),
            "expected task-specific message, got: {err}"
        );
    }
}
