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
pub struct CompactContext {
    pub compact_context: String,
    pub budget_tokens: u32,
    pub source: String,
}

/// Raw structured memory for the RPC `memory` field (spec §4.3). The sidecar
/// uses `compact_context` as authoritative; this carries full structured data
/// for debugging (key_files retain score/reason, open_questions retain status).
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

        let mut sections: Vec<Section> = Vec::new();
        // Identity + intent (never trimmed — tiny).
        let mut header = format!("You are Busytok logical subagent: {}\n", subagent.name);
        if let Some(intent) = &subagent.intent {
            header.push_str(&format!("\nLong-term goal: {intent}\n"));
        }
        sections.push(Section::header(header));

        // Recent task summaries (trim priority 1).
        if !recent_summaries.is_empty() {
            sections.push(Section::recent_summaries(format!(
                "Recent tasks:\n{}\n",
                recent_summaries
                    .iter()
                    .map(|s| format!("- {s}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )));
        }

        // Key files (trim priority 3).
        if !snapshot.key_files.is_empty() {
            sections.push(Section::key_files(format!(
                "Key files: {}\n",
                snapshot
                    .key_files
                    .iter()
                    .map(|f| f.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        // Open questions (trim priority 4).
        if !snapshot.open_questions.is_empty() {
            sections.push(Section::open_questions(format!(
                "Open questions:\n{}\n",
                snapshot
                    .open_questions
                    .iter()
                    .map(|q| format!("- {}", q.question))
                    .collect::<Vec<_>>()
                    .join("\n")
            )));
        }

        // Long summary (trim priority 5).
        if let Some(long) = &snapshot.long_summary {
            sections.push(Section::long_summary(format!(
                "Long-term findings:\n{long}\n"
            )));
        }

        // Hot summary (trim priority 6 — preserve as much as possible).
        if let Some(hot) = &snapshot.hot_summary {
            sections.push(Section::hot_summary(format!("Current state:\n{hot}\n")));
        }

        // Current task prompt (trim priority 7 — NEVER trimmed).
        sections.push(Section::prompt(format!("\nCurrent task:\n{prompt}\n")));

        let budget_chars = (budget as usize) * 4; // token ≈ 4 chars heuristic
        let compact = assemble_with_budget(sections, budget_chars);

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

/// A labeled section with a trim priority (lower number = trimmed first).
/// `protected` sections (header, prompt) are never dropped.
struct Section {
    priority: u8,
    text: String,
    protected: bool,
}

impl Section {
    fn header(text: String) -> Self {
        Self {
            priority: 7,
            text,
            protected: true,
        }
    }
    fn recent_summaries(text: String) -> Self {
        Self {
            priority: 1,
            text,
            protected: false,
        }
    }
    fn key_files(text: String) -> Self {
        Self {
            priority: 3,
            text,
            protected: false,
        }
    }
    fn open_questions(text: String) -> Self {
        Self {
            priority: 4,
            text,
            protected: false,
        }
    }
    fn long_summary(text: String) -> Self {
        Self {
            priority: 5,
            text,
            protected: false,
        }
    }
    fn hot_summary(text: String) -> Self {
        Self {
            priority: 6,
            text,
            protected: false,
        }
    }
    fn prompt(text: String) -> Self {
        Self {
            priority: 7,
            text,
            protected: true,
        }
    }
}

/// Assemble sections into one string. If over budget, progressively drop
/// lowest-priority non-protected sections (never header/prompt) until it fits.
/// Rebuilds from kept sections directly (no string-replace, which could
/// corrupt the context when a section's text appears inside another).
fn assemble_with_budget(sections: Vec<Section>, budget_chars: usize) -> String {
    let full: String = sections.iter().map(|s| s.text.as_str()).collect();
    if full.len() <= budget_chars || budget_chars == 0 {
        return full;
    }

    // Over budget: sort drop candidates by priority ascending (lowest = dropped first).
    let mut drop_order: Vec<usize> = sections
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.protected)
        .collect::<Vec<_>>()
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    drop_order.sort_by_key(|&i| sections[i].priority);

    let mut dropped: Vec<usize> = Vec::new();
    for &idx in &drop_order {
        let current_len: usize = sections
            .iter()
            .enumerate()
            .filter(|(i, _)| !dropped.contains(i))
            .map(|(_, s)| s.text.len())
            .sum();
        if current_len <= budget_chars {
            break;
        }
        dropped.push(idx);
    }

    // Rebuild from kept sections, preserving original order.
    let mut result = String::new();
    for (i, s) in sections.iter().enumerate() {
        if !dropped.contains(&i) {
            result.push_str(&s.text);
        }
    }
    result.trim_end_matches('\n').to_string() + "\n"
}

fn parse_json_vec<T: serde::de::DeserializeOwned>(json: &Option<String>) -> Vec<T> {
    json.as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}
