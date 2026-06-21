use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use busytok_control::{dispatch::RuntimeControl, server::ControlServer, TestRuntimeControl};
use busytok_protocol::dto::*;
use serde_json::Value;

/// Run busytok with args, capturing output.
fn run_busytok(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_busytok"))
        .env("BUSYTOK_SOCKET", "/nonexistent/busytok-test.sock")
        .args(args)
        .output()
        .unwrap()
}

/// Run `busytok prompt create --batch` with the given stdin lines.
/// Returns (exit_success, parsed_stdout_lines, stderr).
fn run_busytok_batch(stdin_lines: &[&str]) -> (bool, Vec<Value>, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_busytok"))
        .env("BUSYTOK_SOCKET", "/nonexistent/busytok-test.sock")
        .args(["prompt", "create", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    {
        let mut stdin = child.stdin.take().unwrap();
        for line in stdin_lines {
            writeln!(stdin, "{}", line).unwrap();
        }
    }

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let parsed: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    (output.status.success(), parsed, stderr)
}

// ── help / usage tests ──────────────────────────────────────────────

#[test]
fn prompt_help_shows_subcommands() {
    let output = run_busytok(&["prompt", "--help"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("create"),
        "prompt help should mention 'create'"
    );
}

#[test]
fn prompt_create_help_shows_options() {
    let output = run_busytok(&["prompt", "create", "--help"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--content"), "should mention --content");
    assert!(stdout.contains("--alias"), "should mention --alias");
    assert!(stdout.contains("--tags"), "should mention --tags");
    assert!(stdout.contains("--batch"), "should mention --batch");
}

#[test]
fn batch_help_shows_options() {
    let output = run_busytok(&["prompt", "create", "--help"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--batch"),
        "create help should mention --batch"
    );
    // The batch help text should reference stdin.
    let lower = stdout.to_lowercase();
    assert!(
        lower.contains("stdin") || lower.contains("jsonl"),
        "batch help should mention stdin or JSONL: {stdout}"
    );
}

#[test]
fn help_mentions_prompt() {
    let output = run_busytok(&["--help"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("prompt"),
        "top-level help should mention 'prompt'"
    );
}

#[test]
fn prompt_create_without_flags_exits_nonzero() {
    let output = Command::new(env!("CARGO_BIN_EXE_busytok"))
        .args(["prompt", "create"])
        .output()
        .unwrap();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("--content") || stderr.contains("USAGE") || stderr.contains("Usage"),
            "missing --content should mention the flag: {stderr}"
        );
    }
}

// ── single-mode client-side validation ──────────────────────────────

#[test]
fn prompt_create_validates_empty_content() {
    // Client-side validation rejects empty content before any RPC call.
    let output = Command::new(env!("CARGO_BIN_EXE_busytok"))
        .args(["prompt", "create", "--content", ""])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "empty content should produce an error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The exact message says "prompt content must not be empty".
    assert!(
        stderr.contains("empty") || stderr.contains("content"),
        "should report empty content error: {stderr}"
    );
}

#[test]
fn prompt_create_validates_alias() {
    // Client-side validation rejects alias with forbidden characters
    // (whitespace, quotes, backticks) before any RPC call.
    let output = Command::new(env!("CARGO_BIN_EXE_busytok"))
        .args([
            "prompt",
            "create",
            "--content",
            "hello",
            "--alias",
            "has space",
        ])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "invalid alias should produce an error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("whitespace") || stderr.contains("alias"),
        "should reject alias with whitespace: {stderr}"
    );
}

// ── batch-mode JSONL pre-validation ─────────────────────────────────
// BUSYTOK_SOCKET points to a nonexistent path, so every line that passes
// pre-validation will be skipped with "rpc_error" when connect_client fails.
// These tests verify the pre-validation pipeline and JSONL output format.

#[test]
fn batch_rejects_invalid_json() {
    // Feeding non-JSON to stdin should produce a "skipped" line with
    // reason "invalid_json".
    let (success, lines, _stderr) = run_busytok_batch(&["not json"]);
    assert!(
        !success,
        "batch should exit non-zero when an entry is skipped"
    );
    assert_eq!(lines.len(), 1, "one input line => one output line");
    let out = &lines[0];
    assert_eq!(out["status"], "skipped");
    assert_eq!(out["index"], 0);
    assert_eq!(out["reason"], "invalid_json");
    let detail = out["detail"].as_str().unwrap();
    assert!(
        detail.contains("expected"),
        "invalid_json detail should mention parse error: {detail}"
    );
}

#[test]
fn batch_rejects_empty_content() {
    // {"content":""} passes JSON parse but fails content pre-validation.
    let (success, lines, _stderr) = run_busytok_batch(&[r#"{"content":""}"#]);
    assert!(
        !success,
        "batch should exit non-zero when an entry is skipped"
    );
    assert_eq!(lines.len(), 1);
    let out = &lines[0];
    assert_eq!(out["status"], "skipped");
    assert_eq!(out["reason"], "empty_content");
    assert!(
        out["detail"]
            .as_str()
            .unwrap()
            .contains("must not be empty"),
        "empty_content detail should explain the constraint"
    );
}

#[test]
fn batch_rejects_invalid_alias() {
    // {"content":"test","alias":"has space"} — alias contains whitespace.
    let (success, lines, _stderr) =
        run_busytok_batch(&[r#"{"content":"test","alias":"has space"}"#]);
    assert!(
        !success,
        "batch should exit non-zero when an entry is skipped"
    );
    assert_eq!(lines.len(), 1);
    let out = &lines[0];
    assert_eq!(out["status"], "skipped");
    assert_eq!(out["reason"], "invalid_alias");
    assert!(
        out["detail"].as_str().unwrap().contains("whitespace"),
        "invalid_alias detail should mention forbidden characters"
    );
}

#[test]
fn batch_exits_nonzero_on_skipped() {
    // Two invalid lines → both skipped → exit non-zero.
    let (success, lines, _stderr) = run_busytok_batch(&[
        r#"{"content":""}"#,
        r#"{"content":"ok","alias":"bad alias"}"#,
    ]);
    assert!(!success, "batch with skips should exit non-zero");
    assert_eq!(lines.len(), 2, "two input lines => two output lines");
    assert_eq!(lines[0]["reason"], "empty_content");
    assert_eq!(lines[1]["reason"], "invalid_alias");
    // Indices should match input position.
    assert_eq!(lines[0]["index"], 0);
    assert_eq!(lines[1]["index"], 1);
}

#[test]
fn batch_line_rpc_error_on_connect_failure() {
    // A valid line passes pre-validation but connect_client fails because
    // no daemon is running. It should be emitted as skipped with
    // "rpc_error".
    let (success, lines, _stderr) =
        run_busytok_batch(&[r#"{"content":""}"#, r#"{"content":"valid payload"}"#]);
    // First line skipped for empty_content, second triggers lazy connect
    // that fails → rpc_error.
    assert!(!success);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["status"], "skipped");
    assert_eq!(lines[0]["reason"], "empty_content");

    // The second line passes pre-validation, so connect_client() is called
    // and fails, producing an rpc_error skip.
    assert_eq!(lines[1]["status"], "skipped");
    assert_eq!(lines[1]["reason"], "rpc_error");
    assert_eq!(lines[1]["index"], 1);
    let detail = lines[1]["detail"].as_str().unwrap();
    assert!(
        detail.contains("connect") || detail.contains("is it running"),
        "rpc_error detail should mention connection failure: {detail}"
    );
}

// ── batch-mode alias_conflict (server-side) ─────────────────────────

/// Wrapper around `TestRuntimeControl` that succeeds on the first
/// `prompts_create` call and returns an alias-conflict error on subsequent
/// calls. All other methods delegate to the inner fake.
struct AliasConflictRuntime {
    inner: TestRuntimeControl,
    first_create: AtomicBool,
}

#[async_trait]
impl RuntimeControl for AliasConflictRuntime {
    async fn service_health(&self) -> anyhow::Result<ServiceHealthDto> {
        self.inner.service_health().await
    }
    async fn service_status(&self) -> anyhow::Result<ServiceStatusDto> {
        self.inner.service_status().await
    }
    async fn shell_status(&self) -> anyhow::Result<ShellStatusDto> {
        self.inner.shell_status().await
    }
    async fn overview_summary(
        &self,
        req: OverviewSummaryRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewSummaryDto>> {
        self.inner.overview_summary(req).await
    }
    async fn overview_trend(
        &self,
        req: OverviewTrendRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewTrendResponseDto>> {
        self.inner.overview_trend(req).await
    }
    async fn overview_heatmap(
        &self,
        req: OverviewHeatmapRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewHeatmapResponseDto>> {
        self.inner.overview_heatmap(req).await
    }
    async fn overview_rankings(
        &self,
        req: OverviewRankingsRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<OverviewRankingsResponseDto>> {
        self.inner.overview_rankings(req).await
    }
    async fn activity_recent(
        &self,
        req: ActivityRecentRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ActivityRecentResponseDto>> {
        self.inner.activity_recent(req).await
    }
    async fn activity_list(
        &self,
        req: ActivityListRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ActivityListResponseDto>> {
        self.inner.activity_list(req).await
    }
    async fn activity_detail(
        &self,
        req: ActivityDetailRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ActivityDetailDto>> {
        self.inner.activity_detail(req).await
    }
    async fn breakdown_list(
        &self,
        req: BreakdownListRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<BreakdownListResponseDto>> {
        self.inner.breakdown_list(req).await
    }
    async fn breakdown_detail(
        &self,
        req: BreakdownDetailRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<BreakdownDetailDto>> {
        self.inner.breakdown_detail(req).await
    }
    async fn clients_snapshot(
        &self,
        req: ClientsSnapshotRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ClientsSnapshotDto>> {
        self.inner.clients_snapshot(req).await
    }
    async fn clients_detail(
        &self,
        req: ClientSourceDetailRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ClientSourceDetailDto>> {
        self.inner.clients_detail(req).await
    }
    async fn settings_snapshot(&self) -> anyhow::Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
        self.inner.settings_snapshot().await
    }
    async fn settings_update(
        &self,
        req: SettingsUpdateRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
        self.inner.settings_update(req).await
    }
    async fn settings_diagnostics(
        &self,
    ) -> anyhow::Result<ReadEnvelopeDto<SettingsDiagnosticsDto>> {
        self.inner.settings_diagnostics().await
    }
    async fn settings_recovery_action(
        &self,
        req: SettingsRecoveryActionRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<SettingsRecoveryActionResponseDto>> {
        self.inner.settings_recovery_action(req).await
    }
    async fn live_window(
        &self,
        req: LiveWindowRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<LiveWindowDto>> {
        self.inner.live_window(req).await
    }
    async fn prompts_list(
        &self,
        req: PromptListQueryDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptListResponseDto>> {
        self.inner.prompts_list(req).await
    }
    async fn prompts_get(
        &self,
        req: PromptGetRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
        self.inner.prompts_get(req).await
    }
    async fn prompts_create(
        &self,
        req: PromptCreateRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
        if self.first_create.swap(false, Ordering::SeqCst) {
            self.inner.prompts_create(req).await
        } else {
            Err(anyhow::anyhow!("an alias with this name already exists"))
        }
    }
    async fn prompts_update(
        &self,
        req: PromptUpdateRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
        self.inner.prompts_update(req).await
    }
    async fn prompts_delete(
        &self,
        req: PromptDeleteRequestDto,
    ) -> anyhow::Result<PromptDeleteResultDto> {
        self.inner.prompts_delete(req).await
    }
    async fn prompts_use(&self, req: PromptUseRequestDto) -> anyhow::Result<PromptUseResultDto> {
        self.inner.prompts_use(req).await
    }
    fn event_bus(&self) -> &busytok_events::AppEventBus {
        self.inner.event_bus()
    }
}

#[tokio::test]
async fn batch_emits_alias_conflict_on_duplicate() {
    let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let runtime = Arc::new(AliasConflictRuntime {
        inner,
        first_create: AtomicBool::new(true),
    });
    let (server, socket_path) = ControlServer::spawn_for_test(runtime).await.unwrap();
    let server = Arc::new(server);
    let server_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };

    // Feed two lines with the same alias via the CLI subprocess.
    let mut child = Command::new(env!("CARGO_BIN_EXE_busytok"))
        .env("BUSYTOK_SOCKET", &socket_path)
        .args(["prompt", "create", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    {
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, r#"{{"content":"first prompt","alias":"dup"}}"#).unwrap();
        writeln!(stdin, r#"{{"content":"second prompt","alias":"dup"}}"#).unwrap();
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking(move || child.wait_with_output()),
    )
    .await
    .expect("subprocess should exit within 10s")
    .unwrap()
    .unwrap();
    server.shutdown();
    server.await_drain().await;
    let _ = server_task.await;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    assert!(
        !output.status.success(),
        "batch with skips should exit non-zero"
    );
    assert_eq!(lines.len(), 2, "two input lines => two output lines");

    // First line succeeds.
    assert_eq!(lines[0]["status"], "created");
    assert_eq!(lines[0]["alias"], "dup");

    // Second line is skipped with alias_conflict.
    assert_eq!(lines[1]["status"], "skipped");
    assert_eq!(lines[1]["reason"], "alias_conflict");
    assert_eq!(lines[1]["index"], 1);
    assert_eq!(lines[1]["alias"], "dup");
    let detail = lines[1]["detail"].as_str().unwrap();
    assert!(
        detail.contains("alias with this name already exists"),
        "alias_conflict detail should contain the server error: {detail}"
    );
}
