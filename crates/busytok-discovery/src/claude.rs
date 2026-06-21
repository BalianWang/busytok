use crate::source::DiscoveredLogSource;
use anyhow::Result;
use busytok_domain::{AgentKind, LogSourceType};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Discovers Claude Code log directories and candidate `.jsonl` files.
///
/// # Default roots
///
/// When constructed via [`ClaudeCodeDiscovery::default_roots`], the following
/// directories are scanned:
///
/// - `~/.claude`
/// - `~/.config/claude`
/// - The value of the `CLAUDE_CONFIG_DIR` environment variable, if set.
///
/// # Scan rules
///
/// Files matching `projects/**/*.jsonl` are collected. Unreadable files are
/// silently skipped (diagnostics may be emitted by the runtime layer later).
/// Deduplication is performed by canonical path when possible, falling back
/// to path-based identity.
pub struct ClaudeCodeDiscovery {
    roots: Vec<PathBuf>,
    configured_by_user: bool,
}

impl ClaudeCodeDiscovery {
    /// Build a discovery that scans the given root directories.
    ///
    /// This is intended for tests or for user-configured paths. When using
    /// default auto-discovery, prefer [`ClaudeCodeDiscovery::default_roots`].
    pub fn from_roots(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        let roots = roots.into_iter().collect();
        Self {
            roots,
            configured_by_user: true,
        }
    }

    /// Build a discovery using the standard default root directories.
    ///
    /// See the type-level documentation for the list of default roots.
    pub fn default_roots() -> Self {
        Self::with_settings(true)
    }

    /// Build a discovery based on settings flags.
    ///
    /// When `claude_code_default_paths` is true, the standard default root
    /// directories are scanned. When false, no default roots are included
    /// (only explicitly configured roots would be added via `from_roots`).
    pub fn with_settings(claude_code_default_paths: bool) -> Self {
        let mut roots = Vec::new();

        if claude_code_default_paths {
            // ~/.claude
            if let Some(home) = dirs::home_dir() {
                let p = home.join(".claude");
                if p.is_dir() {
                    roots.push(p);
                }
            }

            // ~/.config/claude
            if let Some(config_dir) = dirs::config_dir() {
                let p = config_dir.join("claude");
                if p.is_dir() {
                    roots.push(p);
                }
            }

            // CLAUDE_CONFIG_DIR
            if let Ok(env_dir) = std::env::var("CLAUDE_CONFIG_DIR") {
                let p = PathBuf::from(&env_dir);
                if p.is_dir() {
                    roots.push(p);
                }
            }
        }

        Self {
            roots,
            configured_by_user: false,
        }
    }

    /// Run the discovery scan and return all discovered log sources.
    ///
    /// Each unique root that contains at least one candidate `.jsonl` file
    /// produces a separate [`DiscoveredLogSource`]. Files are deduplicated
    /// by canonical path (or by display path if canonicalization fails).
    pub fn discover(&self) -> Result<Vec<DiscoveredLogSource>> {
        let mut sources = Vec::new();

        for root in &self.roots {
            let files = self.scan_root(root)?;
            if files.is_empty() {
                continue;
            }
            let source_id = derive_source_id(root);
            sources.push(DiscoveredLogSource {
                agent: AgentKind::ClaudeCode,
                source_id,
                root_path: root.clone(),
                files,
                source_type: LogSourceType::Jsonl,
                configured_by_user: self.configured_by_user,
            });
        }

        Ok(sources)
    }

    /// Scan a single root directory for `projects/**/*.jsonl` files.
    ///
    /// Deduplicates by canonical path when possible, then by display path.
    fn scan_root(&self, root: &Path) -> Result<Vec<PathBuf>> {
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut files: Vec<PathBuf> = Vec::new();

        // Walk the `projects` subdirectory if it exists.
        let projects_dir = root.join("projects");
        let walk_root = if projects_dir.is_dir() {
            &projects_dir
        } else {
            // If no `projects` subdirectory exists, nothing to find.
            return Ok(files);
        };

        for entry in WalkDir::new(walk_root).into_iter().filter_map(|e| e.ok()) {
            let path = entry.into_path();

            // Only consider .jsonl files.
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }

            // Deduplicate by canonical path, falling back to the path itself.
            let identity = path.canonicalize().unwrap_or_else(|_| path.clone());

            if seen.insert(identity) {
                files.push(path);
            }
        }

        // Sort for deterministic output.
        files.sort();

        Ok(files)
    }
}

/// Derive a stable source ID from a root path.
fn derive_source_id(root: &Path) -> String {
    let display = root.display().to_string();
    // Use a simple hash-like approach: replace path separators with underscores
    // and prefix with the agent name for a human-readable but unique-ish ID.
    let normalized = display.replace(['/', '\\'], "_");
    format!("claude_code_{}", normalized.trim_matches('_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_source_id_is_deterministic() {
        let id1 = derive_source_id(Path::new("/home/user/.claude"));
        let id2 = derive_source_id(Path::new("/home/user/.claude"));
        assert_eq!(id1, id2);
    }

    #[test]
    fn derive_source_id_differs_for_different_roots() {
        let id1 = derive_source_id(Path::new("/home/user/.claude"));
        let id2 = derive_source_id(Path::new("/home/user/.config/claude"));
        assert_ne!(id1, id2);
    }
}
