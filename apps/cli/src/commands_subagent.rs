//! Handlers for `busytok delegate` and `busytok subagent …`.

use std::io::BufRead;

use anyhow::{bail, Context, Result};
use busytok_control::ControlClient;
use busytok_protocol::dto::{
    SubagentDelegateRequestDto, SubagentDeleteRequestDto, SubagentListRequestDto,
    SubagentResolveRequestDto, SubagentTasksRequestDto,
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
) -> Result<()> {
    // Do NOT canonicalize cwd — the service resolver canonicalizes at one chokepoint.
    let mut client = connect().await?;
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
    };
    let resp = client
        .call(ControlRequest::new(
            "subagent.delegate",
            serde_json::to_value(&req)?,
        ))
        .await
        .context("subagent.delegate RPC failed")?;
    let data = unwrap_ok(resp)?;
    print_delegate(&data, &output)
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
    print_array(&data, "subagents", "text")
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

/// Extract the Ok payload or bail with the control error message.
fn unwrap_ok(resp: ControlResponse) -> Result<serde_json::Value> {
    match resp {
        ControlResponse::Ok(v) => Ok(v),
        ControlResponse::Err(e) => bail!("{}: {}", e.code, e.message),
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

/// Print a single subagent detail object.
fn print_detail(value: &serde_json::Value, output: &str) -> Result<()> {
    print_json_or(value, output, |v| {
        let id = v.get("id").and_then(|s| s.as_str()).unwrap_or("?");
        let name = v
            .get("name")
            .and_then(|s| s.as_str())
            .or_else(|| v.get("subagent_name").and_then(|s| s.as_str()))
            .unwrap_or("?");
        let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("?");
        println!("id:     {id}");
        println!("name:   {name}");
        println!("status: {status}");
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

/// Shared helper: `json` output prints pretty JSON; any other value runs the
/// supplied text-mode closure. Centralizes the `match output { "json" => …, _ => … }`
/// pattern previously repeated across `print_delegate`/`print_array`/`print_detail`/`print_ack`.
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
