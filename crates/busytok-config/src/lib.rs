#![allow(warnings, clippy::all)]
#![allow(unexpected_cfgs)]
#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::all))]
mod logging;
mod paths;
pub mod platform;
pub mod service_marker;

pub use logging::{init_logging, prune_old_logs, LogSource, LoggingGuards};
pub use paths::BusytokPaths;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::warn;

const SETTINGS_FILE_NAME: &str = "settings.toml";

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

/// Atomically write `contents` to `path` via temp-file + rename.
///
/// Shared by `BusytokSettings::save` and the GUI-side local lifecycle
/// settings store so atomic-write logic doesn't diverge.
pub fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&parent)
        .with_context(|| format!("failed to create dir {}", parent.display()))?;

    let tmp_path = parent.join(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("settings"),
        std::process::id(),
        uuid::Uuid::new_v4()
    ));

    std::fs::write(&tmp_path, contents)
        .with_context(|| format!("failed to write {}", tmp_path.display()))?;

    if let Err(err) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err).with_context(|| {
            format!(
                "failed to atomically replace {} with {}",
                path.display(),
                tmp_path.display()
            )
        });
    }

    Ok(())
}

/// A configured manual root for a specific agent client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualRootConfig {
    pub id: String,
    pub client_id: String,
    pub root_path: String,
}

/// Privacy-related settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacySettings {
    #[serde(default = "default_true")]
    pub local_only: bool,
    #[serde(default = "default_true")]
    pub redact_sensitive_values: bool,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        Self {
            local_only: true,
            redact_sensitive_values: true,
        }
    }
}

/// Persisted Busytok settings, stored as TOML in the config directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusytokSettings {
    pub timezone: String,
    /// Weekday index: 0=Sunday, 1=Monday, ... 6=Saturday. Defaults to 1 (Monday).
    #[serde(default = "default_week_starts_on")]
    pub week_starts_on: u8,
    #[serde(default)]
    pub privacy: PrivacySettings,
    #[serde(default)]
    pub discovery: DiscoverySettings,
    #[serde(default)]
    pub prompt_palette_default_action: PromptDefaultAction,
    #[serde(default)]
    pub subagent: SubagentSettings,
}

fn default_week_starts_on() -> u8 {
    1
}

impl Default for BusytokSettings {
    fn default() -> Self {
        Self {
            timezone: busytok_domain::detect_system_iana_timezone(),
            week_starts_on: default_week_starts_on(),
            privacy: PrivacySettings::default(),
            discovery: DiscoverySettings::default(),
            prompt_palette_default_action: PromptDefaultAction::default(),
            subagent: SubagentSettings::default(),
        }
    }
}

/// Default action when using a prompt palette entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PromptDefaultAction {
    #[serde(rename = "OnlyCopy", alias = "copy")]
    OnlyCopy,
    #[serde(rename = "OnlyPaste")]
    OnlyPaste,
    #[serde(rename = "Copy&Paste", alias = "paste")]
    CopyAndPaste,
}

impl Default for PromptDefaultAction {
    fn default() -> Self {
        Self::CopyAndPaste
    }
}

/// Discovery-related settings controlling which agent log sources are auto-discovered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverySettings {
    #[serde(default = "default_true")]
    pub claude_code_default_paths: bool,
    #[serde(default = "default_true")]
    pub codex_default_paths: bool,
    #[serde(default)]
    pub manual_roots: Vec<ManualRootConfig>,
}

impl Default for DiscoverySettings {
    fn default() -> Self {
        Self {
            claude_code_default_paths: true,
            codex_default_paths: true,
            manual_roots: vec![],
        }
    }
}

// --- subagent settings -----------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub pi_sidecar: SubagentPiSidecarConfig,
    #[serde(default)]
    pub context: SubagentContextConfig,
    #[serde(default)]
    pub resource_policy: SubagentResourcePolicyConfig,
    #[serde(default)]
    pub models: SubagentModelsConfig,
    #[serde(default)]
    pub profiles: std::collections::HashMap<String, SubagentProfileConfig>,
}
impl Default for SubagentSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            pi_sidecar: SubagentPiSidecarConfig::default(),
            context: SubagentContextConfig::default(),
            resource_policy: SubagentResourcePolicyConfig::default(),
            models: SubagentModelsConfig::default(),
            profiles: default_profiles(),
        }
    }
}

fn default_max_hot_sessions() -> u32 {
    3
}
fn default_idle_exit_seconds() -> u64 {
    300
}
fn default_hibernate_after_seconds() -> u64 {
    600
}
fn default_task_timeout_seconds() -> u64 {
    300
}
fn default_task_queue_max() -> u32 {
    50
}
fn default_memory_soft_limit_mb() -> u32 {
    800
}
fn default_memory_hard_limit_mb() -> u32 {
    1200
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentPiSidecarConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// "bundled" | "system"
    #[serde(default = "default_bundled_runtime")]
    pub node_runtime: String,
    #[serde(default)]
    pub system_node_path: String,
    #[serde(default = "default_max_hot_sessions")]
    pub max_hot_sessions: u32,
    #[serde(default = "default_idle_exit_seconds")]
    pub idle_exit_seconds: u64,
    #[serde(default = "default_hibernate_after_seconds")]
    pub hibernate_after_seconds: u64,
    #[serde(default = "default_task_timeout_seconds")]
    pub task_timeout_seconds: u64,
    #[serde(default = "default_memory_soft_limit_mb")]
    pub memory_soft_limit_mb: u32,
    #[serde(default = "default_memory_hard_limit_mb")]
    pub memory_hard_limit_mb: u32,
    #[serde(default = "default_task_queue_max")]
    pub task_queue_max: u32,
}
impl Default for SubagentPiSidecarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            node_runtime: default_bundled_runtime(),
            system_node_path: String::new(),
            max_hot_sessions: default_max_hot_sessions(),
            idle_exit_seconds: default_idle_exit_seconds(),
            hibernate_after_seconds: default_hibernate_after_seconds(),
            task_timeout_seconds: default_task_timeout_seconds(),
            memory_soft_limit_mb: default_memory_soft_limit_mb(),
            memory_hard_limit_mb: default_memory_hard_limit_mb(),
            task_queue_max: default_task_queue_max(),
        }
    }
}
fn default_bundled_runtime() -> String {
    "bundled".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentContextConfig {
    #[serde(default = "default_budget_tokens")]
    pub default_budget_tokens: u32,
    #[serde(default = "default_max_budget_tokens")]
    pub max_budget_tokens: u32,
    #[serde(default = "default_recent_tasks_limit")]
    pub recent_tasks_limit: u32,
    #[serde(default = "default_compaction_tasks_threshold")]
    pub compaction_tasks_threshold: u32,
    #[serde(default = "default_compaction_budget_ratio")]
    pub compaction_budget_ratio: f64,
}
impl Default for SubagentContextConfig {
    fn default() -> Self {
        Self {
            default_budget_tokens: default_budget_tokens(),
            max_budget_tokens: default_max_budget_tokens(),
            recent_tasks_limit: default_recent_tasks_limit(),
            compaction_tasks_threshold: default_compaction_tasks_threshold(),
            compaction_budget_ratio: default_compaction_budget_ratio(),
        }
    }
}
fn default_budget_tokens() -> u32 {
    4000
}
fn default_max_budget_tokens() -> u32 {
    8000
}
fn default_recent_tasks_limit() -> u32 {
    5
}
fn default_compaction_tasks_threshold() -> u32 {
    5
}
fn default_compaction_budget_ratio() -> f64 {
    0.7
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentResourcePolicyConfig {
    /// System free-memory threshold below which the runtime applies backpressure.
    #[serde(default = "default_memory_pressure_free_mb")]
    pub memory_pressure_free_mb: u32,
    /// Resource sampling interval for ResourceMonitor (Plan 5).
    #[serde(default = "default_monitor_interval_seconds")]
    pub monitor_interval_seconds: u64,
}
impl Default for SubagentResourcePolicyConfig {
    fn default() -> Self {
        Self {
            memory_pressure_free_mb: default_memory_pressure_free_mb(),
            monitor_interval_seconds: default_monitor_interval_seconds(),
        }
    }
}
fn default_memory_pressure_free_mb() -> u32 {
    2048
}
fn default_monitor_interval_seconds() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentModelsConfig {
    #[serde(default = "default_cheap_model")]
    pub default_cheap_model: String,
    #[serde(default = "default_review_model")]
    pub default_review_model: String,
    #[serde(default = "default_reasoning_model")]
    pub default_reasoning_model: String,
    #[serde(default = "default_coder_model")]
    pub default_coder_model: String,
}
impl Default for SubagentModelsConfig {
    fn default() -> Self {
        Self {
            default_cheap_model: default_cheap_model(),
            default_review_model: default_review_model(),
            default_reasoning_model: default_reasoning_model(),
            default_coder_model: default_coder_model(),
        }
    }
}
fn default_cheap_model() -> String {
    "deepseek-chat".to_string()
}
fn default_review_model() -> String {
    "qwen-coder".to_string()
}
fn default_reasoning_model() -> String {
    "deepseek-reasoner".to_string()
}
fn default_coder_model() -> String {
    "qwen-coder".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentProfileConfig {
    #[serde(default = "default_false")]
    pub write_access: bool,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_budget_tokens")]
    pub context_budget_tokens: u32,
    #[serde(default = "default_task_timeout_seconds")]
    pub timeout_seconds: u64,
}

/// The built-in read-only profiles for MVP. `pi/patch-small` is deferred.
fn default_profiles() -> std::collections::HashMap<String, SubagentProfileConfig> {
    let mut m = std::collections::HashMap::new();
    m.insert(
        "pi/search-cheap".to_string(),
        SubagentProfileConfig {
            write_access: false,
            tools: vec!["read".to_string(), "grep".to_string()],
            model: default_cheap_model(),
            context_budget_tokens: 3000,
            timeout_seconds: 120,
        },
    );
    m.insert(
        "pi/review-cheap".to_string(),
        SubagentProfileConfig {
            write_access: false,
            tools: vec![
                "read".to_string(),
                "grep".to_string(),
                "git_diff".to_string(),
            ],
            model: default_review_model(),
            context_budget_tokens: 5000,
            timeout_seconds: 180,
        },
    );
    m.insert(
        "pi/plan-cheap".to_string(),
        SubagentProfileConfig {
            write_access: false,
            tools: vec![
                "read".to_string(),
                "grep".to_string(),
                "git_diff".to_string(),
            ],
            model: default_reasoning_model(),
            context_budget_tokens: 6000,
            timeout_seconds: 300,
        },
    );
    m
}

impl BusytokSettings {
    /// Load settings from `paths.config_dir()/settings.toml`.
    ///
    /// Returns defaults if the file does not exist. Returns defaults with a
    /// warning log if the file exists but cannot be parsed.
    pub fn load(paths: &BusytokPaths) -> Result<Self> {
        let file_path = paths.config_dir().join(SETTINGS_FILE_NAME);

        if !file_path.exists() {
            return Ok(Self::default());
        }

        let contents = std::fs::read_to_string(&file_path)
            .with_context(|| format!("failed to read {}", file_path.display()))?;

        match toml::from_str::<Self>(&contents) {
            Ok(mut settings) => {
                // Canonicalize timezone: resolve "local" → system IANA, validate all forms.
                match busytok_domain::ReportingTimezone::parse(&settings.timezone) {
                    Ok(rtz) => {
                        let canonical = rtz.canonical_name().to_string();
                        if canonical != settings.timezone {
                            tracing::info!(
                                event_code = "timezone.canonicalized",
                                old_timezone = %settings.timezone,
                                new_timezone = %canonical,
                                "timezone canonicalized during settings load"
                            );
                            settings.timezone = canonical;
                            let _ = settings.save(paths);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            event_code = "timezone.parse_failed",
                            old_timezone = %settings.timezone,
                            error = %e,
                            "failed to parse timezone, falling back to system IANA"
                        );
                        settings.timezone = busytok_domain::resolve_local_timezone();
                        let _ = settings.save(paths);
                    }
                }
                Ok(settings)
            }
            Err(e) => {
                warn!(
                    "Corrupt settings file {}: {e}; falling back to defaults",
                    file_path.display()
                );
                Ok(Self::default())
            }
        }
    }

    /// Save settings to `paths.config_dir()/settings.toml`.
    ///
    /// Creates the config directory if it does not exist.
    pub fn save(&self, paths: &BusytokPaths) -> Result<()> {
        let config_dir = paths.config_dir();
        std::fs::create_dir_all(config_dir)
            .with_context(|| format!("failed to create config dir {}", config_dir.display()))?;

        let file_path = config_dir.join(SETTINGS_FILE_NAME);
        let toml_str =
            toml::to_string_pretty(self).context("failed to serialize settings to TOML")?;

        atomic_write(&file_path, &toml_str)?;

        Ok(())
    }

    /// Load settings from a specific file path (for testing).
    pub fn load_from_file(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;

        match toml::from_str::<Self>(&contents) {
            Ok(settings) => Ok(settings),
            Err(e) => {
                warn!(
                    "Corrupt settings file {}: {e}; falling back to defaults",
                    path.display()
                );
                Ok(Self::default())
            }
        }
    }

    /// Parse settings from a TOML string (no filesystem canonicalization/validation).
    /// Used by tests; mirrors `load_from_file`.
    pub fn load_from_str(toml: &str) -> Result<Self> {
        let s: Self = toml::from_str(toml)?;
        Ok(s)
    }

    /// Save settings to a specific file path (for testing).
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create dir {}", parent.display()))?;
        }

        let toml_str =
            toml::to_string_pretty(self).context("failed to serialize settings to TOML")?;

        atomic_write(path, &toml_str)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_settings_match_domain_timezone() {
        let settings = BusytokSettings::default();
        // Default is now an IANA name, not a fixed offset
        let rtz = busytok_domain::ReportingTimezone::parse(&settings.timezone).unwrap();
        assert!(!rtz.canonical_name().is_empty());
        assert_eq!(settings.week_starts_on, 1);
        assert!(settings.privacy.local_only);
        assert!(settings.privacy.redact_sensitive_values);
        assert!(settings.discovery.claude_code_default_paths);
        assert!(settings.discovery.codex_default_paths);
        assert!(settings.discovery.manual_roots.is_empty());
        assert!(matches!(
            settings.prompt_palette_default_action,
            PromptDefaultAction::CopyAndPaste
        ));
    }

    #[test]
    fn load_returns_defaults_when_file_missing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.toml");
        let settings = BusytokSettings::load_from_file(&path).unwrap();
        let rtz = busytok_domain::ReportingTimezone::parse(&settings.timezone).unwrap();
        assert!(!rtz.canonical_name().is_empty());
        assert!(settings.discovery.claude_code_default_paths);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");

        let settings = BusytokSettings {
            timezone: "Etc/UTC".to_string(),
            week_starts_on: 0,
            privacy: PrivacySettings {
                local_only: false,
                redact_sensitive_values: false,
            },
            discovery: DiscoverySettings {
                claude_code_default_paths: true,
                codex_default_paths: true,
                manual_roots: vec![],
            },
            prompt_palette_default_action: PromptDefaultAction::OnlyCopy,
            subagent: SubagentSettings::default(),
        };

        settings.save_to_file(&path).unwrap();
        let loaded = BusytokSettings::load_from_file(&path).unwrap();

        assert_eq!(loaded.timezone, "Etc/UTC");
        assert_eq!(loaded.week_starts_on, 0);
        assert!(!loaded.privacy.local_only);
        assert!(!loaded.privacy.redact_sensitive_values);
        assert!(loaded.discovery.claude_code_default_paths);
        assert!(loaded.discovery.codex_default_paths);
        assert!(loaded.discovery.manual_roots.is_empty());
        assert!(matches!(
            loaded.prompt_palette_default_action,
            PromptDefaultAction::OnlyCopy
        ));
    }

    #[test]
    fn load_corrupt_file_returns_defaults() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        std::fs::write(&path, "this is not valid toml {{{}").unwrap();

        let settings = BusytokSettings::load_from_file(&path).unwrap();
        // Should fall back to defaults when TOML is corrupt.
        let rtz = busytok_domain::ReportingTimezone::parse(&settings.timezone).unwrap();
        assert!(!rtz.canonical_name().is_empty());
        assert!(settings.discovery.claude_code_default_paths);
    }

    #[test]
    fn load_legacy_discovery_settings_fills_missing_fields_with_defaults() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        std::fs::write(
            &path,
            r#"timezone = "UTC"

[discovery]
claude_code_default_paths = true
codex_default_paths = false
"#,
        )
        .unwrap();

        let settings = BusytokSettings::load_from_file(&path).unwrap();
        assert_eq!(settings.timezone, "UTC");
        assert_eq!(settings.week_starts_on, 1);
        assert!(settings.discovery.claude_code_default_paths);
        assert!(!settings.discovery.codex_default_paths);
        assert!(settings.discovery.manual_roots.is_empty());
    }

    #[test]
    fn save_creates_parent_directory() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nested/sub/dir/settings.toml");

        let settings = BusytokSettings::default();
        settings.save_to_file(&path).unwrap();

        assert!(path.exists());
        let loaded = BusytokSettings::load_from_file(&path).unwrap();
        let rtz = busytok_domain::ReportingTimezone::parse(&loaded.timezone).unwrap();
        assert!(!rtz.canonical_name().is_empty());
    }

    #[test]
    fn saved_toml_is_human_readable() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");

        let settings = BusytokSettings {
            timezone: "Etc/UTC".to_string(),
            ..Default::default()
        };

        settings.save_to_file(&path).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();

        // Verify it looks like readable TOML.
        assert!(contents.contains("timezone = \"Etc/UTC\""));
        assert!(contents.contains("[discovery]"));
        assert!(contents.contains("claude_code_default_paths = true"));
        assert!(contents.contains("codex_default_paths = true"));
        assert!(contents.contains("week_starts_on = 1"));
        assert!(contents.contains("[privacy]"));
    }

    #[test]
    fn load_via_paths_returns_defaults_when_no_file() {
        // BusytokPaths::new() resolves to real system dirs, so we use
        // load_from_file as a proxy to verify the same semantics.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        let settings = BusytokSettings::load_from_file(&path).unwrap();
        let rtz = busytok_domain::ReportingTimezone::parse(&settings.timezone).unwrap();
        assert!(!rtz.canonical_name().is_empty());
    }
}
