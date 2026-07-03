#![allow(
    clippy::await_holding_lock,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::field_reassign_with_default,
    clippy::uninlined_format_args,
    clippy::inconsistent_digit_grouping,
    clippy::len_zero,
    clippy::identity_op,
    clippy::useless_vec,
    clippy::manual_dangling_ptr,
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::absurd_extreme_comparisons,
    unused_variables,
    unused_imports,
    dead_code
)]
//! Integration tests for code paths that are awkward to exercise via inline
//! `#[cfg(test)]` modules:
//!
//! - `busytok doctor` exit-code behavior (the `std::process::exit(1)` branch
//!   cannot be tested inline because it would kill the test runner).
//! - `busytok subagent delete --hard` confirmation prompt (reads from stdin;
//!   cleaner to drive via subprocess stdin piping).
//! - End-to-end smoke tests of subagent / usage / sources CLI commands that
//!   build RPC requests and (when no server is available) surface a friendly
//!   connect error — these exercise the `run()` dispatch in `main.rs` and the
//!   argument-to-RPC marshalling in `commands*.rs`.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use busytok_control::{dispatch::RuntimeControl, server::ControlServer, TestRuntimeControl};
use busytok_protocol::dto::*;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Path to the compiled `busytok` binary under test.
fn bin() -> String {
    env!("CARGO_BIN_EXE_busytok").to_string()
}

/// Spawn `busytok` with the given args, capturing stdout/stderr.
fn run_busytok(args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .env("BUSYTOK_SOCKET", "/nonexistent/busytok-coverage-gaps.sock")
        .args(args)
        .output()
        .expect("spawn busytok")
}

/// Spawn `busytok` with args and feed the given lines to stdin.
fn run_busytok_with_stdin(args: &[&str], stdin_lines: &[&str]) -> std::process::Output {
    let mut child = Command::new(bin())
        .env("BUSYTOK_SOCKET", "/nonexistent/busytok-coverage-gaps.sock")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn busytok");
    {
        let mut stdin = child.stdin.take().expect("take stdin");
        for line in stdin_lines {
            writeln!(stdin, "{}", line).expect("write to stdin");
        }
    }
    child.wait_with_output().expect("wait for child")
}

// ---------------------------------------------------------------------------
// doctor end-to-end (covers `std::process::exit(1)` branch)
// ---------------------------------------------------------------------------

/// Runtime wrapper that returns a configurable `SettingsDiagnosticsDto`
/// from `settings_diagnostics()`, so we can drive every `handle_doctor`
/// branch end-to-end through the real CLI binary.
struct DoctorRuntime {
    inner: TestRuntimeControl,
    diagnostics: Mutex<SettingsDiagnosticsDto>,
}

impl DoctorRuntime {
    fn new(inner: TestRuntimeControl, diagnostics: SettingsDiagnosticsDto) -> Self {
        Self {
            inner,
            diagnostics: Mutex::new(diagnostics),
        }
    }
}

#[async_trait]
impl RuntimeControl for DoctorRuntime {
    async fn settings_diagnostics(
        &self,
    ) -> anyhow::Result<ReadEnvelopeDto<SettingsDiagnosticsDto>> {
        Ok(ReadEnvelopeDto {
            data: self.diagnostics.lock().unwrap().clone(),
            generated_at_ms: 0,
            generation_id: None,
            readiness: ReadinessStateDto::ReadyExact,
            is_exact: true,
            is_stale: false,
            watermark_ms: None,
            progress: None,
            degraded_reason: None,
        })
    }
    // Everything else delegates to the inner fake.
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
    async fn receipt_daily(
        &self,
        req: ReceiptDailyRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<ReceiptDailyDto>> {
        self.inner.receipt_daily(req).await
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
        self.inner.prompts_create(req).await
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
    async fn suggest_tags(
        &self,
        req: PromptSuggestTagsRequestDto,
    ) -> anyhow::Result<PromptSuggestTagsResponseDto> {
        self.inner.suggest_tags(req).await
    }
    async fn subagent_delegate(
        &self,
        req: SubagentDelegateRequestDto,
    ) -> anyhow::Result<SubagentDelegateResponseDto> {
        self.inner.subagent_delegate(req).await
    }
    async fn subagent_list(
        &self,
        req: SubagentListRequestDto,
    ) -> anyhow::Result<SubagentListResponseDto> {
        self.inner.subagent_list(req).await
    }
    async fn subagent_show(
        &self,
        req: SubagentResolveRequestDto,
    ) -> anyhow::Result<SubagentDetailDto> {
        self.inner.subagent_show(req).await
    }
    async fn subagent_tasks(
        &self,
        req: SubagentTasksRequestDto,
    ) -> anyhow::Result<SubagentTasksResponseDto> {
        self.inner.subagent_tasks(req).await
    }
    async fn subagent_hibernate(
        &self,
        req: SubagentResolveRequestDto,
    ) -> anyhow::Result<SubagentAckDto> {
        self.inner.subagent_hibernate(req).await
    }
    async fn subagent_delete(
        &self,
        req: SubagentDeleteRequestDto,
    ) -> anyhow::Result<SubagentAckDto> {
        self.inner.subagent_delete(req).await
    }
    async fn subagent_runtime_status(
        &self,
        req: SubagentRuntimeStatusRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<SubagentRuntimeStatusDto>> {
        self.inner.subagent_runtime_status(req).await
    }
    async fn provider_create(&self, req: ProviderCreateRequestDto) -> anyhow::Result<ProviderDto> {
        self.inner.provider_create(req).await
    }
    async fn provider_list(&self) -> anyhow::Result<ProviderListResponseDto> {
        self.inner.provider_list().await
    }
    async fn provider_update(&self, req: ProviderUpdateRequestDto) -> anyhow::Result<ProviderDto> {
        self.inner.provider_update(req).await
    }
    async fn provider_delete(&self, req: ProviderDeleteRequestDto) -> anyhow::Result<()> {
        self.inner.provider_delete(req).await
    }
    async fn provider_test_connection(
        &self,
        req: ProviderTestConnectionRequestDto,
    ) -> anyhow::Result<ProviderTestConnectionResponseDto> {
        self.inner.provider_test_connection(req).await
    }
    async fn pi_sidecar_locator_update(
        &self,
        req: PiSidecarLocatorUpdateRequestDto,
    ) -> anyhow::Result<PiSidecarLocatorUpdateResponseDto> {
        self.inner.pi_sidecar_locator_update(req).await
    }
    async fn profile_create(&self, req: ProfileCreateRequestDto) -> anyhow::Result<ProfileDto> {
        self.inner.profile_create(req).await
    }
    async fn profile_update(&self, req: ProfileUpdateRequestDto) -> anyhow::Result<ProfileDto> {
        self.inner.profile_update(req).await
    }
    async fn profile_delete(&self, req: ProfileDeleteRequestDto) -> anyhow::Result<()> {
        self.inner.profile_delete(req).await
    }
}

/// Build a `SettingsDiagnosticsDto` with sane defaults for the non-subagent
/// fields and the given subagent section.
fn base_diagnostics(subagent: Option<SubagentDoctorResultDto>) -> SettingsDiagnosticsDto {
    SettingsDiagnosticsDto {
        db_healthy: true,
        db_size_bytes: 0,
        migration_version: 0,
        usage_event_count: 0,
        last_log_checkpoint_ms: None,
        writer_queue_depth: 0,
        aggregate_lag_ms: 0,
        recent_diagnostics: vec![],
        subagent,
    }
}

/// A diagnostics DTO with no subagent section (feature disabled).
fn diagnostics_no_subagent() -> SettingsDiagnosticsDto {
    base_diagnostics(None)
}

/// A diagnostics DTO with a subagent section where all checks pass.
fn diagnostics_all_ok() -> SettingsDiagnosticsDto {
    base_diagnostics(Some(SubagentDoctorResultDto {
        checks: vec![DoctorCheckDto {
            name: "sidecar_launchable".to_string(),
            status: "ok".to_string(),
            detail: Some("ok".to_string()),
        }],
        overall_ok: true,
    }))
}

/// A diagnostics DTO with a subagent section where one check fails.
fn diagnostics_with_failure() -> SettingsDiagnosticsDto {
    base_diagnostics(Some(SubagentDoctorResultDto {
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
        overall_ok: false,
    }))
}

/// Spawn a `ControlServer` backed by a `DoctorRuntime` returning the given
/// diagnostics, returning `(socket_path, server_task_handle, server)`.
struct ServerHarness {
    server: Arc<ControlServer>,
    _task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Drop for ServerHarness {
    fn drop(&mut self) {
        self.server.shutdown();
    }
}

async fn spawn_doctor_server(diagnostics: SettingsDiagnosticsDto) -> (ServerHarness, String) {
    let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let runtime: Arc<dyn RuntimeControl> = Arc::new(DoctorRuntime::new(inner, diagnostics));
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

/// Run `busytok` with args and a specific `BUSYTOK_SOCKET` from within an
/// async test context. Uses `spawn_blocking` so the blocking subprocess wait
/// doesn't pin the tokio runtime (which would deadlock the in-process
/// `ControlServer` that must concurrently serve the subprocess's RPC).
async fn run_busytok_with_socket_async(socket: String, args: Vec<String>) -> std::process::Output {
    let bin = bin();
    tokio::task::spawn_blocking(move || {
        Command::new(&bin)
            .env("BUSYTOK_SOCKET", &socket)
            .args(&args)
            .output()
            .expect("spawn busytok")
    })
    .await
    .expect("spawn_blocking task did not panic")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_exits_nonzero_when_a_check_fails() {
    // The inline test module can't exercise the `std::process::exit(1)`
    // branch — it would kill the test runner. Here we drive the real binary
    // as a subprocess so its exit code is observable.
    let (harness, socket) = spawn_doctor_server(diagnostics_with_failure()).await;
    let output = run_busytok_with_socket_async(socket, vec!["doctor".to_string()]).await;
    drop(harness);

    assert!(
        !output.status.success(),
        "doctor should exit non-zero when a check fails"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("subagent doctor: one or more checks failed"),
        "stdout should mention failure: {stdout}"
    );
    // All three statuses should be rendered with their respective symbols.
    assert!(stdout.contains('✓'), "should render an ok check: {stdout}");
    assert!(
        stdout.contains('⚠'),
        "should render a warning check: {stdout}"
    );
    assert!(
        stdout.contains('✗'),
        "should render a failed check: {stdout}"
    );
    // The check without a detail should still print its name only.
    assert!(
        stdout.contains("stale_subagents"),
        "should print check name: {stdout}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_exits_zero_when_all_checks_pass() {
    let (harness, socket) = spawn_doctor_server(diagnostics_all_ok()).await;
    let output = run_busytok_with_socket_async(socket, vec!["doctor".to_string()]).await;
    drop(harness);

    assert!(
        output.status.success(),
        "doctor should exit zero when overall_ok=true"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("all checks passed"),
        "stdout should report success: {stdout}"
    );
    assert!(stdout.contains('✓'), "should render an ok check: {stdout}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_exits_zero_when_subagent_disabled() {
    let (harness, socket) = spawn_doctor_server(diagnostics_no_subagent()).await;
    let output = run_busytok_with_socket_async(socket, vec!["doctor".to_string()]).await;
    drop(harness);

    assert!(
        output.status.success(),
        "doctor should exit zero when subagent is disabled"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("subagent feature disabled"),
        "stdout should report disabled: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// subagent delete --hard confirmation prompt (stdin "y" / "yes" / "n")
// ---------------------------------------------------------------------------

#[test]
fn subagent_hard_delete_aborts_when_stdin_is_no() {
    // No server is running, so even if the user confirms, the RPC call
    // will fail later — but we only care that the confirmation gate works:
    // "n" should bail *before* attempting to connect.
    let output = run_busytok_with_stdin(&["subagent", "delete", "--hard", "my-agent"], &["n"]);
    assert!(
        !output.status.success(),
        "should exit non-zero when user aborts"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("aborted"),
        "should report aborted: {stderr}"
    );
}

#[test]
fn subagent_hard_delete_aborts_when_stdin_is_empty() {
    // An empty line (just Enter) is neither "y" nor "yes" → abort.
    let output = run_busytok_with_stdin(&["subagent", "delete", "--hard", "my-agent"], &[""]);
    assert!(
        !output.status.success(),
        "should exit non-zero on empty stdin"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("aborted"),
        "should report aborted: {stderr}"
    );
}

#[test]
fn subagent_hard_delete_aborts_when_stdin_is_garbage() {
    let output = run_busytok_with_stdin(&["subagent", "delete", "--hard", "my-agent"], &["maybe"]);
    assert!(
        !output.status.success(),
        "should exit non-zero on garbage stdin"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("aborted"),
        "should report aborted: {stderr}"
    );
}

#[test]
fn subagent_hard_delete_proceeds_past_confirmation_with_y() {
    // "y" should pass the confirmation gate. Since no server is running,
    // the subsequent connect_client() will fail — we verify the gate was
    // passed by checking that the error is about connecting (not "aborted").
    let output = run_busytok_with_stdin(&["subagent", "delete", "--hard", "my-agent"], &["y"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("aborted"),
        "should NOT abort on 'y': {stderr}"
    );
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should reach the connect step: {stderr}"
    );
}

#[test]
fn subagent_hard_delete_proceeds_past_confirmation_with_yes() {
    // Same as above but with "yes" (also a valid confirmation).
    let output = run_busytok_with_stdin(&["subagent", "delete", "--hard", "my-agent"], &["yes"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("aborted"),
        "should NOT abort on 'yes': {stderr}"
    );
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should reach the connect step: {stderr}"
    );
}

#[test]
fn subagent_hard_delete_accepts_uppercase_yes() {
    // The confirmation logic trims and lowercases the input.
    let output = run_busytok_with_stdin(&["subagent", "delete", "--hard", "my-agent"], &["YES"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("aborted"),
        "should NOT abort on 'YES' (case-insensitive): {stderr}"
    );
}

#[test]
fn subagent_hard_delete_with_yes_flag_skips_confirmation() {
    // `--yes` skips the prompt entirely; no stdin needed.
    let output = run_busytok(&["subagent", "delete", "--hard", "--yes", "my-agent"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("aborted"),
        "should NOT abort with --yes: {stderr}"
    );
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should reach the connect step: {stderr}"
    );
}

#[test]
fn subagent_soft_delete_skips_confirmation() {
    // Without --hard, no confirmation prompt is shown; goes straight to connect.
    let output = run_busytok(&["subagent", "delete", "my-agent"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("aborted"),
        "soft delete should never abort: {stderr}"
    );
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should reach the connect step: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// subagent / usage / sources CLI smoke tests (exercise run() dispatch + arg
// marshalling; surface friendly connect error when no server is running)
// ---------------------------------------------------------------------------

#[test]
fn delegate_smoke_surfaces_connect_error() {
    let output = run_busytok(&[
        "delegate",
        "--subagent",
        "worker",
        "--profile",
        "default",
        "do the thing",
    ]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn delegate_with_json_output_flag_parses() {
    let output = run_busytok(&[
        "delegate",
        "--subagent",
        "worker",
        "--profile",
        "default",
        "--output",
        "json",
        "do the thing",
    ]);
    // Should parse the --output json flag without error and reach connect.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("error: invalid value"),
        "should accept --output json: {stderr}"
    );
}

#[test]
fn subagent_list_smoke_surfaces_connect_error() {
    let output = run_busytok(&["subagent", "list"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn subagent_list_with_filters_parses() {
    let output = run_busytok(&[
        "subagent",
        "list",
        "--status",
        "hot",
        "--project",
        "myproj",
        "--include-deleted",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("error: unexpected argument") && !stderr.contains("error: invalid value"),
        "should accept the filters: {stderr}"
    );
}

#[test]
fn subagent_show_by_name_smoke_surfaces_connect_error() {
    let output = run_busytok(&["subagent", "show", "my-agent"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn subagent_show_by_id_smoke_surfaces_connect_error() {
    let output = run_busytok(&["subagent", "show", "--id", "abc-123"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn subagent_show_requires_name_or_id() {
    // clap should reject `subagent show` with neither name nor id.
    let output = run_busytok(&["subagent", "show"]);
    assert!(
        !output.status.success(),
        "should exit non-zero when neither name nor id is given"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("required") || stderr.contains("USAGE") || stderr.contains("Usage"),
        "should report a required-argument error: {stderr}"
    );
}

#[test]
fn subagent_show_rejects_name_and_id_together() {
    // name and --id are mutually exclusive (conflicts_with).
    let output = run_busytok(&["subagent", "show", "my-agent", "--id", "abc-123"]);
    assert!(
        !output.status.success(),
        "should exit non-zero when both name and id are given"
    );
}

#[test]
fn subagent_tasks_smoke_surfaces_connect_error() {
    let output = run_busytok(&["subagent", "tasks", "my-agent"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn subagent_tasks_with_limit_parses() {
    let output = run_busytok(&["subagent", "tasks", "my-agent", "--limit", "50"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("error: invalid value"),
        "should accept --limit: {stderr}"
    );
}

#[test]
fn subagent_hibernate_smoke_surfaces_connect_error() {
    let output = run_busytok(&["subagent", "hibernate", "my-agent"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// sources / usage / diagnostics / settings smoke tests
// ---------------------------------------------------------------------------

#[test]
fn sources_list_smoke_surfaces_connect_error() {
    let output = run_busytok(&["sources", "list"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn sources_status_smoke_surfaces_connect_error() {
    let output = run_busytok(&["sources", "status", "some-source-id"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn sources_rescan_dry_run_does_not_connect() {
    // --dry-run short-circuits before connect_client, so it should exit 0
    // even though no server is running.
    let output = run_busytok(&["sources", "rescan", "--dry-run"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("sources.rescan"),
        "dry-run should print the method name: {stdout}"
    );
}

#[test]
fn sources_rescan_dry_run_with_source_id_includes_id() {
    let output = run_busytok(&["sources", "rescan", "--dry-run", "src-123"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("sources.rescan"),
        "dry-run should print the method name: {stdout}"
    );
    assert!(
        stdout.contains("src-123"),
        "dry-run should include the source_id in params: {stdout}"
    );
}

#[test]
fn usage_summary_smoke_surfaces_connect_error() {
    let output = run_busytok(&["usage", "summary"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn usage_timeline_smoke_surfaces_connect_error() {
    let output = run_busytok(&["usage", "timeline"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn usage_timeline_with_filters_parses() {
    let output = run_busytok(&[
        "usage",
        "timeline",
        "--since",
        "2026-01-01",
        "--until",
        "2026-02-01",
        "--agent",
        "claude-code",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("error: unexpected argument") && !stderr.contains("error: invalid value"),
        "should accept the filters: {stderr}"
    );
}

#[test]
fn usage_events_smoke_surfaces_connect_error() {
    let output = run_busytok(&["usage", "events"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn usage_events_with_cursor_and_limit_parses() {
    let output = run_busytok(&["usage", "events", "--cursor", "abc", "--limit", "10"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("error: unexpected argument") && !stderr.contains("error: invalid value"),
        "should accept cursor/limit: {stderr}"
    );
}

#[test]
fn usage_projects_smoke_surfaces_connect_error() {
    let output = run_busytok(&["usage", "projects"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn usage_models_smoke_surfaces_connect_error() {
    let output = run_busytok(&["usage", "models"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn usage_sessions_smoke_surfaces_connect_error() {
    let output = run_busytok(&["usage", "sessions"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn usage_export_dry_run_does_not_connect() {
    let output = run_busytok(&[
        "usage",
        "export",
        "--kind",
        "events",
        "--format",
        "csv",
        "--dry-run",
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("usage.export"),
        "dry-run should print the method name: {stdout}"
    );
    assert!(
        stdout.contains("events"),
        "dry-run should include the kind: {stdout}"
    );
    assert!(
        stdout.contains("csv"),
        "dry-run should include the format: {stdout}"
    );
}

#[test]
fn usage_export_dry_run_normalizes_claude_code_agent() {
    let output = run_busytok(&[
        "usage",
        "export",
        "--kind",
        "events",
        "--format",
        "csv",
        "--agent",
        "claude-code",
        "--dry-run",
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("claude_code"),
        "dry-run should normalize claude-code to claude_code: {stdout}"
    );
}

#[test]
fn diagnostics_scan_status_smoke_surfaces_connect_error() {
    let output = run_busytok(&["diagnostics", "scan-status"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn diagnostics_store_health_smoke_surfaces_connect_error() {
    let output = run_busytok(&["diagnostics", "store-health"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn settings_snapshot_smoke_surfaces_connect_error() {
    let output = run_busytok(&["settings", "snapshot"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn scan_online_smoke_delegates_to_sources_rescan() {
    // `busytok scan` (without --offline) delegates to sources.rescan via RPC.
    // With no server, it should surface the connect error.
    let output = run_busytok(&["scan"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

#[test]
fn scan_offline_requires_path_flag() {
    // `busytok scan --offline` without --path should fail client-side.
    let output = run_busytok(&["scan", "--offline"]);
    assert!(
        !output.status.success(),
        "should exit non-zero without --path"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--path") || stderr.contains("path is required"),
        "should mention --path requirement: {stderr}"
    );
}

#[test]
fn scan_offline_errors_when_path_does_not_exist() {
    let output = run_busytok(&[
        "scan",
        "--offline",
        "--agent",
        "claude-code",
        "--path",
        "/nonexistent/path/abc/xyz",
    ]);
    assert!(
        !output.status.success(),
        "should exit non-zero for nonexistent path"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("path does not exist"),
        "should report missing path: {stderr}"
    );
}

#[test]
fn status_smoke_surfaces_connect_error() {
    let output = run_busytok(&["status"]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface friendly connect error: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// default-command behavior (no subcommand → status)
// ---------------------------------------------------------------------------

#[test]
fn no_subcommand_defaults_to_status_and_surfaces_connect_error() {
    // `busytok` with no subcommand defaults to Status (see main.rs run()).
    let output = run_busytok(&[]);
    assert!(
        !output.status.success(),
        "should exit non-zero because no server is reachable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "default (status) should surface friendly connect error: {stderr}"
    );
}
