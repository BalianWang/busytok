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
        #[arg(long, default_value = "text", value_parser = ["json", "text"])]
        output: String,
        /// Provider ID to bind a new subagent to (required with --bind-model for new subagents)
        #[arg(long)]
        bind_provider: Option<String>,
        /// Model ID to bind a new subagent to (required with --bind-provider for new subagents)
        #[arg(long)]
        bind_model: Option<String>,
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
        Command::Models { .. } => "models",
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
                bind_provider,
                bind_model,
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

        Command::Models {
            provider,
            tags,
            all,
            json,
        } => commands::models::handle_models(provider, tags, all, json).await,

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
            prompt: "do thing".to_string(),
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
            },
        };
        assert_eq!(command_name(&cmd), "subagent");
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
            }) => {
                assert!(provider.is_none());
                assert!(tags.is_empty());
                assert!(!all);
                assert!(!json);
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
        ])
        .unwrap();
        match args.command {
            Some(Command::Models {
                provider,
                tags,
                all,
                json,
            }) => {
                assert_eq!(provider.as_deref(), Some("p1"));
                assert_eq!(tags, vec!["chat".to_string(), "fast".to_string()]);
                assert!(all);
                assert!(json);
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
}
