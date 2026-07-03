use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors from identity derivation operations.
#[derive(Debug, Clone, Error)]
pub enum IdentityError {
    #[error("path is empty")]
    EmptyPath,
    #[error("path is not absolute: {0}")]
    NotAbsolute(String),
    #[error("cannot canonicalize path: {0}")]
    CanonicalizeFailed(String),
}

/// Canonicalize and normalize a project path to a stable string.
///
/// This expands a leading `~` to the user's home directory, resolves `.` and
/// `..` segments, and ensures the path is absolute, producing a deterministic
/// representation for hashing.
pub fn normalize_project_path(path: impl AsRef<Path>) -> Result<String, IdentityError> {
    let path = path.as_ref();
    if path.as_os_str().is_empty() {
        return Err(IdentityError::EmptyPath);
    }
    // Expand leading `~` to the user's home directory so that tilde-prefixed
    // paths are normalized consistently across environments (e.g. Claude Code
    // may report cwd as `~/projects/foo`).
    let path = expand_tilde(path);
    // Use PathBuf::canonicalize-style normalization without hitting the filesystem.
    // We normalize by cleaning the path components.
    let normalized = normalize_path_components(&path);
    if !normalized.is_absolute() {
        return Err(IdentityError::NotAbsolute(normalized.display().to_string()));
    }
    Ok(normalized.display().to_string())
}

/// Expand a leading `~` in a path to the user's home directory.
///
/// Tries `dirs::home_dir()` first, falling back to the `HOME` environment
/// variable. If neither is available the path is returned unchanged.
fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.as_os_str();
    if s == "~" {
        return home_dir().unwrap_or_else(|| path.to_path_buf());
    }
    let bytes = s.as_encoded_bytes();
    if bytes.starts_with(b"~/") || bytes.starts_with(b"~\\") {
        if let Some(home) = home_dir() {
            let rest = &bytes[2..];
            // SAFETY: `rest` is a valid sub-slice of the original OS string
            // bytes, which was valid UTF-8 on Unix and valid WTF-8 on
            // Windows. Reconstructing via `from_encoded_bytes` is safe.
            #[cfg(unix)]
            let rest = unsafe { std::ffi::OsStr::from_encoded_bytes_unchecked(rest) };
            #[cfg(not(unix))]
            let rest = unsafe { std::ffi::OsStr::from_encoded_bytes_unchecked(rest) };
            return home.join(rest);
        }
    }
    path.to_path_buf()
}

/// Return the current user's home directory.
///
/// Prefers `dirs::home_dir()`, falling back to the `HOME` environment
/// variable for environments where `dirs` may not find the home directory.
fn home_dir() -> Option<PathBuf> {
    dirs::home_dir().or_else(|| std::env::var("HOME").ok().map(PathBuf::from))
}

/// Clean path components by resolving `.` and `..` segments without
/// touching the filesystem. This mirrors `std::fs::canonicalize` semantics
/// for path component cleanup but works on non-existent paths.
fn normalize_path_components(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(p) => {
                result.clear();
                result.push(p.as_os_str());
            }
            std::path::Component::RootDir => {
                result.clear();
                result.push(std::path::Component::RootDir);
            }
            std::path::Component::CurDir => {
                // Skip current directory markers
            }
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::Normal(c) => {
                result.push(c);
            }
        }
    }
    result
}

/// Derive a deterministic SHA-256 hash from a normalized project path.
pub fn derive_project_hash(normalized_path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(normalized_path.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

/// Derive a session ID, falling back to the source file ID when no
/// source session ID is available.
pub fn derive_session_id(source_session_id: Option<&str>, source_file_id: &str) -> String {
    source_session_id
        .filter(|s| !s.is_empty())
        .unwrap_or(source_file_id)
        .to_string()
}

/// Compute a short hash (first 12 hex chars of SHA-256) from an input string.
///
/// Useful for generating compact, collision-resistant identifiers from
/// arbitrary strings (e.g. project paths, source IDs).
pub fn hash_short(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)[..12].to_string()
}

/// A metadata-only fingerprint used to compute `raw_event_hash`.
///
/// This fingerprint captures only the identity and token fields from an event.
/// It never includes prompt, response, tool arguments, or other content fields.
/// The persisted `raw_event_hash` database field is populated from this
/// canonical metadata-only representation.
#[derive(Debug, Clone)]
pub struct MetadataFingerprint {
    agent: String,
    session_id: String,
    turn_id: Option<String>,
    request_id: Option<String>,
    message_id: Option<String>,
    input_tokens: i64,
    output_tokens: i64,
    total_tokens: i64,
    timestamp_ms: i64,
    /// When true, token counts are excluded from the hash computation.
    /// Used for fallback replacement identities where delayed-token
    /// completion causes the same logical event to carry different token
    /// counts over time.
    exclude_tokens: bool,
    /// Content that must be explicitly excluded from the hash.
    /// Tracked here so the builder API can demonstrate that adding
    /// ignored content does not change the hash.
    _ignored: Option<String>,
}

impl MetadataFingerprint {
    pub fn new(agent: &str, session_id: &str) -> Self {
        Self {
            agent: agent.to_string(),
            session_id: session_id.to_string(),
            turn_id: None,
            request_id: None,
            message_id: None,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            timestamp_ms: 0,
            exclude_tokens: false,
            _ignored: None,
        }
    }

    pub fn turn_id(mut self, id: &str) -> Self {
        self.turn_id = Some(id.to_string());
        self
    }

    pub fn request_id(mut self, id: &str) -> Self {
        self.request_id = Some(id.to_string());
        self
    }

    pub fn message_id(mut self, id: &str) -> Self {
        self.message_id = Some(id.to_string());
        self
    }

    pub fn tokens(mut self, input: i64, output: i64) -> Self {
        self.input_tokens = input;
        self.output_tokens = output;
        self
    }

    pub fn total_tokens(mut self, total: i64) -> Self {
        self.total_tokens = total;
        self
    }

    pub fn timestamp_ms(mut self, ts: i64) -> Self {
        self.timestamp_ms = ts;
        self
    }

    /// Exclude mutable token fields from the hash computation.
    ///
    /// For fallback replacement identities, token counts change
    /// across updates to the same
    /// logical event. Setting this flag ensures the hash remains stable
    /// regardless of token count mutations, so the event is replaced
    /// rather than duplicated.
    pub fn tokens_excluded(mut self) -> Self {
        self.exclude_tokens = true;
        self
    }

    /// Returns whether token fields are excluded from the hash.
    pub fn is_tokens_excluded(&self) -> bool {
        self.exclude_tokens
    }

    /// Attach content that must not affect the hash.
    /// This exists to verify that ignored content is excluded from
    /// the canonical representation.
    pub fn ignored_content(mut self, content: &str) -> Self {
        self._ignored = Some(content.to_string());
        self
    }
}

/// Compute a deterministic hash from a metadata-only fingerprint.
///
/// The hash covers identity fields (agent, session_id, turn_id,
/// request_id, message_id) and, when `tokens_excluded()` is NOT set,
/// token fields (input_tokens, output_tokens, total_tokens).
/// Timestamp is intentionally excluded from the hash — the same logical
/// event scanned at different times must produce the same hash for
/// deduplication to work.
///
/// When the fingerprint has `tokens_excluded()` set, all token counts are
/// omitted from the hash, producing a stable identity for events whose
/// token fields may mutate across updates with the Replace write policy.
pub fn metadata_event_hash(fingerprint: &MetadataFingerprint) -> String {
    let mut hasher = Sha256::new();
    hasher.update(fingerprint.agent.as_bytes());
    hasher.update(b"\x00");
    hasher.update(fingerprint.session_id.as_bytes());
    hasher.update(b"\x00");
    if let Some(ref tid) = fingerprint.turn_id {
        hasher.update(tid.as_bytes());
    }
    hasher.update(b"\x00");
    if let Some(ref rid) = fingerprint.request_id {
        hasher.update(rid.as_bytes());
    }
    hasher.update(b"\x00");
    if let Some(ref mid) = fingerprint.message_id {
        hasher.update(mid.as_bytes());
    }
    hasher.update(b"\x00");
    if !fingerprint.exclude_tokens {
        hasher.update(fingerprint.input_tokens.to_le_bytes());
        hasher.update(fingerprint.output_tokens.to_le_bytes());
        hasher.update(fingerprint.total_tokens.to_le_bytes());
    }
    let result = hasher.finalize();
    hex::encode(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_identity_different_timestamps_same_hash() {
        let fp1 = MetadataFingerprint::new("test_agent", "session_1")
            .request_id("req_1")
            .message_id("msg_1")
            .tokens(100, 50)
            .total_tokens(150)
            .timestamp_ms(1_000_000);
        let fp2 = MetadataFingerprint::new("test_agent", "session_1")
            .request_id("req_1")
            .message_id("msg_1")
            .tokens(100, 50)
            .total_tokens(150)
            .timestamp_ms(2_000_000);

        assert_eq!(
            metadata_event_hash(&fp1),
            metadata_event_hash(&fp2),
            "hash must be identical regardless of timestamp difference"
        );
    }

    #[test]
    fn different_identity_different_hash() {
        let fp1 = MetadataFingerprint::new("agent_a", "session_1")
            .request_id("req_1")
            .message_id("msg_1")
            .tokens(100, 50)
            .total_tokens(150);
        let fp2 = MetadataFingerprint::new("agent_b", "session_1")
            .request_id("req_1")
            .message_id("msg_1")
            .tokens(100, 50)
            .total_tokens(150);

        assert_ne!(
            metadata_event_hash(&fp1),
            metadata_event_hash(&fp2),
            "different agents must produce different hashes"
        );
    }

    #[test]
    fn tokens_excluded_ignores_all_token_fields() {
        // When tokens_excluded is set, different token values produce the same hash.
        let fp1 = MetadataFingerprint::new("agent", "session")
            .request_id("req")
            .message_id("msg")
            .tokens(100, 50)
            .total_tokens(150)
            .tokens_excluded();
        let fp2 = MetadataFingerprint::new("agent", "session")
            .request_id("req")
            .message_id("msg")
            .tokens(200, 99)
            .total_tokens(299)
            .tokens_excluded();

        assert_eq!(
            metadata_event_hash(&fp1),
            metadata_event_hash(&fp2),
            "hash must be same when tokens_excluded even if token values differ"
        );
    }

    #[test]
    fn without_tokens_excluded_token_values_affect_hash() {
        // Without tokens_excluded, different token values produce different hashes.
        let fp1 = MetadataFingerprint::new("agent", "session")
            .request_id("req")
            .message_id("msg")
            .tokens(100, 50)
            .total_tokens(150);
        let fp2 = MetadataFingerprint::new("agent", "session")
            .request_id("req")
            .message_id("msg")
            .tokens(200, 99)
            .total_tokens(299);

        assert_ne!(
            metadata_event_hash(&fp1),
            metadata_event_hash(&fp2),
            "hash must differ when tokens are included and token values differ"
        );
    }

    // -- normalize_project_path tests --

    #[test]
    fn normalize_project_path_rejects_empty() {
        let err = normalize_project_path("").unwrap_err();
        assert!(matches!(err, IdentityError::EmptyPath));
    }

    #[test]
    fn normalize_project_path_rejects_relative() {
        let err = normalize_project_path("relative/path").unwrap_err();
        assert!(matches!(err, IdentityError::NotAbsolute(_)));
    }

    #[test]
    fn normalize_project_path_rejects_dot_relative() {
        // "./foo" starts with a CurDir component — normalize_path_components
        // strips it but the result remains relative.
        let err = normalize_project_path("./foo").unwrap_err();
        assert!(matches!(err, IdentityError::NotAbsolute(_)));
    }

    #[test]
    fn normalize_project_path_rejects_parent_relative() {
        // "../foo" starts with a ParentDir component — pop on an empty result
        // is a no-op, leaving the path relative.
        let err = normalize_project_path("../foo").unwrap_err();
        assert!(matches!(err, IdentityError::NotAbsolute(_)));
    }

    #[test]
    #[cfg(unix)]
    fn normalize_project_path_accepts_absolute() {
        let result = normalize_project_path("/var/log/foo").unwrap();
        assert_eq!(result, "/var/log/foo");
    }

    #[test]
    #[cfg(unix)]
    fn normalize_project_path_strips_curdir_segments() {
        let result = normalize_project_path("/var/./log/foo").unwrap();
        assert_eq!(result, "/var/log/foo");
    }

    #[test]
    #[cfg(unix)]
    fn normalize_project_path_resolves_parent_segments() {
        let result = normalize_project_path("/var/log/../foo").unwrap();
        assert_eq!(result, "/var/foo");
    }

    #[test]
    #[cfg(unix)]
    fn normalize_project_path_handles_root_only() {
        let result = normalize_project_path("/").unwrap();
        assert_eq!(result, "/");
    }

    #[test]
    #[cfg(unix)]
    fn normalize_project_path_handles_root_with_parent_segments() {
        // "/.." should resolve to "/" (cannot go above root).
        let result = normalize_project_path("/..").unwrap();
        assert_eq!(result, "/");
    }

    #[test]
    #[cfg(unix)]
    fn normalize_project_path_expands_tilde() {
        // "~/foo" should be expanded using the home directory.
        let result = normalize_project_path("~/foo").unwrap();
        // The result must be absolute and end with foo.
        assert!(
            result.starts_with('/'),
            "tilde expansion must produce absolute path"
        );
        assert!(
            result.ends_with("/foo"),
            "result should preserve trailing path: {result}"
        );
        // Must not contain a literal `~`.
        assert!(!result.contains('~'), "tilde must be expanded: {result}");
    }

    #[test]
    #[cfg(unix)]
    fn normalize_project_path_expands_bare_tilde() {
        // "~" alone should be expanded to the home directory.
        let result = normalize_project_path("~").unwrap();
        assert!(
            result.starts_with('/'),
            "home dir must be absolute: {result}"
        );
        assert!(!result.contains('~'), "tilde must be expanded: {result}");
    }

    #[test]
    #[cfg(unix)]
    fn normalize_project_path_handles_complex_segments() {
        let result = normalize_project_path("/a/b/../c/./d/e/../f").unwrap();
        assert_eq!(result, "/a/c/d/f");
    }

    #[test]
    fn derive_project_hash_is_deterministic() {
        let h1 = derive_project_hash("/var/log/foo");
        let h2 = derive_project_hash("/var/log/foo");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64, "SHA-256 hex digest length");
    }

    #[test]
    fn derive_project_hash_differs_for_different_inputs() {
        let h1 = derive_project_hash("/var/log/foo");
        let h2 = derive_project_hash("/var/log/bar");
        assert_ne!(h1, h2);
    }

    #[test]
    fn derive_session_id_prefers_source_session_id() {
        let result = derive_session_id(Some("session-123"), "file-456");
        assert_eq!(result, "session-123");
    }

    #[test]
    fn derive_session_id_falls_back_to_source_file_id_when_none() {
        let result = derive_session_id(None, "file-456");
        assert_eq!(result, "file-456");
    }

    #[test]
    fn derive_session_id_falls_back_to_source_file_id_when_empty() {
        let result = derive_session_id(Some(""), "file-456");
        assert_eq!(result, "file-456");
    }

    #[test]
    fn hash_short_returns_12_chars() {
        let h = hash_short("some-input");
        assert_eq!(h.len(), 12);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_short_differs_for_different_inputs() {
        let h1 = hash_short("input-a");
        let h2 = hash_short("input-b");
        assert_ne!(h1, h2);
    }
}
