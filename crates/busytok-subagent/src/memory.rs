//! Memory merge and rule-based compaction (spec §6.2, §6.3).
//!
//! Pure functions over owned data — the caller handles DB I/O. The manager
//! reads `SubagentMemoryRow`, constructs a `MemoryUpdate` from the executor
//! output, and writes the returned row back via `subagent_upsert_memory`.

use busytok_config::SubagentContextConfig;
use busytok_store::{SubagentMemoryRow, SubagentTaskRow};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

const MAX_LONG_SUMMARY_CHARS: usize = 3000;
const MAX_DECISIONS: usize = 20;
const MAX_KEY_FILES: usize = 20;
const MAX_OPEN_QUESTIONS: usize = 10;
const MAX_ATTEMPTS: usize = 10;
const RECENT_TASKS_FOR_COMPACTION: usize = 5;
const OLD_LONG_SUMMARY_KEEP_CHARS: usize = 2000;
const RECENT_SUMMARIES_KEEP_CHARS: usize = 1000;

/// A tracked source file with a relevance score. Spec §3.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyFile {
    pub path: String,
    pub reason: String,
    pub last_seen_at_ms: i64,
    pub score: i64,
}

/// An open or resolved question. Spec §3.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenQuestion {
    pub question: String,
    /// "open" or "resolved".
    pub status: String,
    pub created_at_ms: i64,
    pub last_seen_at_ms: i64,
}

/// The delta returned by the sidecar after a turn (spec §4.3 result.memory_update).
/// `current_state_summary` becomes `hot_summary` (spec §6.2). When this entire
/// struct is `default()` (no memory_update emitted by the sidecar), `hot_summary`
/// is PRESERVED — a missing update must not destroy existing memory.
#[derive(Debug, Clone, Default)]
pub struct MemoryUpdate {
    pub current_state_summary: Option<String>,
    pub key_files: Vec<KeyFile>,
    pub decisions: Vec<String>,
    pub open_questions: Vec<OpenQuestion>,
}

/// Merges a `MemoryUpdate` into the current `SubagentMemoryRow`, applying
/// normalization (§6.3) and rule-based compaction (§6.2) when triggered.
pub struct MemoryUpdater {
    config: SubagentContextConfig,
}

impl MemoryUpdater {
    pub fn new(config: SubagentContextConfig) -> Self {
        Self { config }
    }

    /// Produce the next memory row. Pure — no DB I/O.
    ///
    /// - `recent_tasks`: DESC-ordered by `created_at_ms` (as returned by
    ///   `subagent_list_tasks`); `recent_tasks[0]` is the most recent. Used
    ///   for compaction's "recent findings" and for trigger (b) size estimate.
    /// - `tasks_since_last_compaction`: authoritative count of tasks with
    ///   `created_at_ms > last_compacted_at_ms`, from a dedicated store query.
    ///   NOT derived from `recent_tasks.len()` — that slice is capped by
    ///   `recent_tasks_limit` and would undercount when
    ///   `compaction_tasks_threshold > recent_tasks_limit`.
    /// - `profile_budget_tokens`: the per-profile context budget, used for
    ///   compaction trigger (b) (§6.2: "> 70% of context budget").
    /// - `repo_path`: used to normalize key_files paths to repo-relative (§6.3).
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &self,
        mut current: SubagentMemoryRow,
        update: MemoryUpdate,
        recent_tasks: &[SubagentTaskRow],
        tasks_since_last_compaction: u32,
        task_id: &str,
        profile_budget_tokens: u32,
        repo_path: &str,
    ) -> SubagentMemoryRow {
        // §6.2.2: hot_summary = current_state_summary (NOT task_summary).
        // When current_state_summary is None (no memory_update), PRESERVE the
        // existing hot_summary — a missing update must not destroy memory.
        if let Some(summary) = update.current_state_summary {
            current.hot_summary = Some(summary);
        }

        // Merge key_files (normalize + dedupe + max score + latest last_seen).
        let mut files = parse_json_vec::<KeyFile>(&current.key_files_json);
        for f in update.key_files {
            let normalized = normalize_path(&f.path, repo_path);
            if let Some(existing) = files
                .iter_mut()
                .find(|e| same_path_normalized(&e.path, &normalized))
            {
                existing.score = existing.score.max(f.score);
                existing.last_seen_at_ms = existing.last_seen_at_ms.max(f.last_seen_at_ms);
                existing.reason = f.reason;
            } else {
                files.push(KeyFile {
                    path: normalized,
                    reason: f.reason,
                    last_seen_at_ms: f.last_seen_at_ms,
                    score: f.score,
                });
            }
        }
        current.key_files_json = Some(serde_json::to_string(&files).unwrap_or_default());

        // Merge open_questions (trim + dedupe by lowercase exact match, preserve casing).
        let mut questions = parse_json_vec::<OpenQuestion>(&current.open_questions_json);
        for q in update.open_questions {
            let trimmed = q.question.trim().to_string();
            let lower = trimmed.to_lowercase();
            if let Some(existing) = questions
                .iter_mut()
                .find(|e| e.question.trim().to_lowercase() == lower)
            {
                existing.last_seen_at_ms = existing.last_seen_at_ms.max(q.last_seen_at_ms);
                if q.status == "resolved" {
                    existing.status = "resolved".into();
                }
            } else {
                questions.push(OpenQuestion {
                    question: trimmed,
                    status: q.status,
                    created_at_ms: q.created_at_ms,
                    last_seen_at_ms: q.last_seen_at_ms,
                });
            }
        }
        current.open_questions_json = Some(serde_json::to_string(&questions).unwrap_or_default());

        // Merge decisions (dedupe by exact match).
        let mut decisions = parse_json_vec::<String>(&current.decisions_json);
        for d in update.decisions {
            if !decisions.iter().any(|e: &String| e == &d) {
                decisions.push(d);
            }
        }
        current.decisions_json = Some(serde_json::to_string(&decisions).unwrap_or_default());

        // Attempts: append a one-line summary from the MOST RECENT task.
        // recent_tasks is DESC-ordered, so .first() is the most recent.
        let mut attempts = parse_json_vec::<String>(&current.attempts_json);
        if let Some(summary) = recent_tasks
            .first()
            .and_then(|t| t.result_summary.as_deref())
        {
            let entry = format!("{}: {}", task_id, summary.lines().next().unwrap_or(summary));
            if !attempts.iter().any(|a: &String| a == &entry) {
                attempts.push(entry);
            }
        }
        // Keep last 10.
        let start = attempts.len().saturating_sub(MAX_ATTEMPTS);
        attempts = attempts[start..].to_vec();
        current.attempts_json = Some(serde_json::to_string(&attempts).unwrap_or_default());

        // Compaction triggers (§6.2.3).
        // (a) ≥ threshold tasks since last compaction. Uses the authoritative
        //     `tasks_since_last_compaction` count from the store query — NOT
        //     `recent_tasks.len()`, which is capped by `recent_tasks_limit`
        //     and would undercount when threshold > limit.
        // (b) hot_summary + long_summary + recent summaries > ratio of profile
        //     budget (§6.2: "hot_summary + long_summary + recent summaries").
        let should_compact = tasks_since_last_compaction >= self.config.compaction_tasks_threshold
            || self.memory_oversized(&current, recent_tasks, profile_budget_tokens);
        if should_compact {
            self.compact(&mut current, recent_tasks);
            // Per-category caps apply DURING compaction (§6.2.4).
            let mut decisions = parse_json_vec::<String>(&current.decisions_json);
            let start = decisions.len().saturating_sub(MAX_DECISIONS);
            decisions = decisions[start..].to_vec();
            current.decisions_json = Some(serde_json::to_string(&decisions).unwrap_or_default());

            let mut files = parse_json_vec::<KeyFile>(&current.key_files_json);
            files.sort_by(|a, b| {
                b.score
                    .cmp(&a.score)
                    .then(b.last_seen_at_ms.cmp(&a.last_seen_at_ms))
            });
            files.truncate(MAX_KEY_FILES);
            current.key_files_json = Some(serde_json::to_string(&files).unwrap_or_default());

            let mut questions = parse_json_vec::<OpenQuestion>(&current.open_questions_json);
            questions.truncate(MAX_OPEN_QUESTIONS);
            current.open_questions_json =
                Some(serde_json::to_string(&questions).unwrap_or_default());

            current.last_compacted_at_ms = Some(busytok_domain::now_ms());
            current.last_compacted_task_id = Some(task_id.to_string());
        }

        current.updated_at_ms = busytok_domain::now_ms();
        current
    }

    /// Compaction trigger (b): hot_summary + long_summary + recent task
    /// summaries exceed `compaction_budget_ratio` of the profile context
    /// budget (§6.2: "hot_summary + long_summary + recent summaries > 70%
    /// of context budget"). We approximate tokens as chars / 4 (a standard
    /// heuristic); this is a conservative over-count. Uses the PROFILE budget
    /// (§6.1: per-profile configurable), NOT the global default.
    fn memory_oversized(
        &self,
        mem: &SubagentMemoryRow,
        recent_tasks: &[SubagentTaskRow],
        profile_budget_tokens: u32,
    ) -> bool {
        let budget_chars = (profile_budget_tokens as usize) * 4;
        let threshold = (budget_chars as f64 * self.config.compaction_budget_ratio) as usize;
        let mut total = 0usize;
        if let Some(s) = &mem.hot_summary {
            total += s.len();
        }
        if let Some(s) = &mem.long_summary {
            total += s.len();
        }
        // §6.2: include recent task summaries in the size estimate so that
        // "memory itself is small but recent summaries are large" still
        // triggers compaction.
        for t in recent_tasks {
            if let Some(s) = &t.result_summary {
                total += s.len();
            }
        }
        total > threshold
    }

    /// Rule-based compaction (§6.2.4). Rebuilds long_summary from the old
    /// long_summary + the 5 MOST RECENT task summaries, then drops resolved
    /// open questions. Per-category caps are applied by the caller after this.
    fn compact(&self, mem: &mut SubagentMemoryRow, recent_tasks: &[SubagentTaskRow]) {
        let old_long = mem
            .long_summary
            .as_deref()
            .unwrap_or("")
            .chars()
            .take(OLD_LONG_SUMMARY_KEEP_CHARS)
            .collect::<String>();
        // recent_tasks is DESC (most recent first). Take the first 5 = most recent.
        let recent_summaries: String = recent_tasks
            .iter()
            .take(RECENT_TASKS_FOR_COMPACTION)
            .filter_map(|t| t.result_summary.as_deref())
            .collect::<Vec<_>>()
            .join("\n")
            .chars()
            .take(RECENT_SUMMARIES_KEEP_CHARS)
            .collect();
        let new_long = if recent_summaries.is_empty() {
            old_long
        } else {
            format!("{old_long}\n\nRecent findings:\n{recent_summaries}")
        };
        let new_long: String = new_long.chars().take(MAX_LONG_SUMMARY_CHARS).collect();
        mem.long_summary = if new_long.is_empty() {
            None
        } else {
            Some(new_long)
        };

        // Drop resolved open questions during compaction (§6.2.4 cap: unresolved only).
        let mut questions = parse_json_vec::<OpenQuestion>(&mem.open_questions_json);
        questions.retain(|q| q.status != "resolved");
        mem.open_questions_json = Some(serde_json::to_string(&questions).unwrap_or_default());
    }
}

/// Generic JSON vec parser — replaces 4 near-identical helpers.
fn parse_json_vec<T: DeserializeOwned>(json: &Option<String>) -> Vec<T> {
    json.as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}

/// Normalize a file path (spec §6.3): repo-relative, forward slashes, strip
/// `./` prefix. macOS case-insensitive dedup is handled by `same_path_normalized`.
fn normalize_path(path: &str, repo_path: &str) -> String {
    let mut p = path.replace('\\', "/");
    // Strip repo_path prefix (absolute or trailing-slash form) to make repo-relative.
    // Check the trailing-slash form FIRST so `/repo/src/...` strips to
    // `src/...` (not `/src/...`); otherwise the shorter `/repo` prefix would
    // match first and leave a leading slash.
    if !repo_path.is_empty() {
        let repo_fwd = repo_path.replace('\\', "/");
        let with_slash = format!("{repo_fwd}/");
        for prefix in &[with_slash.as_str(), repo_fwd.as_str()] {
            if p.starts_with(prefix) {
                p = p[prefix.len()..].to_string();
                break;
            }
        }
    }
    let p = p.strip_prefix("./").unwrap_or(&p).to_string();
    p
}

/// Case-insensitive path comparison for dedup (spec §6.3 macOS rule).
fn same_path_normalized(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}
