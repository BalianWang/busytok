use busytok_store::{live_queries, Database};

#[test]
fn query_exact_buckets_range_sums_by_bucket_for_generation() {
    let db = Database::open_in_memory().unwrap();
    let now = busytok_domain::now_ms();

    for (bucket_start_ms, generation_id, agent, model, tokens, events) in [
        (2_000, "gen-1", "codex", "gpt-5.4", 100, 1),
        (2_000, "gen-1", "claude", "sonnet", 50, 1),
        (4_000, "gen-other", "codex", "gpt-5.4", 999, 1),
    ] {
        db.conn()
            .execute(
                "INSERT INTO usage_buckets_2s (\
                    bucket_start_ms, agent, model, generation_id, \
                    input_tokens, output_tokens, total_tokens, \
                    cost_usd, cost_status, event_count, created_at_ms, updated_at_ms\
                ) VALUES (?1, ?2, ?3, ?4, 0, 0, ?5, NULL, 'unavailable', ?6, ?7, ?7)",
                rusqlite::params![
                    bucket_start_ms,
                    agent,
                    model,
                    generation_id,
                    tokens,
                    events,
                    now
                ],
            )
            .unwrap();
    }

    let rows = live_queries::query_exact_buckets_range(db.conn(), "gen-1", 0, 10_000).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].bucket_start_ms, 2_000);
    assert_eq!(rows[0].tokens_per_sec, 75.0);
    assert_eq!(rows[0].events_per_sec, 1.0);
}

#[test]
fn exact_live_queries_return_empty_when_materialized_table_has_no_rows() {
    let db = Database::open_in_memory().unwrap();
    let now = busytok_domain::now_ms();
    let mut event = busytok_domain::NormalizedUsageEvent::minimal_for_test(
        "raw-live-event",
        busytok_domain::AgentKind::ClaudeCode,
    );
    event.timestamp_ms = now - 2_000;
    event.total_tokens = 500;

    busytok_store::write_queries::insert_usage_events_batch(db.conn(), &[event], "gen-1").unwrap();

    let range =
        live_queries::query_exact_buckets_range(db.conn(), "gen-1", now - 10_000, now).unwrap();
    let sample = live_queries::query_exact_sample_window(db.conn(), "gen-1").unwrap();

    assert!(range.is_empty());
    assert!(sample.is_none());
}
