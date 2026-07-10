#![allow(warnings, clippy::all)]
#![allow(unexpected_cfgs)]
#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::all))]
use clap::{Parser, Subcommand};

use anyhow::Context;
use busytok_config::{init_logging, BusytokPaths, LogSource};
use tracing::{error, info};

mod commands;
mod commands_subagent;
mod shim;

#[derive(Debug, Parser)]
#[command(name = "busytok", about = "Local-first agent usage audit")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Write structured JSON logs to this directory (overrides BUSYTOK_LOG_DIR).
    #[arg(long, env = "BUSYTOK_LOG_DIR")]
    log_dir: Option<std::path::PathBuf>,

    /// Enable verbose logging (info level). Useful for debugging.
    /// Without this flag, the CLI is quiet by default (warn level).
    #[arg(short = 'v', long)]
    verbose: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show service health
    Status,

    /// Scan log sources
    Scan {
        /// Run offline without a service (direct local scan)
        #[arg(long)]
        offline: bool,

        /// Agent type to scan (e.g. claude-code)
        #[arg(long)]
        agent: Option<String>,

        /// Path to scan
        #[arg(long)]
        path: Option<String>,
    },

    /// Manage log sources
    Sources {
        #[command(subcommand)]
        subcommand: SourcesCommand,
    },

    /// Usage statistics and events
    Usage {
        #[command(subcommand)]
        subcommand: UsageCommand,
    },

    /// Diagnostic information
    Diagnostics {
        #[command(subcommand)]
        subcommand: DiagnosticsCommand,
    },

    /// Settings management
    Settings {
        #[command(subcommand)]
        subcommand: SettingsCommand,
    },

    /// Manage prompt palette entries
    Prompt {
        #[command(subcommand)]
        subcommand: PromptCommand,
    },

    /// Manage CLI shim installation
    Cli {
        #[command(subcommand)]
        subcommand: CliCommand,
    },

    /// Delegate a task to a (possibly new) subagent
    Delegate {
        #[arg(long)]
        subagent: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long, default_value = ".")]
        cwd: String,
        #[arg(long)]
        profile: String,
        #[arg(long)]
        intent: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        timeout: Option<u64>,
        #[arg(long, default_value = "text", value_parser = ["json", "text"])]
        output: String,
        /// Provider ID to bind a new subagent to (required with --bind-model for new subagents)
        #[arg(long)]
        bind_provider: Option<String>,
        /// Model ID to bind a new subagent to (required with --bind-provider for new subagents)
        #[arg(long)]
        bind_model: Option<String>,
        /// Wait for the task to reach a terminal state (completed/failed/cancelled)
        /// before returning. Polls `subagent.task_get` every 2s. Without
        /// this flag, a `queued` result is printed immediately.
        #[arg(long)]
        wait: bool,
        /// Read the prompt from a file instead of the positional argument.
        /// Mutually exclusive with <prompt>, --stdin, and --artifact-ref.
        #[arg(long)]
        prompt_file: Option<String>,
        /// Read the prompt from stdin. Mutually exclusive with <prompt>,
        /// --prompt-file, and --artifact-ref.
        #[arg(long = "stdin")]
        stdin_prompt: bool,
        /// Reference to a stored artifact to use as the prompt. Mutually
        /// exclusive with <prompt>, --prompt-file, and --stdin.
        #[arg(long)]
        artifact_ref: Option<String>,
        /// Client-side deadline for --wait, in seconds. If the task has not
        /// reached a terminal state by this time, the last-known task JSON
        /// is printed and the process exits with code 124.
        #[arg(long)]
        wait_timeout: Option<u64>,
        /// Interval between --wait polls, in seconds (default: 2).
        #[arg(long)]
        poll_interval: Option<u64>,
        /// Binding reuse policy when a subagent with the same name already
        /// exists: "create" (always create new), "reuse" (reuse existing),
        /// "fail" (fail on conflict). Defaults to "fail" when --bind-* flags
        /// are given and the existing binding differs.
        #[arg(long, value_parser = ["create", "reuse", "fail"])]
        reuse_policy: Option<String>,
        /// The task prompt (positional). Mutually exclusive with --prompt-file,
        /// --stdin, and --artifact-ref. Exactly one of the four must be given.
        prompt: Option<String>,
    },

    /// Inspect and manage subagents
    Subagent {
        #[command(subcommand)]
        subcommand: SubagentCommand,
    },

    /// Run doctor health checks (spec §855, §1068: `busytok doctor`).
    /// Calls the existing `settings.diagnostics` RPC and pretty-prints
    /// the subagent section. No new RPC method.
    Doctor,

    /// List models in the catalog
    Models {
        /// Filter by provider id
        #[arg(long)]
        provider: Option<String>,
        /// Filter by tag (repeatable, AND semantics)
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Include disabled models and disabled-provider models
        #[arg(long)]
        all: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Sort key: "name" (default, by provider then model), "context_window_desc", "max_tokens_desc"
        #[arg(long, value_parser = ["name", "context_window_desc", "max_tokens_desc"])]
        sort: Option<String>,
        /// Filter to reasoning-capable models only
        #[arg(long)]
        reasoning: bool,
    },

    /// Manage providers and their models
    Provider {
        #[command(subcommand)]
        subcommand: ProviderCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ProviderCommand {
    /// List all providers
    List {
        #[arg(long)]
        json: bool,
    },
    /// Create a new provider
    Add {
        #[arg(long)]
        url: String,
        #[arg(long)]
        key: String,
        #[arg(long, default_value = "openai_compatible", value_parser = ["openai_compatible", "anthropic_compatible"])]
        kind: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        tags: Option<String>,
    },
    /// Show provider details
    Show { id: String },
    /// Update a provider
    Update {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        key: Option<String>,
        #[arg(long, value_parser = ["openai_compatible", "anthropic_compatible"])]
        kind: Option<String>,
        #[arg(long)]
        enabled: Option<bool>,
    },
    /// Delete a provider (cascades to models; may break bound subagents)
    Delete {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Test connection to a provider
    Test { id: String },
    /// Manage models under a provider
    Model {
        #[command(subcommand)]
        subcommand: ProviderModelCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ProviderModelCommand {
    /// List models for a provider (includes disabled models)
    List {
        provider_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Add a model to a provider
    Add {
        provider_id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long)]
        context_window: Option<i64>,
        #[arg(long)]
        max_tokens: Option<i64>,
        #[arg(long, default_value = "true")]
        reasoning: bool,
        #[arg(long)]
        display_name: Option<String>,
    },
    /// Update a model
    Update {
        provider_id: String,
        model_id: String,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long)]
        context_window: Option<i64>,
        #[arg(long)]
        max_tokens: Option<i64>,
        #[arg(long)]
        reasoning: Option<bool>,
        #[arg(long)]
        enabled: Option<bool>,
        #[arg(long)]
        display_name: Option<String>,
    },
    /// Delete a model (may break bound subagents)
    Delete {
        provider_id: String,
        model_id: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
enum SubagentCommand {
    /// List known subagents
    List {
        /// "hot" | "warm" | "cold"
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        include_deleted: bool,
        #[arg(long, default_value = "text", value_parser = ["json", "text"])]
        output: String,
    },

    /// Resolve by <name> (within --cwd) or by --id <uuid>.
    Show {
        /// Subagent name (within --cwd). Required unless --id is given.
        #[arg(required_unless_present = "id", conflicts_with = "id")]
        name: Option<String>,
        /// Subagent UUID. Mutually exclusive with <name>.
        #[arg(long, conflicts_with = "name")]
        id: Option<String>,
        #[arg(long, default_value = ".")]
        cwd: String,
        #[arg(long, default_value = "text", value_parser = ["json", "text"])]
        output: String,
    },

    /// List recent tasks for a subagent
    Tasks {
        #[arg(required_unless_present = "id", conflicts_with = "id")]
        name: Option<String>,
        #[arg(long, conflicts_with = "name")]
        id: Option<String>,
        #[arg(long, default_value = ".")]
        cwd: String,
        #[arg(long, default_value_t = 20)]
        limit: i64,
        #[arg(long, default_value = "text", value_parser = ["json", "text"])]
        output: String,
    },

    /// Hibernate a subagent (move to cold tier)
    Hibernate {
        #[arg(required_unless_present = "id", conflicts_with = "id")]
        name: Option<String>,
        #[arg(long, conflicts_with = "name")]
        id: Option<String>,
        #[arg(long, default_value = ".")]
        cwd: String,
        #[arg(long, default_value = "text", value_parser = ["json", "text"])]
        output: String,
    },

    /// Delete a subagent (use --hard for permanent removal)
    Delete {
        #[arg(required_unless_present = "id", conflicts_with = "id")]
        name: Option<String>,
        #[arg(long, conflicts_with = "name")]
        id: Option<String>,
        #[arg(long, default_value = ".")]
        cwd: String,
        #[arg(long)]
        hard: bool,
        #[arg(long)]
        yes: bool,
        #[arg(long, default_value = "text", value_parser = ["json", "text"])]
        output: String,
    },

    /// Look up a single task by its task_id
    Task {
        #[arg(long)]
        task_id: String,
        #[arg(long, default_value = "text", value_parser = ["json", "text"])]
        output: String,
    },

    /// Cancel a task by its task_id (queued → cancelled; running → cooperative cancel)
    Cancel {
        #[arg(long)]
        task_id: String,
        /// Optional human-readable reason for the cancel (recorded in the task log)
        #[arg(long)]
        reason: Option<String>,
        #[arg(long, default_value = "text", value_parser = ["json", "text"])]
        output: String,
    },
}

#[derive(Debug, Subcommand)]
enum SourcesCommand {
    /// List discovered log sources
    List,

    /// Show status of a specific source
    Status {
        /// Source ID
        id: String,
    },

    /// Trigger rescan of sources
    Rescan {
        /// Specific source ID to rescan (omit for all)
        source_id: Option<String>,

        /// Show what would be called without making the RPC
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
enum UsageCommand {
    /// Usage dashboard summary
    Summary,

    /// Usage over time
    Timeline {
        /// Start date (ISO 8601)
        #[arg(long)]
        since: Option<String>,

        /// End date (ISO 8601)
        #[arg(long)]
        until: Option<String>,

        /// Filter by agent
        #[arg(long)]
        agent: Option<String>,
    },

    /// List usage events
    Events {
        /// Pagination cursor
        #[arg(long)]
        cursor: Option<String>,

        /// Maximum number of events to return
        #[arg(long)]
        limit: Option<u32>,
    },

    /// Project summaries
    Projects,

    /// Model summaries
    Models,

    /// Session summaries
    Sessions,

    /// Export usage data
    Export {
        /// Kind of data to export (events, timeline, models)
        #[arg(long)]
        kind: String,

        /// Output format (json, csv)
        #[arg(long)]
        format: String,

        /// Filter by agent
        #[arg(long)]
        agent: Option<String>,

        /// Show what would be called without making the RPC
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
enum SettingsCommand {
    /// Get current settings
    Snapshot,

    /// Update settings
    Update {
        /// Timezone offset (e.g. +08:00, UTC)
        #[arg(long)]
        timezone: Option<String>,

        /// Discovery default toggle (agent:bool, e.g. claude-code:true)
        #[arg(long = "discovery-default", value_parser = parse_discovery_default)]
        discovery_default: Vec<(String, bool)>,

        /// Add a manual log root (agent:path, e.g. claude-code:/path/to/logs)
        #[arg(long = "add-root", value_parser = parse_add_root)]
        add_root: Vec<(String, String)>,
    },
}

#[derive(Debug, Subcommand)]
enum DiagnosticsCommand {
    /// Scan status
    ScanStatus,

    /// Store health check
    StoreHealth,
}

#[derive(Debug, Subcommand)]
enum PromptCommand {
    /// Create one or more prompt entries
    Create {
        #[arg(long, required_unless_present = "batch")]
        content: Option<String>,
        #[arg(long)]
        alias: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, conflicts_with_all = ["content", "alias", "tags"])]
        /// Read JSONL entries from stdin
        batch: bool,
    },
}

/// Parse `agent:bool` strings like `claude-code:true`.
fn parse_discovery_default(s: &str) -> Result<(String, bool), String> {
    let (agent, val) = s
        .split_once(':')
        .ok_or_else(|| format!("expected agent:bool, got '{s}'"))?;
    let enabled = val
        .parse::<bool>()
        .map_err(|_| format!("expected true or false after colon, got '{val}'"))?;
    Ok((agent.to_string(), enabled))
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Install the `busytok` CLI shim on PATH
    Install {
        /// Directory to install the shim into (must be on PATH). Defaults
        /// to `~/.local/bin`; use `/usr/local/bin` (with sudo) for a
        /// system-wide install.
        #[arg(long, default_value = "~/.local/bin")]
        bin_dir: String,

        /// Path to the Busytok.app bundle
        #[arg(long)]
        app_bundle_path: Option<String>,
    },

    /// Show CLI shim installation status
    Status {
        /// Directory where the shim is installed
        #[arg(long, default_value = "~/.local/bin")]
        bin_dir: String,
    },

    /// Uninstall the CLI shim
    Uninstall {
        /// Directory where the shim is installed
        #[arg(long, default_value = "~/.local/bin")]
        bin_dir: String,
    },
}

/// Resolve a `~/`-prefixed path to an absolute one. Used by the shim
/// commands so users can copy the documented default verbatim.
fn expand_home(p: &str) -> std::path::PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    std::path::PathBuf::from(p)
}

/// Parse `agent:path` strings like `claude-code:/path/to/logs`.
fn parse_add_root(s: &str) -> Result<(String, String), String> {
    let (agent, path) = s
        .split_once(':')
        .ok_or_else(|| format!("expected agent:path, got '{s}'"))?;
    Ok((agent.to_string(), path.to_string()))
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let session_id = uuid::Uuid::new_v4().to_string();
    let paths = BusytokPaths::new();
    let _ = paths.ensure_dirs_exist();

    // Route --log-dir / BUSYTOK_LOG_DIR through env for the shared factory
    if let Some(dir) = &args.log_dir {
        std::env::set_var("BUSYTOK_LOG_DIR", dir);
    }
    // --verbose restores the info-level terminal output that was the
    // pre-quiet default. Only set if RUST_LOG isn't already set so we
    // don't clobber a more specific user configuration.
    if args.verbose && std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    let _guards = init_logging(&paths.log_dir(), LogSource::Cli, &session_id);

    let _root = tracing::info_span!(
        "cli_process",
        session_id = %session_id,
        source = "cli",
        pid = std::process::id(),
    )
    .entered();

    info!(
        event_code = "cli.startup.begin",
        session_id = %session_id,
        version = env!("CARGO_PKG_VERSION"),
        "CLI starting"
    );

    let cmd_name = command_name(args.command.as_ref().unwrap_or(&Command::Status));
    info!(
        event_code = "cli.command",
        command = cmd_name,
        "executing command"
    );

    let result = run(args).await;
    match &result {
        Ok(()) => info!(
            event_code = "cli.complete",
            command = cmd_name,
            "command completed"
        ),
        Err(e) => {
            error!(
                event_code = "cli.complete",
                command = cmd_name,
                error = %e,
                "command failed"
            );
        }
    }

    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

/// Return a human-readable name for the selected (sub)command.
fn command_name(cmd: &Command) -> &'static str {
    match cmd {
        Command::Status => "status",
        Command::Scan { .. } => "scan",
        Command::Sources { .. } => "sources",
        Command::Usage { .. } => "usage",
        Command::Diagnostics { .. } => "diagnostics",
        Command::Settings { .. } => "settings",
        Command::Prompt { .. } => "prompt",
        Command::Cli { .. } => "cli",
        Command::Delegate { .. } => "delegate",
        Command::Subagent { .. } => "subagent",
        Command::Doctor => "doctor",
        Command::Models { .. } => "models",
        Command::Provider { .. } => "provider",
    }
}

async fn run(args: Args) -> anyhow::Result<()> {
    let command = args.command.unwrap_or(Command::Status);

    match command {
        Command::Status => commands::handle_status().await,

        Command::Scan {
            offline,
            agent,
            path,
        } => {
            if offline {
                let agent = agent.as_deref().unwrap_or("claude-code");
                let path = path
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("--path is required for offline scan"))?;
                commands::handle_scan_offline(agent, path).await
            } else {
                // Online scan: delegate to sources.rescan via control method.
                commands::handle_sources_rescan(None, false).await
            }
        }

        Command::Sources { subcommand } => match subcommand {
            SourcesCommand::List => commands::handle_sources_list().await,
            SourcesCommand::Status { id } => commands::handle_sources_status(&id).await,
            SourcesCommand::Rescan { source_id, dry_run } => {
                commands::handle_sources_rescan(source_id.as_deref(), dry_run).await
            }
        },

        Command::Usage { subcommand } => match subcommand {
            UsageCommand::Summary => commands::handle_usage_summary().await,
            UsageCommand::Timeline {
                since,
                until,
                agent,
            } => {
                commands::handle_usage_timeline(
                    since.as_deref(),
                    until.as_deref(),
                    agent.as_deref(),
                )
                .await
            }
            UsageCommand::Events { cursor, limit } => {
                commands::handle_usage_events(cursor.as_deref(), limit).await
            }
            UsageCommand::Projects => commands::handle_usage_projects().await,
            UsageCommand::Models => commands::handle_usage_models().await,
            UsageCommand::Sessions => commands::handle_usage_sessions().await,
            UsageCommand::Export {
                kind,
                format,
                agent,
                dry_run,
            } => commands::handle_usage_export(&kind, &format, agent.as_deref(), dry_run).await,
        },

        Command::Diagnostics { subcommand } => match subcommand {
            DiagnosticsCommand::ScanStatus => commands::handle_diagnostics_scan_status().await,
            DiagnosticsCommand::StoreHealth => commands::handle_diagnostics_store_health().await,
        },

        Command::Prompt { subcommand } => match subcommand {
            PromptCommand::Create {
                content,
                alias,
                tags,
                batch,
            } => commands::handle_prompt_create(content, alias, tags, batch).await,
        },

        Command::Settings { subcommand } => match subcommand {
            SettingsCommand::Snapshot => commands::handle_settings_get().await,
            SettingsCommand::Update {
                timezone,
                discovery_default,
                add_root,
            } => {
                let discovery_refs: Vec<(&str, bool)> = discovery_default
                    .iter()
                    .map(|(a, b)| (a.as_str(), *b))
                    .collect();
                let add_root_ref: Option<(&str, &str)> =
                    add_root.first().map(|(a, p)| (a.as_str(), p.as_str()));
                commands::handle_settings_update(timezone.as_deref(), discovery_refs, add_root_ref)
                    .await
            }
        },

        Command::Cli { subcommand } => match subcommand {
            CliCommand::Install {
                bin_dir,
                app_bundle_path,
            } => {
                let bin_path = expand_home(&bin_dir);
                let paths = BusytokPaths::new();
                let manager = shim::ShimManager::new(paths.config_dir());

                let app_bundle = match app_bundle_path {
                    Some(p) => std::path::PathBuf::from(p),
                    None => {
                        // Try to auto-detect the app bundle location.
                        let fallback_roots = vec![
                            std::path::PathBuf::from("/Applications"),
                            dirs::home_dir()
                                .map(|h| h.join("Applications"))
                                .unwrap_or_default(),
                        ];
                        shim::resolve_app_bundle_for_shim(None, &fallback_roots)?
                    }
                };

                manager
                    .install(&bin_path, &app_bundle)
                    .context("installing CLI shim")
            }
            CliCommand::Status { bin_dir } => {
                let bin_path = expand_home(&bin_dir);
                let paths = BusytokPaths::new();
                let manager = shim::ShimManager::new(paths.config_dir());
                manager.status(&bin_path)
            }
            CliCommand::Uninstall { bin_dir } => {
                let bin_path = expand_home(&bin_dir);
                let paths = BusytokPaths::new();
                let manager = shim::ShimManager::new(paths.config_dir());
                manager.uninstall(&bin_path)
            }
        },

        Command::Delegate {
            subagent,
            id,
            cwd,
            profile,
            intent,
            model,
            timeout,
            output,
            bind_provider,
            bind_model,
            wait,
            prompt_file,
            stdin_prompt,
            artifact_ref,
            wait_timeout,
            poll_interval,
            reuse_policy,
            prompt,
        } => {
            commands_subagent::handle_delegate(
                subagent,
                id,
                cwd,
                profile,
                intent,
                model,
                timeout,
                output,
                prompt,
                prompt_file,
                stdin_prompt,
                artifact_ref,
                bind_provider,
                bind_model,
                wait,
                wait_timeout,
                poll_interval,
                reuse_policy,
            )
            .await
        }

        Command::Subagent { subcommand } => match subcommand {
            SubagentCommand::List {
                status,
                project,
                include_deleted,
                output,
            } => commands_subagent::handle_list(status, project, include_deleted, output).await,
            SubagentCommand::Show {
                name,
                id,
                cwd,
                output,
            } => commands_subagent::handle_show(name, id, cwd, output).await,
            SubagentCommand::Tasks {
                name,
                id,
                cwd,
                limit,
                output,
            } => commands_subagent::handle_tasks(name, id, cwd, limit, output).await,
            SubagentCommand::Hibernate {
                name,
                id,
                cwd,
                output,
            } => commands_subagent::handle_hibernate(name, id, cwd, output).await,
            SubagentCommand::Delete {
                name,
                id,
                cwd,
                hard,
                yes,
                output,
            } => commands_subagent::handle_delete(name, id, cwd, hard, yes, output).await,
            SubagentCommand::Task { task_id, output } => {
                commands_subagent::handle_task_get(task_id, output).await
            }
            SubagentCommand::Cancel {
                task_id,
                reason,
                output,
            } => commands_subagent::handle_task_cancel(task_id, reason, output).await,
        },

        Command::Models {
            provider,
            tags,
            all,
            json,
            sort,
            reasoning,
        } => commands::models::handle_models(provider, tags, all, json, sort, reasoning).await,

        Command::Provider { subcommand } => commands::provider::handle(subcommand).await,

        Command::Doctor => commands::handle_doctor().await,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
    use super::*;

    // ── command_name ────────────────────────────────────────────────────

    #[test]
    fn command_name_returns_status_for_status_variant() {
        assert_eq!(command_name(&Command::Status), "status");
    }

    #[test]
    fn command_name_returns_scan_for_scan_variant() {
        let cmd = Command::Scan {
            offline: false,
            agent: None,
            path: None,
        };
        assert_eq!(command_name(&cmd), "scan");
    }

    #[test]
    fn command_name_returns_sources_for_sources_variant() {
        let cmd = Command::Sources {
            subcommand: SourcesCommand::List,
        };
        assert_eq!(command_name(&cmd), "sources");
    }

    #[test]
    fn command_name_returns_usage_for_usage_variant() {
        let cmd = Command::Usage {
            subcommand: UsageCommand::Summary,
        };
        assert_eq!(command_name(&cmd), "usage");
    }

    #[test]
    fn command_name_returns_diagnostics_for_diagnostics_variant() {
        let cmd = Command::Diagnostics {
            subcommand: DiagnosticsCommand::ScanStatus,
        };
        assert_eq!(command_name(&cmd), "diagnostics");
    }

    #[test]
    fn command_name_returns_settings_for_settings_variant() {
        let cmd = Command::Settings {
            subcommand: SettingsCommand::Snapshot,
        };
        assert_eq!(command_name(&cmd), "settings");
    }

    #[test]
    fn command_name_returns_prompt_for_prompt_variant() {
        let cmd = Command::Prompt {
            subcommand: PromptCommand::Create {
                content: None,
                alias: None,
                tags: vec![],
                batch: false,
            },
        };
        assert_eq!(command_name(&cmd), "prompt");
    }

    #[test]
    fn command_name_returns_cli_for_cli_variant() {
        let cmd = Command::Cli {
            subcommand: CliCommand::Status {
                bin_dir: "~/.local/bin".to_string(),
            },
        };
        assert_eq!(command_name(&cmd), "cli");
    }

    #[test]
    fn command_name_returns_delegate_for_delegate_variant() {
        let cmd = Command::Delegate {
            subagent: "worker".to_string(),
            id: None,
            cwd: ".".to_string(),
            profile: "default".to_string(),
            intent: None,
            model: None,
            timeout: None,
            output: "text".to_string(),
            bind_provider: None,
            bind_model: None,
            wait: false,
            prompt_file: None,
            stdin_prompt: false,
            artifact_ref: None,
            wait_timeout: None,
            poll_interval: None,
            reuse_policy: None,
            prompt: Some("do thing".to_string()),
        };
        assert_eq!(command_name(&cmd), "delegate");
    }

    #[test]
    fn command_name_returns_subagent_for_subagent_variant() {
        let cmd = Command::Subagent {
            subcommand: SubagentCommand::List {
                status: None,
                project: None,
                include_deleted: false,
                output: "text".to_string(),
            },
        };
        assert_eq!(command_name(&cmd), "subagent");
    }

    #[test]
    fn args_parses_delegate_with_bind_provider_and_bind_model() {
        let args = Args::try_parse_from([
            "busytok",
            "delegate",
            "--subagent",
            "worker",
            "--profile",
            "default",
            "--bind-provider",
            "prov-1",
            "--bind-model",
            "model-1",
            "do the thing",
        ])
        .unwrap();
        match args.command {
            Some(Command::Delegate {
                bind_provider,
                bind_model,
                model,
                ..
            }) => {
                assert_eq!(bind_provider.as_deref(), Some("prov-1"));
                assert_eq!(bind_model.as_deref(), Some("model-1"));
                // --model (task-level override) is separate and defaults to None
                assert_eq!(model, None);
            }
            other => panic!("expected Delegate with bind flags, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_delegate_with_model_override_separate_from_bind_model() {
        // --model maps to model_override (task-level); --bind-model maps to
        // the bound model. Both can coexist without conflict.
        let args = Args::try_parse_from([
            "busytok",
            "delegate",
            "--subagent",
            "worker",
            "--profile",
            "default",
            "--model",
            "gpt-4",
            "--bind-provider",
            "prov-1",
            "--bind-model",
            "model-1",
            "do the thing",
        ])
        .unwrap();
        match args.command {
            Some(Command::Delegate {
                model,
                bind_provider,
                bind_model,
                ..
            }) => {
                assert_eq!(model.as_deref(), Some("gpt-4"));
                assert_eq!(bind_provider.as_deref(), Some("prov-1"));
                assert_eq!(bind_model.as_deref(), Some("model-1"));
            }
            other => panic!("expected Delegate with both --model and --bind-*, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_delegate_with_wait_flag() {
        // --wait is a bool flag: present → true.
        let args = Args::try_parse_from([
            "busytok",
            "delegate",
            "--subagent",
            "worker",
            "--profile",
            "default",
            "--wait",
            "do the thing",
        ])
        .unwrap();
        match args.command {
            Some(Command::Delegate { wait, .. }) => {
                assert!(wait, "--wait present → wait should be true");
            }
            other => panic!("expected Delegate with --wait, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_delegate_without_wait_flag_defaults_false() {
        // Without --wait, the flag defaults to false.
        let args = Args::try_parse_from([
            "busytok",
            "delegate",
            "--subagent",
            "worker",
            "--profile",
            "default",
            "do the thing",
        ])
        .unwrap();
        match args.command {
            Some(Command::Delegate { wait, .. }) => {
                assert!(!wait, "no --wait → wait should default to false");
            }
            other => panic!("expected Delegate without --wait, got: {other:?}"),
        }
    }

    #[test]
    fn command_name_returns_doctor_for_doctor_variant() {
        assert_eq!(command_name(&Command::Doctor), "doctor");
    }

    #[test]
    fn command_name_returns_models_for_models_variant() {
        let cmd = Command::Models {
            provider: None,
            tags: vec![],
            all: false,
            json: false,
            sort: None,
            reasoning: false,
        };
        assert_eq!(command_name(&cmd), "models");
    }

    // ── parse_discovery_default ─────────────────────────────────────────

    #[test]
    fn parse_discovery_default_accepts_true_value() {
        let result = parse_discovery_default("claude-code:true").unwrap();
        assert_eq!(result.0, "claude-code");
        assert!(result.1);
    }

    #[test]
    fn parse_discovery_default_accepts_false_value() {
        let result = parse_discovery_default("codex:false").unwrap();
        assert_eq!(result.0, "codex");
        assert!(!result.1);
    }

    #[test]
    fn parse_discovery_default_rejects_missing_colon() {
        let err = parse_discovery_default("claude-code").unwrap_err();
        assert!(
            err.contains("expected agent:bool"),
            "should mention agent:bool, got: {err}"
        );
        assert!(
            err.contains("claude-code"),
            "should echo back the bad input: {err}"
        );
    }

    #[test]
    fn parse_discovery_default_rejects_non_bool_value() {
        let err = parse_discovery_default("claude-code:yes").unwrap_err();
        assert!(
            err.contains("expected true or false"),
            "should mention true/false, got: {err}"
        );
        assert!(err.contains("yes"), "should echo back the bad value: {err}");
    }

    #[test]
    fn parse_discovery_default_accepts_empty_agent_name() {
        // An empty agent string is structurally valid (agent:bool shape);
        // validation of agent names happens downstream in handle_settings_update.
        let result = parse_discovery_default(":true").unwrap();
        assert_eq!(result.0, "");
        assert!(result.1);
    }

    // ── parse_add_root ─────────────────────────────────────────────────

    #[test]
    fn parse_add_root_accepts_absolute_path() {
        let result = parse_add_root("claude-code:/path/to/logs").unwrap();
        assert_eq!(result.0, "claude-code");
        assert_eq!(result.1, "/path/to/logs");
    }

    #[test]
    fn parse_add_root_accepts_relative_path() {
        let result = parse_add_root("codex:relative/path").unwrap();
        assert_eq!(result.0, "codex");
        assert_eq!(result.1, "relative/path");
    }

    #[test]
    fn parse_add_root_rejects_missing_colon() {
        let err = parse_add_root("claude-code").unwrap_err();
        assert!(
            err.contains("expected agent:path"),
            "should mention agent:path, got: {err}"
        );
        assert!(
            err.contains("claude-code"),
            "should echo back the bad input: {err}"
        );
    }

    #[test]
    fn parse_add_root_accepts_empty_path() {
        // Structurally valid (agent:path shape); downstream validation
        // handles empty paths.
        let result = parse_add_root("claude-code:").unwrap();
        assert_eq!(result.0, "claude-code");
        assert_eq!(result.1, "");
    }

    // ── expand_home ────────────────────────────────────────────────────

    #[test]
    fn expand_home_expands_tilde_prefix() {
        let home = dirs::home_dir().expect("home_dir should be available in test env");
        let result = expand_home("~/some/relative/path");
        assert_eq!(result, home.join("some/relative/path"));
    }

    #[test]
    fn expand_home_returns_home_for_bare_tilde() {
        // "~/".strip_prefix("~/") returns Some("") so this yields home.join("")
        // which is equivalent to the home directory itself.
        let home = dirs::home_dir().expect("home_dir should be available in test env");
        let result = expand_home("~/");
        assert_eq!(result, home.join(""));
    }

    #[test]
    fn expand_home_does_not_expand_absolute_path() {
        let result = expand_home("/absolute/path");
        assert_eq!(result, std::path::PathBuf::from("/absolute/path"));
    }

    #[test]
    fn expand_home_does_not_expand_relative_path() {
        let result = expand_home("relative/path");
        assert_eq!(result, std::path::PathBuf::from("relative/path"));
    }

    #[test]
    fn expand_home_does_not_expand_tilde_in_middle() {
        // Only a leading "~/..." is expanded; a tilde elsewhere is literal.
        let result = expand_home("/foo/~/bar");
        assert_eq!(result, std::path::PathBuf::from("/foo/~/bar"));
    }

    #[test]
    fn expand_home_returns_unchanged_when_home_dir_unavailable() {
        // We can't easily force `dirs::home_dir()` to return None in a real
        // test environment, but we can at least exercise the non-tilde branch
        // (which is the same fallback path) for a non-~/ input.
        let result = expand_home("no-tilde-here");
        assert_eq!(result, std::path::PathBuf::from("no-tilde-here"));
    }

    // ── Args parsing round-trip via clap ───────────────────────────────

    #[test]
    fn args_defaults_to_none_command_when_omitted() {
        // clap's Parser::try_parse_from with no args yields Args with
        // command=None (the default behavior when no subcommand is given).
        let args = Args::try_parse_from(["busytok"]).unwrap();
        assert!(args.command.is_none());
        assert!(args.log_dir.is_none());
    }

    #[test]
    fn args_parses_log_dir_flag() {
        let args = Args::try_parse_from(["busytok", "--log-dir", "/tmp/logs"]).unwrap();
        assert_eq!(
            args.log_dir.as_deref(),
            Some(std::path::Path::new("/tmp/logs"))
        );
    }

    #[test]
    fn args_verbose_defaults_to_false() {
        let args = Args::try_parse_from(["busytok"]).unwrap();
        assert!(!args.verbose);
    }

    #[test]
    fn args_parses_verbose_short_flag() {
        let args = Args::try_parse_from(["busytok", "-v", "status"]).unwrap();
        assert!(args.verbose);
        assert!(matches!(args.command, Some(Command::Status)));
    }

    #[test]
    fn args_parses_verbose_long_flag() {
        let args = Args::try_parse_from(["busytok", "--verbose", "status"]).unwrap();
        assert!(args.verbose);
    }

    #[test]
    fn args_parses_status_subcommand() {
        let args = Args::try_parse_from(["busytok", "status"]).unwrap();
        assert!(matches!(args.command, Some(Command::Status)));
    }

    #[test]
    fn args_parses_doctor_subcommand() {
        let args = Args::try_parse_from(["busytok", "doctor"]).unwrap();
        assert!(matches!(args.command, Some(Command::Doctor)));
    }

    #[test]
    fn args_parses_models_subcommand_with_defaults() {
        let args = Args::try_parse_from(["busytok", "models"]).unwrap();
        match args.command {
            Some(Command::Models {
                provider,
                tags,
                all,
                json,
                sort,
                reasoning,
            }) => {
                assert!(provider.is_none());
                assert!(tags.is_empty());
                assert!(!all);
                assert!(!json);
                assert!(sort.is_none());
                assert!(!reasoning);
            }
            other => panic!("expected Models, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_models_subcommand_with_all_flags() {
        let args = Args::try_parse_from([
            "busytok",
            "models",
            "--provider",
            "p1",
            "--tag",
            "chat",
            "--tag",
            "fast",
            "--all",
            "--json",
            "--sort",
            "context_window_desc",
            "--reasoning",
        ])
        .unwrap();
        match args.command {
            Some(Command::Models {
                provider,
                tags,
                all,
                json,
                sort,
                reasoning,
            }) => {
                assert_eq!(provider.as_deref(), Some("p1"));
                assert_eq!(tags, vec!["chat".to_string(), "fast".to_string()]);
                assert!(all);
                assert!(json);
                assert_eq!(sort.as_deref(), Some("context_window_desc"));
                assert!(reasoning);
            }
            other => panic!("expected Models with flags, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_settings_update_with_discovery_default() {
        let args = Args::try_parse_from([
            "busytok",
            "settings",
            "update",
            "--discovery-default",
            "claude-code:true",
            "--discovery-default",
            "codex:false",
        ])
        .unwrap();
        match args.command {
            Some(Command::Settings {
                subcommand:
                    SettingsCommand::Update {
                        discovery_default, ..
                    },
            }) => {
                assert_eq!(discovery_default.len(), 2);
                assert_eq!(discovery_default[0], ("claude-code".to_string(), true));
                assert_eq!(discovery_default[1], ("codex".to_string(), false));
            }
            other => panic!("expected Settings::Update, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_settings_update_with_add_root() {
        let args = Args::try_parse_from([
            "busytok",
            "settings",
            "update",
            "--add-root",
            "claude-code:/path/to/logs",
        ])
        .unwrap();
        match args.command {
            Some(Command::Settings {
                subcommand: SettingsCommand::Update { add_root, .. },
            }) => {
                assert_eq!(add_root.len(), 1);
                assert_eq!(
                    add_root[0],
                    ("claude-code".to_string(), "/path/to/logs".to_string())
                );
            }
            other => panic!("expected Settings::Update, got: {other:?}"),
        }
    }

    #[test]
    fn args_rejects_invalid_discovery_default_value() {
        // clap's value_parser propagates the error from parse_discovery_default.
        let result = Args::try_parse_from([
            "busytok",
            "settings",
            "update",
            "--discovery-default",
            "claude-code:yes",
        ]);
        assert!(
            result.is_err(),
            "should reject non-bool discovery-default value"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("expected true or false"),
            "should surface the parser error, got: {err}"
        );
    }

    #[test]
    fn args_rejects_invalid_add_root_format() {
        let result = Args::try_parse_from([
            "busytok",
            "settings",
            "update",
            "--add-root",
            "no-colon-here",
        ]);
        assert!(result.is_err(), "should reject add-root without colon");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("expected agent:path"),
            "should surface the parser error, got: {err}"
        );
    }

    #[test]
    fn args_parses_subagent_task_with_task_id() {
        // `busytok subagent task --task-id task-1` parses into
        // `SubagentCommand::Task` with `output` defaulting to "text".
        let args =
            Args::try_parse_from(["busytok", "subagent", "task", "--task-id", "task-1"]).unwrap();
        match args.command {
            Some(Command::Subagent {
                subcommand: SubagentCommand::Task { task_id, output },
            }) => {
                assert_eq!(task_id, "task-1");
                assert_eq!(output, "text");
            }
            other => panic!("expected Subagent::Task, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_subagent_task_with_json_output() {
        // `--output json` overrides the default text mode.
        let args = Args::try_parse_from([
            "busytok",
            "subagent",
            "task",
            "--task-id",
            "task-1",
            "--output",
            "json",
        ])
        .unwrap();
        match args.command {
            Some(Command::Subagent {
                subcommand: SubagentCommand::Task { task_id, output },
            }) => {
                assert_eq!(task_id, "task-1");
                assert_eq!(output, "json");
            }
            other => panic!("expected Subagent::Task with json output, got: {other:?}"),
        }
    }

    #[test]
    fn args_rejects_subagent_task_without_task_id() {
        // `--task-id` is required; omitting it must produce a clap error.
        let result = Args::try_parse_from(["busytok", "subagent", "task"]);
        assert!(
            result.is_err(),
            "should reject `subagent task` without --task-id"
        );
    }

    #[test]
    fn args_rejects_subagent_task_with_invalid_output_value() {
        // `--output` only accepts "json" or "text" (value_parser).
        let result = Args::try_parse_from([
            "busytok",
            "subagent",
            "task",
            "--task-id",
            "task-1",
            "--output",
            "yaml",
        ]);
        assert!(
            result.is_err(),
            "should reject `--output yaml` for subagent task"
        );
    }

    // ── Provider subcommand parsing ────────────────────────────────────

    #[test]
    fn args_parses_provider_list() {
        let args = Args::try_parse_from(["busytok", "provider", "list"]).unwrap();
        match args.command {
            Some(Command::Provider { subcommand }) => {
                assert!(matches!(subcommand, ProviderCommand::List { json: false }));
            }
            other => panic!("expected Provider, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_provider_add_with_required_flags() {
        let args = Args::try_parse_from([
            "busytok",
            "provider",
            "add",
            "--url",
            "https://api.deepseek.com/v1",
            "--key",
            "sk-test",
        ])
        .unwrap();
        match args.command {
            Some(Command::Provider {
                subcommand:
                    ProviderCommand::Add {
                        url,
                        key,
                        kind,
                        name,
                        model,
                        tags,
                    },
            }) => {
                assert_eq!(url, "https://api.deepseek.com/v1");
                assert_eq!(key, "sk-test");
                assert_eq!(kind, "openai_compatible");
                assert!(name.is_none());
                assert!(model.is_none());
                assert!(tags.is_none());
            }
            other => panic!("expected Provider Add, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_provider_add_with_all_flags() {
        let args = Args::try_parse_from([
            "busytok",
            "provider",
            "add",
            "--url",
            "https://api.deepseek.com/v1",
            "--key",
            "sk-test",
            "--kind",
            "anthropic_compatible",
            "--name",
            "custom_name",
            "--model",
            "claude-3-opus",
            "--tags",
            "fast,reasoning",
        ])
        .unwrap();
        match args.command {
            Some(Command::Provider {
                subcommand:
                    ProviderCommand::Add {
                        url,
                        key,
                        kind,
                        name,
                        model,
                        tags,
                    },
            }) => {
                assert_eq!(kind, "anthropic_compatible");
                assert_eq!(name.as_deref(), Some("custom_name"));
                assert_eq!(model.as_deref(), Some("claude-3-opus"));
                assert_eq!(tags.as_deref(), Some("fast,reasoning"));
                (url, key); // unused
            }
            other => panic!("expected Provider Add, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_provider_delete_with_yes_flag() {
        let args =
            Args::try_parse_from(["busytok", "provider", "delete", "prov-1", "--yes"]).unwrap();
        match args.command {
            Some(Command::Provider {
                subcommand: ProviderCommand::Delete { id, yes },
            }) => {
                assert_eq!(id, "prov-1");
                assert!(yes);
            }
            other => panic!("expected Provider Delete, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_provider_delete_without_yes_flag() {
        let args = Args::try_parse_from(["busytok", "provider", "delete", "prov-1"]).unwrap();
        match args.command {
            Some(Command::Provider {
                subcommand: ProviderCommand::Delete { yes, .. },
            }) => {
                assert!(!yes);
            }
            other => panic!("expected Provider Delete, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_provider_model_add() {
        let args = Args::try_parse_from([
            "busytok",
            "provider",
            "model",
            "add",
            "prov-1",
            "--name",
            "deepseek-chat",
        ])
        .unwrap();
        match args.command {
            Some(Command::Provider {
                subcommand:
                    ProviderCommand::Model {
                        subcommand:
                            ProviderModelCommand::Add {
                                provider_id,
                                name,
                                tags,
                                context_window,
                                max_tokens,
                                reasoning,
                                display_name,
                            },
                    },
            }) => {
                assert_eq!(provider_id, "prov-1");
                assert_eq!(name, "deepseek-chat");
                assert!(tags.is_none());
                assert!(context_window.is_none());
                assert!(max_tokens.is_none());
                assert!(reasoning);
                assert!(display_name.is_none());
            }
            other => panic!("expected Provider Model Add, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_provider_model_delete_with_yes() {
        let args = Args::try_parse_from([
            "busytok",
            "provider",
            "model",
            "delete",
            "prov-1",
            "deepseek-chat",
            "--yes",
        ])
        .unwrap();
        match args.command {
            Some(Command::Provider {
                subcommand:
                    ProviderCommand::Model {
                        subcommand:
                            ProviderModelCommand::Delete {
                                provider_id,
                                model_id,
                                yes,
                            },
                    },
            }) => {
                assert_eq!(provider_id, "prov-1");
                assert_eq!(model_id, "deepseek-chat");
                assert!(yes);
            }
            other => panic!("expected Provider Model Delete, got: {other:?}"),
        }
    }

    #[test]
    fn command_name_returns_provider_for_provider_variant() {
        let args = Args::try_parse_from(["busytok", "provider", "list"]).unwrap();
        assert_eq!(command_name(args.command.as_ref().unwrap()), "provider");
    }

    #[test]
    fn args_rejects_provider_add_with_invalid_kind() {
        let result = Args::try_parse_from([
            "busytok",
            "provider",
            "add",
            "--url",
            "https://api.deepseek.com",
            "--key",
            "sk-test",
            "--kind",
            "invalid_kind",
        ]);
        assert!(result.is_err());
    }

    // ── Delegate new flags (P0/P1) ────────────────────────────────────

    #[test]
    fn args_parses_delegate_with_prompt_file_flag() {
        let args = Args::try_parse_from([
            "busytok",
            "delegate",
            "--subagent",
            "worker",
            "--profile",
            "default",
            "--prompt-file",
            "/tmp/prompt.txt",
        ])
        .unwrap();
        match args.command {
            Some(Command::Delegate {
                prompt_file,
                prompt,
                stdin_prompt,
                artifact_ref,
                ..
            }) => {
                assert_eq!(prompt_file.as_deref(), Some("/tmp/prompt.txt"));
                assert!(prompt.is_none());
                assert!(!stdin_prompt);
                assert!(artifact_ref.is_none());
            }
            other => panic!("expected Delegate with --prompt-file, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_delegate_with_stdin_flag() {
        let args = Args::try_parse_from([
            "busytok",
            "delegate",
            "--subagent",
            "worker",
            "--profile",
            "default",
            "--stdin",
        ])
        .unwrap();
        match args.command {
            Some(Command::Delegate { stdin_prompt, .. }) => {
                assert!(
                    stdin_prompt,
                    "--stdin present → stdin_prompt should be true"
                );
            }
            other => panic!("expected Delegate with --stdin, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_delegate_with_artifact_ref_flag() {
        let args = Args::try_parse_from([
            "busytok",
            "delegate",
            "--subagent",
            "worker",
            "--profile",
            "default",
            "--artifact-ref",
            "artifact:abc-123",
        ])
        .unwrap();
        match args.command {
            Some(Command::Delegate { artifact_ref, .. }) => {
                assert_eq!(artifact_ref.as_deref(), Some("artifact:abc-123"));
            }
            other => panic!("expected Delegate with --artifact-ref, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_delegate_with_wait_timeout_and_poll_interval() {
        let args = Args::try_parse_from([
            "busytok",
            "delegate",
            "--subagent",
            "worker",
            "--profile",
            "default",
            "--wait",
            "--wait-timeout",
            "120",
            "--poll-interval",
            "5",
            "do the thing",
        ])
        .unwrap();
        match args.command {
            Some(Command::Delegate {
                wait,
                wait_timeout,
                poll_interval,
                ..
            }) => {
                assert!(wait);
                assert_eq!(wait_timeout, Some(120));
                assert_eq!(poll_interval, Some(5));
            }
            other => panic!("expected Delegate with --wait-timeout, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_delegate_with_reuse_policy_create() {
        let args = Args::try_parse_from([
            "busytok",
            "delegate",
            "--subagent",
            "worker",
            "--profile",
            "default",
            "--reuse-policy",
            "create",
            "do the thing",
        ])
        .unwrap();
        match args.command {
            Some(Command::Delegate { reuse_policy, .. }) => {
                assert_eq!(reuse_policy.as_deref(), Some("create"));
            }
            other => panic!("expected Delegate with --reuse-policy create, got: {other:?}"),
        }
    }

    #[test]
    fn args_rejects_delegate_with_invalid_reuse_policy() {
        let result = Args::try_parse_from([
            "busytok",
            "delegate",
            "--subagent",
            "worker",
            "--profile",
            "default",
            "--reuse-policy",
            "invalid",
            "do the thing",
        ]);
        assert!(
            result.is_err(),
            "should reject invalid --reuse-policy value"
        );
    }

    #[test]
    fn args_parses_delegate_prompt_as_optional_when_absent() {
        // No positional prompt, no --prompt-file/--stdin/--artifact-ref.
        // clap should still parse (validation is done in resolve_prompt).
        let args = Args::try_parse_from([
            "busytok",
            "delegate",
            "--subagent",
            "worker",
            "--profile",
            "default",
        ])
        .unwrap();
        match args.command {
            Some(Command::Delegate { prompt, .. }) => {
                assert!(prompt.is_none(), "prompt should be None when absent");
            }
            other => panic!("expected Delegate without prompt, got: {other:?}"),
        }
    }

    // ── Subagent Cancel subcommand ────────────────────────────────────

    #[test]
    fn args_parses_subagent_cancel_with_task_id() {
        let args =
            Args::try_parse_from(["busytok", "subagent", "cancel", "--task-id", "task-1"]).unwrap();
        match args.command {
            Some(Command::Subagent {
                subcommand:
                    SubagentCommand::Cancel {
                        task_id,
                        reason,
                        output,
                    },
            }) => {
                assert_eq!(task_id, "task-1");
                assert!(reason.is_none());
                assert_eq!(output, "text");
            }
            other => panic!("expected Subagent::Cancel, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_subagent_cancel_with_reason_and_json_output() {
        let args = Args::try_parse_from([
            "busytok",
            "subagent",
            "cancel",
            "--task-id",
            "task-1",
            "--reason",
            "user aborted",
            "--output",
            "json",
        ])
        .unwrap();
        match args.command {
            Some(Command::Subagent {
                subcommand:
                    SubagentCommand::Cancel {
                        task_id,
                        reason,
                        output,
                    },
            }) => {
                assert_eq!(task_id, "task-1");
                assert_eq!(reason.as_deref(), Some("user aborted"));
                assert_eq!(output, "json");
            }
            other => panic!("expected Subagent::Cancel with reason, got: {other:?}"),
        }
    }

    #[test]
    fn args_rejects_subagent_cancel_without_task_id() {
        let result = Args::try_parse_from(["busytok", "subagent", "cancel"]);
        assert!(
            result.is_err(),
            "should reject `subagent cancel` without --task-id"
        );
    }

    // ── Subagent read commands with --output ─────────────────────────

    #[test]
    fn args_parses_subagent_list_with_output_json() {
        let args =
            Args::try_parse_from(["busytok", "subagent", "list", "--output", "json"]).unwrap();
        match args.command {
            Some(Command::Subagent {
                subcommand: SubagentCommand::List { output, .. },
            }) => {
                assert_eq!(output, "json");
            }
            other => panic!("expected Subagent::List with json, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_subagent_show_with_output_json() {
        let args = Args::try_parse_from([
            "busytok", "subagent", "show", "my-agent", "--output", "json",
        ])
        .unwrap();
        match args.command {
            Some(Command::Subagent {
                subcommand: SubagentCommand::Show { output, .. },
            }) => {
                assert_eq!(output, "json");
            }
            other => panic!("expected Subagent::Show with json, got: {other:?}"),
        }
    }

    #[test]
    fn args_parses_models_with_sort_and_reasoning() {
        let args = Args::try_parse_from([
            "busytok",
            "models",
            "--sort",
            "context_window_desc",
            "--reasoning",
        ])
        .unwrap();
        match args.command {
            Some(Command::Models {
                sort, reasoning, ..
            }) => {
                assert_eq!(sort.as_deref(), Some("context_window_desc"));
                assert!(reasoning);
            }
            other => panic!("expected Models with sort+reasoning, got: {other:?}"),
        }
    }

    #[test]
    fn args_rejects_models_with_invalid_sort_value() {
        let result = Args::try_parse_from(["busytok", "models", "--sort", "price"]);
        assert!(
            result.is_err(),
            "should reject invalid --sort value for models"
        );
    }
}
