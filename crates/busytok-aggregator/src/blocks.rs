//! Billing block identification and burn rate calculation.
//!
//! Groups usage events into time-based billing blocks (default 5 hours,
//! matching Claude's billing windows) with gap detection between periods
//! of inactivity. Also provides burn rate (tokens/min, cost/hr) analysis.
//!
//! This mirrors ccusage's `_session-blocks.ts` algorithm.

use busytok_domain::{now_ms, BillingBlock, BurnRate, BurnStatus, NormalizedUsageEvent};
use tracing::debug;

/// Default session duration in hours (matches Claude's billing window).
pub const DEFAULT_SESSION_DURATION_HOURS: i64 = 5;

/// Identify billing blocks from a list of usage events.
///
/// Groups events into time-based blocks with floor-to-hour alignment.
/// Inserts gap blocks between blocks where no activity occurred.
/// Returns an empty vec if `events` is empty.
pub fn identify_session_blocks(
    events: &[NormalizedUsageEvent],
    session_duration_hours: i64,
) -> Vec<BillingBlock> {
    if events.is_empty() {
        return vec![];
    }

    let session_duration_ms = session_duration_hours * 60 * 60 * 1000;
    let mut sorted: Vec<&NormalizedUsageEvent> = events.iter().collect();
    sorted.sort_by_key(|e| e.timestamp_ms);

    let mut blocks: Vec<BillingBlock> = Vec::new();
    let mut block_start: Option<i64> = None;
    let mut block_entries: Vec<&NormalizedUsageEvent> = Vec::new();
    let now_ms = now_ms();

    for entry in &sorted {
        let entry_time = entry.timestamp_ms;

        if block_start.is_none() {
            // Floor to nearest hour
            block_start = Some(floor_to_hour_ms(entry_time));
            block_entries = vec![*entry];
        } else {
            let start = block_start.unwrap();
            let time_since_start = entry_time - start;
            let last_entry = block_entries.last().unwrap();
            let time_since_last = entry_time - last_entry.timestamp_ms;

            if time_since_start > session_duration_ms || time_since_last > session_duration_ms {
                // Close current block
                let block = create_block(start, &block_entries, now_ms, session_duration_ms);
                blocks.push(block);

                // Add gap block if there's a significant gap
                if time_since_last > session_duration_ms {
                    if let Some(gap) =
                        create_gap_block(last_entry.timestamp_ms, entry_time, session_duration_ms)
                    {
                        blocks.push(gap);
                    }
                }

                // Start new block (floored to hour)
                block_start = Some(floor_to_hour_ms(entry_time));
                block_entries = vec![*entry];
            } else {
                block_entries.push(entry);
            }
        }
    }

    // Close the last block
    if let Some(start) = block_start {
        if !block_entries.is_empty() {
            let block = create_block(start, &block_entries, now_ms, session_duration_ms);
            blocks.push(block);
        }
    }

    debug!(
        "identified {} billing blocks from {} events",
        blocks.len(),
        events.len()
    );

    blocks
}

/// Floor a timestamp in milliseconds to the nearest hour boundary.
fn floor_to_hour_ms(timestamp_ms: i64) -> i64 {
    let remainder = timestamp_ms % (3600 * 1000);
    timestamp_ms - remainder
}

/// Create a `BillingBlock` from a set of event entries.
fn create_block(
    start_ms: i64,
    entries: &[&NormalizedUsageEvent],
    now_ms: i64,
    duration_ms: i64,
) -> BillingBlock {
    let end_ms = start_ms + duration_ms;
    let last = entries.last().unwrap();
    let actual_end = last.timestamp_ms;
    let is_active = (now_ms - actual_end) < duration_ms && now_ms < end_ms;

    let mut input_tokens = 0i64;
    let mut output_tokens = 0i64;
    let mut total_tokens = 0i64;
    let mut cached_input_tokens = 0i64;
    let mut cache_creation_tokens = 0i64;
    let mut cache_read_tokens = 0i64;
    let mut cost_usd: Option<f64> = None;
    let mut estimated_cost_usd: Option<f64> = None;
    let mut models: Vec<String> = Vec::new();
    let mut usage_limit_reset: Option<i64> = None;

    for e in entries {
        input_tokens += e.input_tokens;
        output_tokens += e.output_tokens;
        total_tokens += e.total_tokens;
        cached_input_tokens += e.cached_input_tokens;
        cache_creation_tokens += e.cache_creation_tokens;
        cache_read_tokens += e.cache_read_tokens;
        cost_usd = merge_opt_add(cost_usd, e.cost_usd);
        estimated_cost_usd = merge_opt_add(estimated_cost_usd, e.estimated_cost_usd);
        if let Some(ref model) = e.model {
            if !model.is_empty() && !models.contains(model) {
                models.push(model.clone());
            }
        }
        // Take the last non-None usage_limit_reset_time_ms from entries
        if e.usage_limit_reset_time_ms.is_some() {
            usage_limit_reset = e.usage_limit_reset_time_ms;
        }
    }

    BillingBlock {
        id: format!("block_{}", start_ms),
        start_time_ms: start_ms,
        end_time_ms: end_ms,
        actual_end_time_ms: Some(actual_end),
        is_active,
        is_gap: false,
        input_tokens,
        output_tokens,
        total_tokens,
        cached_input_tokens,
        cache_creation_tokens,
        cache_read_tokens,
        cost_usd,
        estimated_cost_usd,
        models,
        event_count: entries.len() as i64,
        usage_limit_reset_time_ms: usage_limit_reset,
        agent: entries.first().map(|e| e.agent.as_str().to_string()),
    }
}

/// Create a gap block representing a period of inactivity between two real blocks.
/// Returns `None` if the gap is not significant enough.
fn create_gap_block(
    last_time_ms: i64,
    next_time_ms: i64,
    duration_ms: i64,
) -> Option<BillingBlock> {
    let gap = next_time_ms - last_time_ms;
    if gap <= duration_ms {
        return None;
    }
    let gap_start = last_time_ms + duration_ms;
    let gap_end = next_time_ms;
    Some(BillingBlock {
        id: format!("gap_{}", gap_start),
        start_time_ms: gap_start,
        end_time_ms: gap_end,
        actual_end_time_ms: None,
        is_active: false,
        is_gap: true,
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        cost_usd: None,
        estimated_cost_usd: None,
        models: vec![],
        event_count: 0,
        usage_limit_reset_time_ms: None,
        agent: None,
    })
}

/// Merge two optional f64 values by addition.
fn merge_opt_add(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

/// Calculate burn rate (tokens/min and cost/hr) for a billing block.
///
/// Returns `None` for gap blocks, blocks with no events, or blocks
/// that have no recorded activity duration.
pub fn calculate_burn_rate(block: &BillingBlock) -> Option<BurnRate> {
    if block.is_gap || block.entries_count() == 0 || block.actual_end_time_ms.is_none() {
        return None;
    }
    let duration_ms = block.actual_end_time_ms.unwrap() - block.start_time_ms;
    if duration_ms <= 0 {
        return None;
    }
    let duration_min = duration_ms as f64 / 60000.0;
    let tokens_per_minute = block.total_tokens as f64 / duration_min;
    let cost_per_hour = block.cost_usd.map(|cost| cost / duration_min * 60.0);

    // Use non-cache tokens for burn rate indicator (like ccusage).
    let non_cache_tokens = block.input_tokens + block.output_tokens;
    let indicator_tpm = non_cache_tokens as f64 / duration_min;
    let status = if indicator_tpm < 2000.0 {
        BurnStatus::Normal
    } else if indicator_tpm < 5000.0 {
        BurnStatus::Moderate
    } else {
        BurnStatus::High
    };

    debug!(
        "calculated burn rate for block {}: status={:?}",
        block.id, status
    );

    Some(BurnRate {
        tokens_per_minute,
        cost_per_hour,
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use busytok_domain::AgentKind;

    fn make_event(timestamp_ms: i64, input: i64, output: i64) -> NormalizedUsageEvent {
        let mut e = NormalizedUsageEvent::minimal_for_test("test", AgentKind::ClaudeCode);
        e.timestamp_ms = timestamp_ms;
        e.input_tokens = input;
        e.output_tokens = output;
        e.total_tokens = input + output;
        e
    }

    #[test]
    fn empty_events_returns_empty_blocks() {
        let blocks = identify_session_blocks(&[], 5);
        assert!(blocks.is_empty());
    }

    #[test]
    fn single_event_creates_one_block() {
        let events = vec![make_event(1000, 10, 5)];
        let blocks = identify_session_blocks(&events, 5);
        assert_eq!(blocks.len(), 1);
        assert!(!blocks[0].is_gap);
        assert_eq!(blocks[0].input_tokens, 10);
        assert_eq!(blocks[0].output_tokens, 5);
        assert_eq!(blocks[0].event_count, 1);
    }

    #[test]
    fn two_close_events_same_block() {
        let events = vec![make_event(1000, 10, 5), make_event(2000, 20, 10)];
        let blocks = identify_session_blocks(&events, 5);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].event_count, 2);
        assert_eq!(blocks[0].total_tokens, 45);
    }

    #[test]
    fn gap_creates_separate_blocks() {
        let far_future = 6 * 60 * 60 * 1000; // 6 hours later
        let events = vec![
            make_event(1000, 10, 5),
            make_event(1000 + far_future, 20, 10),
        ];
        let blocks = identify_session_blocks(&events, 5);
        assert_eq!(blocks.len(), 3); // block1 + gap + block2
        assert_eq!(blocks[0].event_count, 1);
        assert!(blocks[1].is_gap);
        assert_eq!(blocks[2].event_count, 1);
    }

    #[test]
    fn burn_rate_normal() {
        // 1000 tokens over 10 minutes = 100 tpm (well under 2000)
        let mut e = make_event(1000, 500, 500);
        e.total_tokens = 1000;
        let block = BillingBlock {
            id: "test".to_string(),
            start_time_ms: 0,
            end_time_ms: 5 * 3600 * 1000,
            actual_end_time_ms: Some(600_000), // 10 minutes
            is_active: false,
            is_gap: false,
            input_tokens: 500,
            output_tokens: 500,
            total_tokens: 1000,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            cost_usd: Some(0.01),
            estimated_cost_usd: None,
            models: vec![],
            event_count: 1,
            usage_limit_reset_time_ms: None,
            agent: Some("claude_code".to_string()),
        };
        let rate = calculate_burn_rate(&block).unwrap();
        assert_eq!(rate.status, BurnStatus::Normal);
        assert!((rate.tokens_per_minute - 100.0).abs() < 0.01);
    }

    #[test]
    fn burn_rate_returns_none_for_gap() {
        let gap = BillingBlock {
            id: "gap".to_string(),
            start_time_ms: 0,
            end_time_ms: 1000,
            actual_end_time_ms: None,
            is_active: false,
            is_gap: true,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            cost_usd: None,
            estimated_cost_usd: None,
            models: vec![],
            event_count: 0,
            usage_limit_reset_time_ms: None,
            agent: None,
        };
        assert!(calculate_burn_rate(&gap).is_none());
    }

    #[test]
    fn floor_to_hour_ms_works() {
        // 1:30:00 => 1:00:00
        let ts = 3600_000 + 1800_000; // 1h30m in ms
        assert_eq!(floor_to_hour_ms(ts), 3600_000);

        // exactly on the hour
        assert_eq!(floor_to_hour_ms(3600_000), 3600_000);

        // 0
        assert_eq!(floor_to_hour_ms(0), 0);
    }

    #[test]
    fn model_dedup_in_block() {
        let mut e1 = make_event(1000, 10, 5);
        e1.model = Some("claude-sonnet-4".to_string());
        let mut e2 = make_event(2000, 20, 10);
        e2.model = Some("claude-sonnet-4".to_string());
        let events = vec![e1, e2];
        let blocks = identify_session_blocks(&events, 5);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].models.len(), 1);
        assert_eq!(blocks[0].models[0], "claude-sonnet-4");
    }
}
