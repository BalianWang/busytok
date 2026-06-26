//! Context builder with budget control and trim priority (spec §6.1).
//!
//! Produces the `compact_context` string consumed by the sidecar as the
//! authoritative context source, plus a `MemorySnapshot` carrying the raw
//! structured memory fields (for the RPC `memory` field — debugging and
//! future direct-sidecar consumption). Pure functions, no I/O.

use busytok_config::SubagentContextConfig;
use busytok_store::{SubagentLogicalSubagentRow, SubagentMemoryRow, SubagentTaskRow};

use crate::memory::{KeyFile, OpenQuestion};

/// The prompt-ready context string + budget metadata (spec §6.1 output).
#[derive(Clone)]
pub struct CompactContext {
    pub compact_context: String,
    pub budget_tokens: u32,
    pub source: String,
}

/// Raw structured memory for the RPC `memory` field (spec §4.3). The sidecar
/// uses `compact_context` as authoritative; this carries full structured data
/// for debugging (key_files retain score/reason, open_questions retain status).
#[derive(Clone)]
pub struct MemorySnapshot {
    pub hot_summary: Option<String>,
    pub long_summary: Option<String>,
    pub key_files: Vec<KeyFile>,
    pub decisions: Vec<String>,
    pub open_questions: Vec<OpenQuestion>,
}

pub struct ContextBuilder {
    config: SubagentContextConfig,
}

impl ContextBuilder {
    pub fn new(config: SubagentContextConfig) -> Self {
        Self { config }
    }

    /// Build the compact context string from the subagent's memory + recent
    /// tasks. Applies the §6.1 trim priority when over budget.
    pub fn build(
        &self,
        subagent: &SubagentLogicalSubagentRow,
        memory: &SubagentMemoryRow,
        recent_tasks: &[SubagentTaskRow],
        prompt: &str,
        profile_budget_tokens: u32,
    ) -> (CompactContext, MemorySnapshot) {
        // Zero budget → fall back to default (avoid clamp(1, max) treating 0 as 1).
        let effective_budget = if profile_budget_tokens == 0 {
            self.config.default_budget_tokens
        } else {
            profile_budget_tokens
        };
        let budget = effective_budget.clamp(1, self.config.max_budget_tokens);

        let snapshot = MemorySnapshot {
            hot_summary: memory.hot_summary.clone(),
            long_summary: memory.long_summary.clone(),
            key_files: parse_json_vec(&memory.key_files_json),
            decisions: parse_json_vec(&memory.decisions_json),
            open_questions: parse_json_vec(&memory.open_questions_json),
        };

        // Recent task summaries: most recent first. Sort by created_at_ms DESC
        // so the output is most-recent-first regardless of input order (the
        // store query returns DESC, but we sort defensively to guarantee the
        // contract).
        let mut sorted: Vec<&SubagentTaskRow> = recent_tasks.iter().collect();
        sorted.sort_unstable_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
        let recent_summaries: Vec<String> = sorted
            .iter()
            .take(self.config.recent_tasks_limit as usize)
            .filter_map(|t| t.result_summary.as_deref())
            .map(String::from)
            .collect();

        // Identity + intent (never trimmed — tiny).
        let mut header = format!("You are Busytok logical subagent: {}\n", subagent.name);
        if let Some(intent) = &subagent.intent {
            header.push_str(&format!("\nLong-term goal: {intent}\n"));
        }

        let parts = ContextParts {
            header,
            recent_summaries,
            attempts: parse_json_vec::<String>(&memory.attempts_json),
            key_files: snapshot.key_files.clone(),
            open_questions: snapshot.open_questions.clone(),
            decisions: snapshot.decisions.clone(),
            long_summary: snapshot.long_summary.clone(),
            hot_summary: snapshot.hot_summary.clone(),
            prompt: format!("\nCurrent task:\n{prompt}\n"),
        };

        let budget_chars = (budget as usize) * 4; // token ≈ 4 chars heuristic
        let compact = assemble_with_budget(parts, budget_chars);

        (
            CompactContext {
                compact_context: compact,
                budget_tokens: budget,
                source: "busytok-context-builder/v1".to_string(),
            },
            snapshot,
        )
    }
}

/// Raw components for context assembly. `assemble_with_budget` re-slices
/// these to apply progressive reduction per §6.1 trim priority.
struct ContextParts {
    header: String,                    // protected (priority 7), never trimmed
    recent_summaries: Vec<String>,     // priority 1: 5 → 3 → 1 → drop
    attempts: Vec<String>,             // priority 2: all → last 3 → drop (newest last)
    key_files: Vec<KeyFile>,           // priority 3: 20 → 10 → drop
    open_questions: Vec<OpenQuestion>, // priority 4: 10 → 5 → drop
    decisions: Vec<String>,            // protected (not in trim priority list)
    long_summary: Option<String>,      // priority 5: truncate → drop
    hot_summary: Option<String>,       // priority 6: preserve (never trimmed)
    prompt: String,                    // protected (priority 7), never trimmed
}

/// Per-section reduction state during progressive trimming.
/// `Some(n)` on item-based sections = take `n` items; `None` = drop.
/// For `attempts`, the LAST `n` items are kept (newest-last storage order
/// from `MemoryUpdater`); other item-sections take the first `n`.
/// `Some(n)` on `long_summary_chars` = truncate text to `n` chars; `None` = drop.
#[derive(Clone, Copy)]
struct TrimState {
    recent: Option<usize>,
    attempts: Option<usize>,
    key_files: Option<usize>,
    open_questions: Option<usize>,
    long_summary_chars: Option<usize>,
}

impl ContextParts {
    /// Render the full context at the given reduction levels. Empty sections
    /// (no items or dropped) emit nothing; this mirrors the original gate
    /// `if !foo.is_empty()` so reduced-to-zero item sections stay absent.
    fn render(&self, state: TrimState) -> String {
        let mut out = String::new();
        out.push_str(&self.header);

        if let Some(n) = state.recent {
            let slice = take_slice(&self.recent_summaries, n);
            if !slice.is_empty() {
                out.push_str(&format!(
                    "Recent tasks:\n{}\n",
                    slice
                        .iter()
                        .map(|s| format!("- {s}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
        }

        // Attempts (priority 2): stored newest-last by MemoryUpdater, so keep
        // the LAST n items (most recent) rather than the first n.
        if let Some(n) = state.attempts {
            let slice = take_last_slice(&self.attempts, n);
            if !slice.is_empty() {
                out.push_str(&format!(
                    "Attempts:\n{}\n",
                    slice
                        .iter()
                        .map(|a| format!("- {a}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
        }

        if let Some(n) = state.key_files {
            let slice = take_slice(&self.key_files, n);
            if !slice.is_empty() {
                out.push_str(&format!(
                    "Key files: {}\n",
                    slice
                        .iter()
                        .map(|f| f.path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }

        if let Some(n) = state.open_questions {
            let slice = take_slice(&self.open_questions, n);
            if !slice.is_empty() {
                out.push_str(&format!(
                    "Open questions:\n{}\n",
                    slice
                        .iter()
                        .map(|q| format!("- {}", q.question))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
        }

        // Decisions: protected (not in spec §6.1 trim priority list). Always
        // rendered in full — high-value, typically few entries.
        if !self.decisions.is_empty() {
            out.push_str(&format!(
                "Decisions:\n{}\n",
                self.decisions
                    .iter()
                    .map(|d| format!("- {d}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }

        if let Some(chars) = state.long_summary_chars {
            if let Some(long) = &self.long_summary {
                let truncated: String = long.chars().take(chars).collect();
                out.push_str(&format!("Long-term findings:\n{truncated}\n"));
            }
        }

        if let Some(hot) = &self.hot_summary {
            out.push_str(&format!("Current state:\n{hot}\n"));
        }

        out.push_str(&self.prompt);
        out
    }
}

fn take_slice<T>(v: &[T], n: usize) -> &[T] {
    &v[..n.min(v.len())]
}

/// Take the LAST `n` items (used for `attempts`, which `MemoryUpdater` stores
/// newest-last via `push`). Returns all items if `n >= len`.
fn take_last_slice<T>(v: &[T], n: usize) -> &[T] {
    let start = v.len().saturating_sub(n);
    &v[start..]
}

/// Render at the given state and return the finalized string if it fits the
/// budget; otherwise `None`.
fn try_fit(parts: &ContextParts, budget_chars: usize, state: TrimState) -> Option<String> {
    let rendered = parts.render(state);
    if rendered.len() <= budget_chars {
        Some(finalize(rendered))
    } else {
        None
    }
}

/// Assemble parts into one string. If over budget, apply progressive
/// reduction per §6.1 trim priority: priority 1 (recent_summaries), then
/// 2 (attempts), 3 (key_files), 4 (open_questions), 5 (long_summary).
/// Priorities 6 (hot_summary) and 7 (header/prompt) are never trimmed.
/// `decisions` is also protected (not listed in §6.1 trim priority).
///
/// For each trim priority, reduction levels are attempted in order; after each
/// level the total is rechecked and the algorithm stops as soon as it fits.
/// Only after all reduction levels for a priority are exhausted is that
/// section dropped before moving to the next priority.
fn assemble_with_budget(parts: ContextParts, budget_chars: usize) -> String {
    let recent_full = parts.recent_summaries.len();
    let attempts_full = parts.attempts.len();
    let key_files_full = parts.key_files.len();
    let open_questions_full = parts.open_questions.len();
    let long_summary_full_chars = parts
        .long_summary
        .as_deref()
        .map(|s| s.chars().count())
        .unwrap_or(0);

    let mut state = TrimState {
        recent: Some(recent_full),
        attempts: Some(attempts_full),
        key_files: Some(key_files_full),
        open_questions: Some(open_questions_full),
        long_summary_chars: Some(long_summary_full_chars),
    };

    // Initial: all sections at full size. `budget_chars` is always ≥ 4
    // (caller clamps `budget` to ≥ 1 token, then ×4 chars/token), so the
    // zero-budget case is handled upstream by the default-budget fallback.
    let full = parts.render(state);
    if full.len() <= budget_chars {
        return finalize(full);
    }

    // Trim priority 1: recent_summaries 5 → 3 → 1 → drop.
    for &level in &[3usize, 1] {
        if recent_full > level {
            state.recent = Some(level);
            if let Some(out) = try_fit(&parts, budget_chars, state) {
                return out;
            }
        }
    }
    state.recent = None;
    if let Some(out) = try_fit(&parts, budget_chars, state) {
        return out;
    }

    // Trim priority 2: attempts all → last 3 → drop.
    // Attempts are stored newest-last, so `Some(3)` keeps the 3 most recent.
    if attempts_full > 3 {
        state.attempts = Some(3);
        if let Some(out) = try_fit(&parts, budget_chars, state) {
            return out;
        }
    }
    state.attempts = None;
    if let Some(out) = try_fit(&parts, budget_chars, state) {
        return out;
    }

    // Trim priority 3: key_files 20 → 10 → drop.
    if key_files_full > 10 {
        state.key_files = Some(10);
        if let Some(out) = try_fit(&parts, budget_chars, state) {
            return out;
        }
    }
    state.key_files = None;
    if let Some(out) = try_fit(&parts, budget_chars, state) {
        return out;
    }

    // Trim priority 4: open_questions 10 → 5 → drop.
    if open_questions_full > 5 {
        state.open_questions = Some(5);
        if let Some(out) = try_fit(&parts, budget_chars, state) {
            return out;
        }
    }
    state.open_questions = None;
    if let Some(out) = try_fit(&parts, budget_chars, state) {
        return out;
    }

    // Trim priority 5: long_summary truncate to fit → drop. Compute the
    // remaining budget after all other kept sections (recent/attempts/
    // key_files/open_questions are all dropped by this point — decisions,
    // hot_summary, header, and prompt are protected), then truncate the
    // long_summary text to fit. If the remaining budget can't even fit the
    // section overhead, drop long_summary entirely.
    if parts.long_summary.is_some() {
        state.long_summary_chars = None;
        let without_long = parts.render(state);
        if without_long.len() < budget_chars {
            let remaining = budget_chars - without_long.len();
            // Section overhead: "Long-term findings:\n" (20) + trailing "\n" (1) = 21.
            let long_overhead = "Long-term findings:\n".len() + "\n".len();
            if remaining > long_overhead {
                state.long_summary_chars = Some(remaining - long_overhead);
                let rendered = parts.render(state);
                if rendered.len() <= budget_chars {
                    return finalize(rendered);
                }
            }
        }
        // Drop long_summary entirely (best effort — hot_summary and prompt are
        // protected, so we can't trim further even if still over budget).
        return finalize(without_long);
    }

    // long_summary was None — nothing more to trim. Return current state
    // (hot_summary + prompt + header, all protected).
    finalize(parts.render(state))
}

fn finalize(s: String) -> String {
    s.trim_end_matches('\n').to_string() + "\n"
}

fn parse_json_vec<T: serde::de::DeserializeOwned>(json: &Option<String>) -> Vec<T> {
    json.as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}
