//! Source discovery orchestration.
//!
//! Merges default-path sources, settings manual roots, and DB-configured
//! user roots into a deduplicated list of [`DiscoveredLogSource`]s.
//! Pure discovery — no DB writes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

use busytok_config::BusytokSettings;
use busytok_discovery::{ClaudeCodeDiscovery, CodexDiscovery, DiscoveredLogSource};
use busytok_domain::AgentKind;
use busytok_events::AppEventBus;
use busytok_store::Database;

pub(crate) struct SourceRegistry {
    settings: Arc<Mutex<BusytokSettings>>,
    db: Arc<Mutex<Database>>,
    event_bus: Arc<AppEventBus>,
}

impl SourceRegistry {
    pub fn new(
        settings: Arc<Mutex<BusytokSettings>>,
        db: Arc<Mutex<Database>>,
        event_bus: Arc<AppEventBus>,
    ) -> Self {
        Self {
            settings,
            db,
            event_bus,
        }
    }

    /// Full discovery: default roots + settings manual roots + DB user-configured roots.
    /// Merged and deduplicated by source_id with deterministic ordering.
    pub fn discover_all(&self) -> Result<Vec<DiscoveredLogSource>> {
        let settings = self.settings.lock().unwrap();

        let mut sources = BTreeMap::new();
        for s in self.discover_default_sources(&settings)? {
            sources.entry(s.source_id.clone()).or_insert(s);
        }
        for s in self.discover_manual_roots(&settings) {
            sources.entry(s.source_id.clone()).or_insert(s);
        }
        for s in self.discover_db_configured_roots()? {
            sources.entry(s.source_id.clone()).or_insert(s);
        }

        Ok(sources.into_values().collect())
    }

    /// Discover sources for a specific user-configured root path.
    pub fn discover_configured_root(
        &self,
        agent: AgentKind,
        root_path: &Path,
    ) -> Result<Vec<DiscoveredLogSource>> {
        self.discover_roots_for_agent(agent, vec![root_path.to_path_buf()])
    }
}

// ── Private helpers ────────────────────────────────────────────────────

impl SourceRegistry {
    fn discover_default_sources(
        &self,
        settings: &BusytokSettings,
    ) -> Result<Vec<DiscoveredLogSource>> {
        let mut sources = Vec::new();

        let claude_discovery =
            ClaudeCodeDiscovery::with_settings(settings.discovery.claude_code_default_paths);
        sources.extend(
            claude_discovery
                .discover()
                .context("failed to discover Claude Code log sources")?,
        );

        if settings.discovery.codex_default_paths {
            let codex_discovery =
                CodexDiscovery::with_settings(settings.discovery.codex_default_paths);
            sources.extend(
                codex_discovery
                    .discover()
                    .context("failed to discover Codex log sources")?,
            );
        }

        Ok(sources)
    }

    fn discover_manual_roots(&self, settings: &BusytokSettings) -> Vec<DiscoveredLogSource> {
        let mut sources = Vec::new();

        let claude_roots: Vec<PathBuf> = settings
            .discovery
            .manual_roots
            .iter()
            .filter(|r| r.client_id == "claude_code")
            .map(|r| PathBuf::from(&r.root_path))
            .collect();
        if !claude_roots.is_empty() {
            sources.extend(self.discover_roots_best_effort(
                AgentKind::ClaudeCode,
                claude_roots,
                "Claude Code",
            ));
        }

        let codex_roots: Vec<PathBuf> = settings
            .discovery
            .manual_roots
            .iter()
            .filter(|r| r.client_id == "codex")
            .map(|r| PathBuf::from(&r.root_path))
            .collect();
        if !codex_roots.is_empty() {
            sources.extend(self.discover_roots_best_effort(AgentKind::Codex, codex_roots, "Codex"));
        }

        sources
    }

    /// Discover roots with best-effort semantics: on failure, publish an
    /// ephemeral error event, warn, and return empty — do not propagate.
    fn discover_roots_best_effort(
        &self,
        agent: AgentKind,
        roots: Vec<PathBuf>,
        label: &str,
    ) -> Vec<DiscoveredLogSource> {
        match self.discover_roots_for_agent(agent, roots) {
            Ok(s) => s,
            Err(e) => {
                let _ = self
                    .event_bus
                    .publish_ephemeral(busytok_events::AppEvent::Error {
                        message: format!("failed to discover {label} manual roots: {e:#}"),
                        source: Some("source_registry.manual_roots".to_string()),
                    });
                tracing::warn!(
                    event_code = "source_registry.manual_roots.failed",
                    error = %e,
                    "failed to discover {label} manual roots; skipping"
                );
                Vec::new()
            }
        }
    }

    fn discover_db_configured_roots(&self) -> Result<Vec<DiscoveredLogSource>> {
        let db = self.db.lock().unwrap();
        let user_roots =
            busytok_store::source_queries::list_active_user_configured_roots(db.conn())
                .context("failed to list active user-configured roots")?;

        let mut claude_roots: Vec<PathBuf> = Vec::new();
        let mut codex_roots: Vec<PathBuf> = Vec::new();

        for s in user_roots {
            match s.agent.parse::<AgentKind>() {
                Ok(AgentKind::ClaudeCode) => claude_roots.push(s.root_path),
                Ok(AgentKind::Codex) => codex_roots.push(s.root_path),
                _ => {
                    let _ = self
                        .event_bus
                        .publish_ephemeral(busytok_events::AppEvent::Error {
                            message: format!(
                                "Unknown agent type '{}' for user root: {}",
                                s.agent,
                                s.root_path.display()
                            ),
                            source: Some("source_registry.discovery".to_string()),
                        });
                    tracing::warn!(
                        event_code = "source_registry.unknown_agent",
                        agent = %s.agent,
                        root = %s.root_path.display(),
                        "unknown agent type for user root"
                    );
                }
            }
        }

        let mut sources = Vec::new();
        if !claude_roots.is_empty() {
            sources.extend(self.discover_roots_for_agent(AgentKind::ClaudeCode, claude_roots)?);
        }
        if !codex_roots.is_empty() {
            sources.extend(self.discover_roots_for_agent(AgentKind::Codex, codex_roots)?);
        }
        Ok(sources)
    }

    fn discover_roots_for_agent(
        &self,
        agent: AgentKind,
        roots: Vec<PathBuf>,
    ) -> Result<Vec<DiscoveredLogSource>> {
        match agent {
            AgentKind::ClaudeCode => ClaudeCodeDiscovery::from_roots(roots)
                .discover()
                .context("failed to discover user-configured Claude Code log sources"),
            AgentKind::Codex => CodexDiscovery::from_roots(roots)
                .discover()
                .context("failed to discover user-configured Codex log sources"),
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use busytok_store::Database;

    /// Build a SourceRegistry against a temp directory with no real agent
    /// installations, and with default discovery disabled so the tests are
    /// fully deterministic regardless of the developer's machine.
    fn make_registry(temp_root: &std::path::Path) -> SourceRegistry {
        let db = Database::open_in_memory().expect("db");
        let mut settings = BusytokSettings::default();
        // Disable default-path discovery — tests seed roots explicitly.
        settings.discovery.claude_code_default_paths = false;
        settings.discovery.codex_default_paths = false;
        // Register a manual root to exercise that code path.
        settings.discovery.manual_roots = vec![busytok_config::ManualRootConfig {
            id: "test-manual-root".to_string(),
            client_id: "claude_code".to_string(),
            root_path: temp_root.display().to_string(),
        }];
        SourceRegistry {
            settings: Arc::new(Mutex::new(settings)),
            db: Arc::new(Mutex::new(db)),
            event_bus: Arc::new(AppEventBus::new(8)),
        }
    }

    #[test]
    fn discover_all_with_disabled_defaults_and_empty_manual_root_returns_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = make_registry(dir.path());
        let sources = registry.discover_all().expect("discover_all");
        // Default discovery disabled + manual root pointing to empty dir = 0 sources.
        assert!(
            sources.is_empty(),
            "expected empty sources from disabled defaults + empty manual dir"
        );
    }

    #[test]
    fn discover_all_dedupes_when_db_and_manual_overlap() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Create a minimal Claude Code project directory with a .jsonl
        // file so discovery actually produces a source from the root.
        let agent_dir = dir.path().join("projects").join("-server-agent");
        std::fs::create_dir_all(&agent_dir).expect("create agent dir");
        std::fs::write(
            agent_dir.join("agent-server-2025-07-15.jsonl"),
            "{\"type\":\"message\"}\n",
        )
        .expect("write dummy jsonl");

        let root_str = dir.path().display().to_string();

        // Seed DB with the same root — DB path uses s.root_path directly.
        let db = Database::open_in_memory().expect("db");
        db.conn()
            .execute(
                "INSERT INTO log_sources \
             (id, agent, source_type, root_path, status, \
              configured_by_user, first_seen_at_ms, last_seen_at_ms, \
              created_at_ms, updated_at_ms) \
             VALUES ('dup-db', 'claude_code', 'jsonl', ?1, 'active', 1, \
                     1000, 1000, 1000, 1000)",
                [root_str.as_str()],
            )
            .expect("seed");

        let mut settings = BusytokSettings::default();
        settings.discovery.claude_code_default_paths = false;
        settings.discovery.codex_default_paths = false;
        // Same root as the DB row — discovery produces the same source_id.
        settings.discovery.manual_roots = vec![busytok_config::ManualRootConfig {
            id: "dup-manual".to_string(),
            client_id: "claude_code".to_string(),
            root_path: root_str.clone(),
        }];

        let registry = SourceRegistry {
            settings: Arc::new(Mutex::new(settings)),
            db: Arc::new(Mutex::new(db)),
            event_bus: Arc::new(AppEventBus::new(8)),
        };

        let sources = registry.discover_all().expect("discover_all");
        let source_ids: Vec<&str> = sources.iter().map(|s| s.source_id.as_str()).collect();

        assert_eq!(
            source_ids.len(),
            1,
            "overlapping DB and manual root must produce exactly 1 source, \
             got {} sources: {source_ids:?}",
            source_ids.len(),
        );
    }

    #[test]
    fn discover_configured_root_for_empty_temp_dir_returns_empty_sources() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Create an empty subdirectory as the "root" so the path exists
        // but contains no log files.
        let root = dir.path().join("empty-agent-dir");
        std::fs::create_dir(&root).expect("create empty dir");

        let registry = make_registry(dir.path());
        let sources = registry
            .discover_configured_root(AgentKind::ClaudeCode, &root)
            .expect("discover_configured_root on an empty dir should succeed");
        assert!(sources.is_empty(), "empty dir should yield zero sources");
    }
}
