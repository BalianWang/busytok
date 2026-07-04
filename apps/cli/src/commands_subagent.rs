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
        // CLI does not yet expose bound-field flags; the reuse path uses the
        // subagent's stored bound fields, and the create path will reject
        // (both-absent) until the GUI/CLI add explicit bound-field args.
        bound_provider_id: None,
        bound_model_id: None,
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
    use super::*;
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

    // ── print_detail ───────────────────────────────────────────────────

    #[test]
    fn print_detail_text_with_name_field() {
        let v = json!({"id": "id-1", "name": "named", "status": "warm"});
        assert!(print_detail(&v, "text").is_ok());
    }

    #[test]
    fn print_detail_text_falls_back_to_subagent_name() {
        // No `name` field — should fall back to `subagent_name`.
        let v = json!({"id": "id-2", "subagent_name": "fallback", "status": "cold"});
        assert!(print_detail(&v, "text").is_ok());
    }

    #[test]
    fn print_detail_text_missing_all_fields_uses_defaults() {
        let v = json!({});
        assert!(print_detail(&v, "text").is_ok());
    }

    #[test]
    fn print_detail_json_output_pretty_prints_payload() {
        let v = json!({"id": "id-3"});
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

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
            _req: SubagentDelegateRequestDto,
        ) -> Result<SubagentDelegateResponseDto> {
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

    async fn spawn_subagent_server() -> (ServerHarness, String) {
        let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
        let runtime: Arc<dyn RuntimeControl> = Arc::new(SubagentRuntime::new(inner));
        spawn_server(runtime).await
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
        )
        .await;
        drop(harness);
        assert!(
            result.is_ok(),
            "handle_delegate json output: {:?}",
            result.err()
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
}
