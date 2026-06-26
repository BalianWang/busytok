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
        #[arg(long, default_value = "text")]
        output: String,
        /// The task prompt (positional)
        prompt: String,
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
    },

    /// Hibernate a subagent (move to cold tier)
    Hibernate {
        #[arg(required_unless_present = "id", conflicts_with = "id")]
        name: Option<String>,
        #[arg(long, conflicts_with = "name")]
        id: Option<String>,
        #[arg(long, default_value = ".")]
        cwd: String,
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
            prompt,
        } => {
            commands_subagent::handle_delegate(
                subagent, id, cwd, profile, intent, model, timeout, output, prompt,
            )
            .await
        }

        Command::Subagent { subcommand } => match subcommand {
            SubagentCommand::List {
                status,
                project,
                include_deleted,
            } => commands_subagent::handle_list(status, project, include_deleted).await,
            SubagentCommand::Show { name, id, cwd } => {
                commands_subagent::handle_show(name, id, cwd).await
            }
            SubagentCommand::Tasks {
                name,
                id,
                cwd,
                limit,
            } => commands_subagent::handle_tasks(name, id, cwd, limit).await,
            SubagentCommand::Hibernate { name, id, cwd } => {
                commands_subagent::handle_hibernate(name, id, cwd).await
            }
            SubagentCommand::Delete {
                name,
                id,
                cwd,
                hard,
                yes,
            } => commands_subagent::handle_delete(name, id, cwd, hard, yes).await,
        },

        Command::Doctor => commands::handle_doctor().await,
    }
}
