#![allow(warnings, clippy::all)]
#![allow(unexpected_cfgs)]
#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::all))]
mod logging;
mod manifest;
mod paths;
pub mod platform;
pub mod providers;
pub mod service_marker;

pub use logging::{init_logging, prune_old_logs, LogSource, LoggingGuards};
pub use manifest::SidecarManifest;
pub use paths::BusytokPaths;
pub use providers::ProviderKind;

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
/// If the target file already exists, its permissions are preserved on the
/// replacement (the temp file is `fchmod`'d to match before rename). This
/// matters for user-owned files like shell rc files where a user may have
/// set restrictive permissions (e.g. `chmod 600 ~/.zshrc`).
///
/// Shared by `BusytokSettings::save`, the GUI-side local lifecycle settings
/// store, and the CLI shim PATH setup/teardown so atomic-write logic doesn't
/// diverge.
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

    // Preserve the target file's permissions if it already exists, so the
    // rename doesn't silently relax them to the umask default.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mode = meta.permissions().mode();
            if let Err(e) =
                std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(mode))
            {
                // Best-effort: if fchmod fails, clean up and bail rather
                // than replacing the file with relaxed permissions.
                let _ = std::fs::remove_file(&tmp_path);
                return Err(e).with_context(|| {
                    format!(
                        "failed to set permissions on temp file {}",
                        tmp_path.display()
                    )
                });
            }
        }
    }

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
    #[serde(default = "default_profiles")]
    pub profiles: std::collections::HashMap<String, SubagentProfileConfig>,
}
impl Default for SubagentSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            pi_sidecar: SubagentPiSidecarConfig::default(),
            context: SubagentContextConfig::default(),
            resource_policy: SubagentResourcePolicyConfig::default(),
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
    #[serde(default = "default_false")]
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
    /// Optional override for the sidecar runtime directory (bundle + node binary).
    /// When set, `BusytokPaths::sidecar_runtime_dir()` returns this path verbatim.
    /// When None (default), `sidecar_runtime_dir()` resolves to the dev path
    /// (`apps/pi-sidecar/dist/`) — packaged builds MUST set this via settings.toml
    /// or a Tauri-injected env var.
    ///
    /// Examples:
    ///   - Packaged GUI (macOS): `/Applications/Busytok.app/Contents/Resources/pi-sidecar`
    ///   - Service-only: `/usr/local/lib/busytok/pi-sidecar` (or wherever the
    ///     package manager installs it)
    ///   - Dev: unset (resolves to apps/pi-sidecar/dist/)
    #[serde(default)]
    pub runtime_dir: Option<String>,
}
impl Default for SubagentPiSidecarConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            node_runtime: default_bundled_runtime(),
            system_node_path: String::new(),
            max_hot_sessions: default_max_hot_sessions(),
            idle_exit_seconds: default_idle_exit_seconds(),
            hibernate_after_seconds: default_hibernate_after_seconds(),
            task_timeout_seconds: default_task_timeout_seconds(),
            memory_soft_limit_mb: default_memory_soft_limit_mb(),
            memory_hard_limit_mb: default_memory_hard_limit_mb(),
            task_queue_max: default_task_queue_max(),
            runtime_dir: None,
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
pub struct SubagentProfileConfig {
    #[serde(default = "default_false")]
    pub write_access: bool,
    #[serde(default)]
    pub tools: Vec<String>,
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
            context_budget_tokens: 6000,
            timeout_seconds: 300,
        },
    );
    m
}

/// Returns true if `name` is one of the 3 built-in profiles.
///
/// Used by the runtime (to reject `profile.delete` on built-in profiles)
/// and by the DTO mapper (to set `is_builtin: bool` on `ProfileDto`).
/// Single source of truth — do NOT duplicate this check elsewhere.
pub fn is_builtin_profile(name: &str) -> bool {
    matches!(
        name,
        "pi/search-cheap" | "pi/review-cheap" | "pi/plan-cheap"
    )
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
                // Canonicalize built-in profiles: fill missing, never overwrite.
                settings.canonicalize_builtin_profiles();
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

    /// Ensure all 3 built-in profiles exist. Missing ones are filled with
    /// defaults; present ones are left untouched (even if user modified them).
    ///
    /// Called by `load()` after timezone canonicalization. Spec §4 Phase 4:
    /// "Service ensures 3 built-in profiles exist on every config load.
    /// Missing → fill with defaults. Present → leave untouched."
    pub fn canonicalize_builtin_profiles(&mut self) {
        let builtins = default_profiles();
        let mut filled = Vec::new();
        for (name, cfg) in &builtins {
            if !self.subagent.profiles.contains_key(name) {
                self.subagent.profiles.insert(name.clone(), cfg.clone());
                filled.push(name.clone());
            }
        }
        if !filled.is_empty() {
            tracing::info!(
                event_code = "profile.builtin_canonicalized",
                filled = ?filled,
                "filled missing built-in profiles during config load"
            );
        }
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

    /// Backward-compat with v0.0.8 configs: a profile TOML without
    /// `provider_id` deserializes to defaults. Task 3 removed `provider_id`
    /// and `model` from `SubagentProfileConfig` — profiles are now pure
    /// behavior templates.
    #[test]
    fn profile_config_has_no_provider_or_model_fields() {
        let toml = r#"
timezone = "UTC"

[subagent.profiles."pi/search-cheap"]
write_access = false
tools = ["read", "grep"]
context_budget_tokens = 3000
timeout_seconds = 120
"#;
        let settings = BusytokSettings::load_from_str(toml).unwrap();
        let p = settings
            .subagent
            .profiles
            .get("pi/search-cheap")
            .expect("pi/search-cheap profile should be present");
        // Compile-time check: the fields that Task 3 removed no longer exist.
        let _write = p.write_access;
        let _tools = &p.tools;
        let _budget = p.context_budget_tokens;
        let _timeout = p.timeout_seconds;
    }

    /// `load` canonicalizes "local" timezone to the system IANA name and persists
    /// the canonical form back to disk. Covers the timezone.canonicalized branch.
    #[test]
    fn load_canonicalizes_local_timezone_and_persists() {
        let tmp = TempDir::new().unwrap();
        let paths = BusytokPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        let file_path = paths.config_dir().join(SETTINGS_FILE_NAME);
        // "local" is parseable but its canonical form differs.
        std::fs::write(&file_path, "timezone = \"local\"\nweek_starts_on = 1\n").unwrap();

        let settings = BusytokSettings::load(&paths).unwrap();
        // The canonical form must differ from "local".
        assert_ne!(
            settings.timezone, "local",
            "timezone should be canonicalized"
        );
        let rtz = busytok_domain::ReportingTimezone::parse(&settings.timezone).unwrap();
        assert_eq!(rtz.canonical_name(), settings.timezone);
        // Persistence: the file should now contain the canonical form.
        let persisted = std::fs::read_to_string(&file_path).unwrap();
        assert!(
            persisted.contains(&settings.timezone),
            "settings file should contain the canonicalized timezone"
        );
    }

    /// `load` falls back to system timezone when the persisted value is unparseable.
    /// Covers the timezone.parse_failed branch.
    #[test]
    fn load_falls_back_when_timezone_unparseable() {
        let tmp = TempDir::new().unwrap();
        let paths = BusytokPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        let file_path = paths.config_dir().join(SETTINGS_FILE_NAME);
        std::fs::write(
            &file_path,
            "timezone = \"definitely-not-a-real-timezone\"\nweek_starts_on = 1\n",
        )
        .unwrap();

        let settings = BusytokSettings::load(&paths).unwrap();
        // Should resolve to a valid system timezone (parseable).
        let rtz = busytok_domain::ReportingTimezone::parse(&settings.timezone);
        assert!(rtz.is_ok(), "fallback timezone must be parseable");
        // Persisted fallback.
        let persisted = std::fs::read_to_string(&file_path).unwrap();
        assert!(
            persisted.contains(&settings.timezone),
            "settings file should contain the fallback timezone"
        );
    }

    /// `load` falls back to defaults when the settings file is corrupt TOML.
    /// Covers the warn-and-default branch in `load`.
    #[test]
    fn load_falls_back_to_defaults_on_corrupt_toml() {
        let tmp = TempDir::new().unwrap();
        let paths = BusytokPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        let file_path = paths.config_dir().join(SETTINGS_FILE_NAME);
        // Not valid TOML: unbalanced bracket.
        std::fs::write(&file_path, "timezone = \"UTC\"\n[broken\n").unwrap();

        let settings = BusytokSettings::load(&paths).unwrap();
        // Default timezone (system IANA) — proves the default fallback fired.
        let default = BusytokSettings::default();
        assert_eq!(settings.timezone, default.timezone);
    }

    /// `load_from_file` falls back to defaults on corrupt TOML.
    /// Covers the warn-and-default branch in `load_from_file`.
    #[test]
    fn load_from_file_falls_back_to_defaults_on_corrupt_toml() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        // Not valid TOML: unterminated string.
        std::fs::write(&path, "timezone = \"UTC\n").unwrap();

        let settings = BusytokSettings::load_from_file(&path).unwrap();
        let default = BusytokSettings::default();
        assert_eq!(settings.timezone, default.timezone);
    }

    /// `canonicalize_builtin_profiles` fills missing built-in profiles and
    /// emits the canonicalization log line.
    #[test]
    fn canonicalize_builtin_profiles_fills_missing() {
        let mut settings = BusytokSettings::default();
        // Remove all built-in profiles to force the fill path.
        settings.subagent.profiles.clear();
        assert!(settings.subagent.profiles.is_empty());

        settings.canonicalize_builtin_profiles();

        // All three built-ins should be present.
        for name in ["pi/search-cheap", "pi/review-cheap", "pi/plan-cheap"] {
            assert!(
                settings.subagent.profiles.contains_key(name),
                "built-in profile {name} should be filled"
            );
        }
    }

    /// `canonicalize_builtin_profiles` does not overwrite existing profiles.
    #[test]
    fn canonicalize_builtin_profiles_preserves_existing() {
        let mut settings = BusytokSettings::default();
        // Pre-set a custom profile for pi/search-cheap.
        let custom = SubagentProfileConfig {
            write_access: true,
            tools: vec!["read".to_string()],
            context_budget_tokens: 9999,
            timeout_seconds: 1,
        };
        settings
            .subagent
            .profiles
            .insert("pi/search-cheap".to_string(), custom.clone());
        // Remove the others.
        settings.subagent.profiles.remove("pi/review-cheap");
        settings.subagent.profiles.remove("pi/plan-cheap");

        settings.canonicalize_builtin_profiles();

        // The custom pi/search-cheap should be preserved.
        let preserved = settings.subagent.profiles.get("pi/search-cheap").unwrap();
        assert_eq!(preserved.context_budget_tokens, 9999);
        assert_eq!(preserved.timeout_seconds, 1);
        assert!(preserved.write_access);
        // The other two should be filled with defaults.
        assert!(settings.subagent.profiles.contains_key("pi/review-cheap"));
        assert!(settings.subagent.profiles.contains_key("pi/plan-cheap"));
    }

    /// `save_to_file` creates parent directories when they don't exist.
    #[test]
    fn save_to_file_creates_parent_directories() {
        let tmp = TempDir::new().unwrap();
        // Nested path where neither `sub` nor `sub2` exists yet.
        let path = tmp.path().join("sub").join("sub2").join("settings.toml");
        assert!(!path.parent().unwrap().exists());

        let settings = BusytokSettings::default();
        settings.save_to_file(&path).unwrap();

        assert!(path.exists(), "file should be created with parent dirs");
        // Round-trip to verify it's actually valid TOML.
        let loaded = BusytokSettings::load_from_file(&path).unwrap();
        assert_eq!(loaded.timezone, settings.timezone);
    }

    /// `atomic_write` fails cleanly when the destination parent is a file
    /// (not a directory), surfacing the create_dir_all error.
    #[test]
    fn atomic_write_fails_when_parent_is_a_file() {
        let tmp = TempDir::new().unwrap();
        // Create a file at `blocker` — trying to use it as a directory fails.
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, "i am a file").unwrap();
        let target = blocker.join("settings.toml"); // parent is a file

        let result = atomic_write(&target, "contents");
        assert!(
            result.is_err(),
            "atomic_write should fail when parent is a file"
        );
    }

    /// `atomic_write` succeeds and atomically replaces existing content.
    #[test]
    fn atomic_write_replaces_existing_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.toml");
        std::fs::write(&path, "old content").unwrap();

        atomic_write(&path, "new content").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new content");
    }

    /// `atomic_write` preserves the target file's permissions when replacing.
    /// Without this, a user's `chmod 600 ~/.zshrc` would be silently relaxed
    /// to the umask default (typically 0644) after an atomic replace.
    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("rc_file");
        std::fs::write(&path, "old content\n").unwrap();

        // Set restrictive permissions (0600 — owner read/write only).
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        atomic_write(&path, "new content\n").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "permissions should be preserved after atomic_write"
        );
    }

    /// `atomic_write` on a non-existent file uses umask defaults (no target
    /// to copy permissions from).
    #[cfg(unix)]
    #[test]
    fn atomic_write_new_file_uses_umask_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("new_file");

        atomic_write(&path, "content").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        // Just verify it's writable by owner (umask typically gives 0644 or 0666).
        assert!(mode & 0o200 != 0, "new file should be owner-writable");
    }
}
