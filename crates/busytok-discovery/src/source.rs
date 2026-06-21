use busytok_domain::{AgentKind, LogSourceType};
use std::path::PathBuf;

/// A log source discovered on the local filesystem by a discovery pass.
///
/// This is the output of a discovery scan — it describes *where* candidate
/// log files live, but does NOT read their contents.
#[derive(Debug, Clone)]
pub struct DiscoveredLogSource {
    /// Which agent produced this log source.
    pub agent: AgentKind,

    /// Stable identifier for this source (e.g. derived from root path).
    pub source_id: String,

    /// The root directory that was scanned (e.g. `~/.claude`).
    pub root_path: PathBuf,

    /// Candidate log files found under this source.
    pub files: Vec<PathBuf>,

    /// The storage format of the log files.
    pub source_type: LogSourceType,

    /// Whether this source was explicitly configured by the user
    /// (as opposed to auto-discovered from default paths).
    pub configured_by_user: bool,
}
