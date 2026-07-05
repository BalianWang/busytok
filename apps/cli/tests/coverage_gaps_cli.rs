#![allow(
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    dead_code,
    unused_imports,
    unused_variables
)]
//! Coverage gap tests for `apps/cli`.
//!
//! Targets uncovered source lines in `commands.rs`, `main.rs`, and `shim.rs`
//! that can be exercised through the CLI binary as a subprocess. Uses
//! in-process `ControlServer` instances backed by custom `RuntimeControl`
//! wrappers to drive RPC-dependent paths.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use async_trait::async_trait;
use busytok_config::BusytokPaths;
use busytok_control::{dispatch::RuntimeControl, server::ControlServer, TestRuntimeControl};
use busytok_protocol::dto::*;
use serial_test::serial;
use tempfile::TempDir;

// ===========================================================================
// Helpers
// ===========================================================================

/// Path to the compiled `busytok` binary under test.
fn bin() -> String {
    env!("CARGO_BIN_EXE_busytok").to_string()
}

/// Spawn `busytok` with the given args (no server), capturing stdout/stderr.
fn run_busytok(args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .env("BUSYTOK_SOCKET", "/nonexistent/busytok-cov-cli.sock")
        .args(args)
        .output()
        .expect("spawn busytok")
}

/// Spawn `busytok` with args and a specific `BUSYTOK_SOCKET` from within an
/// async test context.
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

/// Spawn `busytok prompt create --batch` with stdin lines and a specific
/// `BUSYTOK_SOCKET`.
async fn run_busytok_batch_with_socket_async(
    socket: String,
    stdin_lines: Vec<String>,
) -> std::process::Output {
    let bin = bin();
    tokio::task::spawn_blocking(move || {
        let mut child = Command::new(&bin)
            .env("BUSYTOK_SOCKET", &socket)
            .args(["prompt", "create", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn busytok");
        {
            let mut stdin = child.stdin.take().expect("take stdin");
            for line in &stdin_lines {
                writeln!(stdin, "{}", line).expect("write to stdin");
            }
        }
        child.wait_with_output().expect("wait for child")
    })
    .await
    .expect("spawn_blocking task did not panic")
}

/// Hold a running `ControlServer` for the lifetime of the test.
struct ServerHarness {
    server: Arc<ControlServer>,
    _task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Drop for ServerHarness {
    fn drop(&mut self) {
        self.server.shutdown();
    }
}

/// Spawn a `ControlServer` backed by the given runtime.
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

/// Create a fake Busytok.app bundle structure under `root`.
fn make_fake_bundle(root: &Path, name: &str) -> PathBuf {
    let bundle = root.join(name);
    let macos_dir = bundle.join("Contents/MacOS");
    std::fs::create_dir_all(&macos_dir).unwrap();
    let binary = macos_dir.join("busytok");
    std::fs::write(&binary, "fake binary").unwrap();
    bundle
}

/// Guard that saves the original `busytok-shim` config dir state on creation
/// and restores it on drop.
struct ShimConfigGuard {
    shim_config_dir: PathBuf,
    original_bundle_path: Option<String>,
}

impl ShimConfigGuard {
    fn new() -> Self {
        let config_dir = BusytokPaths::new()
            .config_dir()
            .to_path_buf()
            .join("busytok-shim");
        let bundle_path_file = config_dir.join("app-bundle-path");
        let original_bundle_path = std::fs::read_to_string(&bundle_path_file).ok();
        Self {
            shim_config_dir: config_dir,
            original_bundle_path,
        }
    }
}

impl Drop for ShimConfigGuard {
    fn drop(&mut self) {
        if self.shim_config_dir.exists() {
            let _ = std::fs::remove_dir_all(&self.shim_config_dir);
        }
        if let Some(ref path) = self.original_bundle_path {
            let _ = std::fs::create_dir_all(&self.shim_config_dir);
            let bundle_path_file = self.shim_config_dir.join("app-bundle-path");
            let _ = std::fs::write(&bundle_path_file, path);
        }
    }
}

// ===========================================================================
// ConfigurableRuntime — a single wrapper that can inject errors into
// specific RPC methods.
// ===========================================================================

struct ConfigurableRuntime {
    inner: TestRuntimeControl,
    settings_update_error: Option<String>,
    prompts_create_error: Option<String>,
    /// When true, `prompts_create` panics. This breaks the server-side
    /// connection task, which closes the socket mid-call and causes the
    /// client's `client.call()` to return `Err` — exercising the
    /// transport-error branch of `create_prompt_entry` (commands.rs lines
    /// 587-588) and the reconnect path in `handle_prompt_create_batch`
    /// (line 726).
    panic_prompts_create: bool,
    /// When true, `settings_snapshot` returns a snapshot with the given
    /// manual_roots (instead of the default empty list from the inner fake).
    snapshot_manual_roots: Vec<ManualRootDto>,
}

impl ConfigurableRuntime {
    fn new(inner: TestRuntimeControl) -> Self {
        Self {
            inner,
            settings_update_error: None,
            prompts_create_error: None,
            panic_prompts_create: false,
            snapshot_manual_roots: Vec::new(),
        }
    }

    fn with_settings_update_error(mut self, msg: impl Into<String>) -> Self {
        self.settings_update_error = Some(msg.into());
        self
    }

    fn with_prompts_create_error(mut self, msg: impl Into<String>) -> Self {
        self.prompts_create_error = Some(msg.into());
        self
    }

    fn with_panicking_prompts_create(mut self) -> Self {
        self.panic_prompts_create = true;
        self
    }

    fn with_snapshot_manual_roots(mut self, roots: Vec<ManualRootDto>) -> Self {
        self.snapshot_manual_roots = roots;
        self
    }
}

#[async_trait]
impl RuntimeControl for ConfigurableRuntime {
    async fn settings_update(
        &self,
        req: SettingsUpdateRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<SettingsSnapshotDto>> {
        if let Some(ref err) = self.settings_update_error {
            anyhow::bail!("{}", err);
        }
        self.inner.settings_update(req).await
    }

    async fn prompts_create(
        &self,
        req: PromptCreateRequestDto,
    ) -> anyhow::Result<ReadEnvelopeDto<PromptEntryDto>> {
        if self.panic_prompts_create {
            // Panicking inside the dispatch future unwinds the connection
            // handler task, dropping the server-side reader/writer halves and
            // closing the socket. The client's `read_frame` then fails, which
            // surfaces as a transport-level Err from `client.call()`.
            panic!("simulated transport failure in prompts_create");
        }
        if let Some(ref err) = self.prompts_create_error {
            anyhow::bail!("{}", err);
        }
        self.inner.prompts_create(req).await
    }

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
        // When the test configures non-empty manual_roots, return a snapshot
        // that includes them — so the merge logic in `handle_settings_update`
        // has prior roots to read.
        if !self.snapshot_manual_roots.is_empty() {
            let mut env = self.inner.settings_snapshot().await?;
            env.data.discovery.manual_roots = self.snapshot_manual_roots.clone();
            return Ok(env);
        }
        self.inner.settings_snapshot().await
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
    async fn subagent_task_get(
        &self,
        req: SubagentTaskGetRequestDto,
    ) -> anyhow::Result<SubagentTaskDetailDto> {
        self.inner.subagent_task_get(req).await
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
    async fn model_create(
        &self,
        req: ModelCreateRequestDto,
    ) -> anyhow::Result<ModelCatalogEntryDto> {
        self.inner.model_create(req).await
    }
    async fn model_list(&self, req: ModelListRequestDto) -> anyhow::Result<ModelListResponseDto> {
        self.inner.model_list(req).await
    }
    async fn model_update(&self, req: ModelUpdateRequestDto) -> anyhow::Result<()> {
        self.inner.model_update(req).await
    }
    async fn model_delete(&self, req: ModelDeleteRequestDto) -> anyhow::Result<()> {
        self.inner.model_delete(req).await
    }
    async fn model_tags_update(&self, req: ModelTagUpdateDto) -> anyhow::Result<()> {
        self.inner.model_tags_update(req).await
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
    fn event_bus(&self) -> &busytok_events::AppEventBus {
        self.inner.event_bus()
    }
}

/// Spawn a server backed by a `ConfigurableRuntime`.
async fn spawn_configurable_server(runtime: ConfigurableRuntime) -> (ServerHarness, String) {
    let runtime: Arc<dyn RuntimeControl> = Arc::new(runtime);
    spawn_server(runtime).await
}

// ===========================================================================
// commands.rs coverage
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sources_rescan_with_source_id_calls_rpc() {
    // Line 104: non-dry-run path with source_id constructs params with "source_id".
    let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let runtime: Arc<dyn RuntimeControl> = Arc::new(inner);
    let (harness, socket) = spawn_server(runtime).await;

    let output = run_busytok_with_socket_async(
        socket,
        vec![
            "sources".to_string(),
            "rescan".to_string(),
            "src-1".to_string(),
        ],
    )
    .await;
    drop(harness);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "should exit non-zero: {stderr}");
    assert!(
        stderr.contains("RPC error") || stderr.contains("method_not_found"),
        "should surface RPC error: {stderr}"
    );
}

#[test]
fn usage_export_dry_run_normalizes_non_claude_code_agent() {
    // Line 266: `other => other` — a non-"claude-code" agent (e.g. "codex")
    // passes through unchanged in the normalize step.
    let output = run_busytok(&[
        "usage",
        "export",
        "--kind",
        "events",
        "--format",
        "csv",
        "--agent",
        "codex",
        "--dry-run",
    ]);
    assert!(
        output.status.success(),
        "dry-run should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("usage.export"),
        "should print method: {stdout}"
    );
    assert!(
        stdout.contains("codex"),
        "should include codex agent: {stdout}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usage_export_non_dry_run_calls_rpc() {
    // Lines 281, 283: non-dry-run path calls rpc_call("usage.export", params).
    let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let runtime: Arc<dyn RuntimeControl> = Arc::new(inner);
    let (harness, socket) = spawn_server(runtime).await;

    let output = run_busytok_with_socket_async(
        socket,
        vec![
            "usage".to_string(),
            "export".to_string(),
            "--kind".to_string(),
            "events".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    )
    .await;
    drop(harness);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "should exit non-zero: {stderr}");
    assert!(
        stderr.contains("RPC error") || stderr.contains("method_not_found"),
        "should surface RPC error: {stderr}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_update_with_add_root_calls_rpc() {
    // Line 410: manual_roots.push(...) in the add_root branch.
    let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let runtime: Arc<dyn RuntimeControl> = Arc::new(inner);
    let (harness, socket) = spawn_server(runtime).await;

    let output = run_busytok_with_socket_async(
        socket,
        vec![
            "settings".to_string(),
            "update".to_string(),
            "--add-root".to_string(),
            "claude-code:/some/test/path".to_string(),
        ],
    )
    .await;
    drop(harness);

    assert!(
        output.status.success(),
        "settings update with add-root should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_update_with_discovery_defaults_propagates_validation_error() {
    // Lines 457-458: ControlResponse::Err in the discovery_defaults merge path.
    let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let runtime = ConfigurableRuntime::new(inner).with_settings_update_error(
        "SETTINGS_VALIDATION_FAILED: {\"errors\":[{\"code\":\"invalid_timezone\",\"field\":\"timezone\"}]}",
    );
    let (harness, socket) = spawn_configurable_server(runtime).await;

    let output = run_busytok_with_socket_async(
        socket,
        vec![
            "settings".to_string(),
            "update".to_string(),
            "--discovery-default".to_string(),
            "claude-code:true".to_string(),
        ],
    )
    .await;
    drop(harness);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "should exit non-zero: {stderr}");
    assert!(
        stderr.contains("RPC error"),
        "should surface RPC error: {stderr}"
    );
    assert!(
        stderr.contains("settings_validation_failed"),
        "should include error code: {stderr}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_batch_skips_empty_lines() {
    // Line 653: `continue` when a line is empty after trimming.
    let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let runtime: Arc<dyn RuntimeControl> = Arc::new(inner);
    let (harness, socket) = spawn_server(runtime).await;

    let output = run_busytok_batch_with_socket_async(
        socket,
        vec![
            "".to_string(),
            r#"{"content":"hello world","alias":"greeting"}"#.to_string(),
        ],
    )
    .await;
    drop(harness);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "batch with empty + valid should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("created"),
        "should have a created entry: {stdout}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_batch_emits_rpc_error_on_failing_create() {
    // Line 719: reason = "rpc_error" when error doesn't contain alias conflict.
    let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let runtime = ConfigurableRuntime::new(inner)
        .with_prompts_create_error("create_failed: prompts.create is broken");
    let (harness, socket) = spawn_configurable_server(runtime).await;

    let output =
        run_busytok_batch_with_socket_async(socket, vec![r#"{"content":"hello"}"#.to_string()])
            .await;
    drop(harness);

    assert!(
        !output.status.success(),
        "should exit non-zero when entries skipped: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("skipped"),
        "should emit skipped entry: {stdout}"
    );
    assert!(
        stdout.contains("rpc_error"),
        "should use rpc_error reason: {stdout}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_batch_succeeds_with_all_valid() {
    // Lines 734-735: Ok(()) at end of batch when no entries were skipped.
    let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let runtime: Arc<dyn RuntimeControl> = Arc::new(inner);
    let (harness, socket) = spawn_server(runtime).await;

    let output = run_busytok_batch_with_socket_async(
        socket,
        vec![
            r#"{"content":"first prompt","alias":"first"}"#.to_string(),
            r#"{"content":"second prompt","alias":"second"}"#.to_string(),
        ],
    )
    .await;
    drop(harness);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "batch with all valid should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let created_count = stdout.matches("created").count();
    assert!(
        created_count >= 2,
        "should have >=2 created entries, got {created_count}: {stdout}"
    );
    assert!(
        !stdout.contains("skipped"),
        "should not have skipped: {stdout}"
    );
}

// ===========================================================================
// commands.rs coverage — transport-error & reconnect paths
//
// `create_prompt_entry` (commands.rs lines 587-588) has a transport-error arm
// that fires when `client.call()` returns `Err` (i.e. the underlying socket
// breaks mid-call). The only practical way to trigger this from an external
// test is to make the server-side dispatch panic, which unwinds the
// connection handler task, drops the reader/writer halves, and closes the
// socket. The client's `read_frame` then fails, surfacing as a transport
// `Err`.
//
// That transport error message also contains the string "transport error",
// which `handle_prompt_create_batch` detects to trigger its one-shot
// reconnect retry (line 726).
// ===========================================================================

/// Parse newline-delimited JSON from a subprocess stdout into a Vec<Value>.
fn parse_jsonl_stdout(stdout: &[u8]) -> Vec<serde_json::Value> {
    let s = String::from_utf8_lossy(stdout);
    s.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSON line"))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_batch_emits_transport_error_when_connection_drops() {
    // Lines 587-588 in `create_prompt_entry`: the `Err(e) =>` arm is only
    // reached when `client.call()` itself fails at the transport layer
    // (not when the server returns an RPC error). We trigger this by having
    // the runtime panic inside `prompts_create`, which closes the socket
    // before the server can write a response. The batch's reconnect retry
    // (line 726) is also exercised because the resulting error string
    // contains "transport error".
    let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let runtime = ConfigurableRuntime::new(inner).with_panicking_prompts_create();
    let (harness, socket) = spawn_configurable_server(runtime).await;

    // Suppress the default panic hook so the simulated server-side panic
    // doesn't spam the test runner's stderr. We restore it afterwards.
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        run_busytok_batch_with_socket_async(socket, vec![r#"{"content":"hello"}"#.to_string()]),
    )
    .await
    .expect("batch subprocess should complete within 20s");

    std::panic::set_hook(prev_hook);
    drop(harness);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "batch should exit non-zero when transport error occurs: {stderr}"
    );

    let lines = parse_jsonl_stdout(&output.stdout);
    let skipped = lines
        .iter()
        .find(|v| v.get("status").and_then(|s| s.as_str()) == Some("skipped"))
        .expect("should emit at least one skipped entry");
    assert_eq!(
        skipped.get("reason").and_then(|r| r.as_str()),
        Some("rpc_error"),
        "transport-error entries are reported as rpc_error: {stdout}"
    );
    let detail = skipped
        .get("detail")
        .and_then(|d| d.as_str())
        .expect("skipped entry should have detail");
    assert!(
        detail.contains("transport error"),
        "detail should mention transport error, got: {detail}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_batch_reconnects_when_rpc_error_mentions_transport_error() {
    // Line 726: when `create_prompt_entry` returns an Err whose message
    // contains "transport error" (even if it was actually an RPC error),
    // the batch loop attempts a reconnect for the next entry. We trigger
    // this with a runtime that bails from `prompts_create` with a message
    // containing "transport error".
    let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let runtime = ConfigurableRuntime::new(inner)
        .with_prompts_create_error("transport error: simulated mid-call drop");
    let (harness, socket) = spawn_configurable_server(runtime).await;

    let output = run_busytok_batch_with_socket_async(
        socket,
        vec![
            r#"{"content":"first"}"#.to_string(),
            r#"{"content":"second"}"#.to_string(),
        ],
    )
    .await;
    drop(harness);

    assert!(
        !output.status.success(),
        "batch should exit non-zero when entries skipped: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let lines = parse_jsonl_stdout(&output.stdout);
    // Both entries should be skipped (the runtime always errors), and the
    // reconnect retry at line 726 should fire between them.
    assert!(
        lines
            .iter()
            .all(|v| v.get("status").and_then(|s| s.as_str()) == Some("skipped")),
        "both entries should be skipped: {:?}",
        lines
    );
    // Verify the detail carries "transport error" so we know the
    // `contains("transport error")` branch was satisfied.
    for entry in &lines {
        let detail = entry
            .get("detail")
            .and_then(|d| d.as_str())
            .expect("skipped entry should have detail");
        assert!(
            detail.contains("transport error"),
            "detail should contain 'transport error': {detail}"
        );
    }
}

// ===========================================================================
// commands.rs coverage — settings.update manual_roots merge path
//
// `handle_settings_update` reads the current settings.snapshot to merge
// existing manual_roots with the new ones being added (lines 417-440). The
// default TestRuntimeControl returns an empty manual_roots list, so the
// merge loop body is never entered. This test injects pre-existing roots via
// ConfigurableRuntime so the merge code has prior roots to iterate over.
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_update_add_root_with_existing_manual_roots() {
    // Lines 417-440: when settings.snapshot returns existing manual_roots,
    // the merge loop should iterate them and preserve non-duplicate entries.
    let inner = TestRuntimeControl::with_claude_fixture().await.unwrap();
    let existing_root = ManualRootDto {
        id: String::new(),
        client_id: "claude_code".to_string(),
        root_path: "/existing/path".to_string(),
        source_type: SourceTypeDto::ManualRoot,
    };
    let runtime = ConfigurableRuntime::new(inner).with_snapshot_manual_roots(vec![existing_root]);
    let (harness, socket) = spawn_configurable_server(runtime).await;

    let output = run_busytok_with_socket_async(
        socket,
        vec![
            "settings".to_string(),
            "update".to_string(),
            "--add-root".to_string(),
            "claude-code:/new/path".to_string(),
        ],
    )
    .await;
    drop(harness);

    assert!(
        output.status.success(),
        "settings update should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The response is a serialized ReadEnvelopeDto<SettingsSnapshotDto>.
    // Parse it to inspect manual_roots.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let resp: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("settings update stdout is JSON");
    let manual_roots = resp
        .get("data")
        .and_then(|d| d.get("discovery"))
        .and_then(|d| d.get("manual_roots"))
        .and_then(|r| r.as_array())
        .expect("response should contain data.discovery.manual_roots");
    let paths: Vec<&str> = manual_roots
        .iter()
        .filter_map(|r| r.get("root_path").and_then(|p| p.as_str()))
        .collect();
    assert!(
        paths.contains(&"/new/path"),
        "new root should be present: {paths:?}"
    );
}

// ===========================================================================
// main.rs coverage — Settings::Update arm (lines 517-528)
// ===========================================================================

#[test]
fn settings_update_arm_runs_and_surfaces_connect_error() {
    // Lines 517-528: Settings::Update arm of run() in main.rs.
    let output = run_busytok(&["settings", "update", "--timezone", "UTC"]);
    assert!(
        !output.status.success(),
        "should exit non-zero when no server: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connecting to Busytok service") || stderr.contains("Open Busytok.app"),
        "should surface connect error: {stderr}"
    );
}

// ===========================================================================
// main.rs + shim.rs coverage — Cli::{Install, Status, Uninstall} arms
// ===========================================================================

#[test]
#[serial]
fn cli_install_creates_shim_and_reports_valid_status() {
    // Lines 532-557 (main.rs Cli::Install), 91-92 (shim tracing), 116 (valid).
    let _guard = ShimConfigGuard::new();

    let tmp = TempDir::new().unwrap();
    let bundle = make_fake_bundle(tmp.path(), "Busytok.app");
    let bin_dir = tmp.path().join("bin");

    let output = run_busytok(&[
        "cli",
        "install",
        "--bin-dir",
        bin_dir.to_str().unwrap(),
        "--app-bundle-path",
        bundle.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "cli install should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(bin_dir.join("busytok").is_file(), "shim should exist");

    let output = run_busytok(&["cli", "status", "--bin-dir", bin_dir.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "cli status should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("valid"), "should report valid: {stdout}");

    let output = run_busytok(&["cli", "uninstall", "--bin-dir", bin_dir.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "cli uninstall should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !bin_dir.join("busytok").exists(),
        "shim removed after uninstall"
    );
}

#[test]
#[serial]
fn cli_uninstall_reports_success() {
    // Lines 565-569 (main.rs Cli::Uninstall), 148 (shim tracing).
    let _guard = ShimConfigGuard::new();

    let tmp = TempDir::new().unwrap();
    let bundle = make_fake_bundle(tmp.path(), "Busytok.app");
    let bin_dir = tmp.path().join("bin");

    run_busytok(&[
        "cli",
        "install",
        "--bin-dir",
        bin_dir.to_str().unwrap(),
        "--app-bundle-path",
        bundle.to_str().unwrap(),
    ]);

    let output = run_busytok(&["cli", "uninstall", "--bin-dir", bin_dir.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "cli uninstall should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The uninstall message is printed to stderr.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("uninstalled"),
        "should report uninstall: {stderr}"
    );
    assert!(!bin_dir.join("busytok").exists(), "shim removed");
}

#[test]
#[serial]
fn cli_status_reports_stale_bundle() {
    // Lines 118-122 (shim.rs): stale bundle when binary is missing.
    let _guard = ShimConfigGuard::new();

    let tmp = TempDir::new().unwrap();
    let bundle = make_fake_bundle(tmp.path(), "Busytok.app");
    let bin_dir = tmp.path().join("bin");

    run_busytok(&[
        "cli",
        "install",
        "--bin-dir",
        bin_dir.to_str().unwrap(),
        "--app-bundle-path",
        bundle.to_str().unwrap(),
    ]);

    // Delete the binary inside the bundle to make it stale.
    let binary = bundle.join("Contents/MacOS/busytok");
    std::fs::remove_file(&binary).unwrap();

    let output = run_busytok(&["cli", "status", "--bin-dir", bin_dir.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "cli status should succeed with stale bundle: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("stale"), "should report stale: {stdout}");

    run_busytok(&["cli", "uninstall", "--bin-dir", bin_dir.to_str().unwrap()]);
}

#[test]
#[serial]
fn cli_status_reports_not_recorded_when_bundle_path_missing() {
    // Lines 124-126 (shim.rs): "not recorded" when bundle path file is missing.
    let _guard = ShimConfigGuard::new();

    let tmp = TempDir::new().unwrap();
    let bundle = make_fake_bundle(tmp.path(), "Busytok.app");
    let bin_dir = tmp.path().join("bin");

    run_busytok(&[
        "cli",
        "install",
        "--bin-dir",
        bin_dir.to_str().unwrap(),
        "--app-bundle-path",
        bundle.to_str().unwrap(),
    ]);

    // Remove the bundle path file from the config dir.
    let config_dir = BusytokPaths::new()
        .config_dir()
        .to_path_buf()
        .join("busytok-shim");
    let bundle_path_file = config_dir.join("app-bundle-path");
    assert!(
        bundle_path_file.exists(),
        "bundle path file should exist after install"
    );
    std::fs::remove_file(&bundle_path_file).unwrap();

    let output = run_busytok(&["cli", "status", "--bin-dir", bin_dir.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "cli status should succeed without recorded path: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("not recorded"),
        "should report not recorded: {stdout}"
    );

    run_busytok(&["cli", "uninstall", "--bin-dir", bin_dir.to_str().unwrap()]);
}

#[test]
#[serial]
fn cli_status_reports_not_installed_when_shim_missing() {
    // Lines 559-563 (main.rs Cli::Status) + shim.rs bail when shim missing.
    let _guard = ShimConfigGuard::new();

    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("nonexistent_bin");

    let output = run_busytok(&["cli", "status", "--bin-dir", bin_dir.to_str().unwrap()]);
    assert!(
        !output.status.success(),
        "should exit non-zero when shim not installed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not installed"),
        "should report not installed: {stderr}"
    );
}
