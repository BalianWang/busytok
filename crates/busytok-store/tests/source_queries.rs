#![allow(
    clippy::await_holding_lock,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::field_reassign_with_default,
    clippy::uninlined_format_args,
    clippy::inconsistent_digit_grouping,
    clippy::len_zero,
    clippy::identity_op,
    clippy::useless_vec,
    clippy::manual_dangling_ptr,
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::absurd_extreme_comparisons,
    unused_variables,
    unused_imports,
    dead_code
)]
use busytok_store::source_queries;
use busytok_store::Database;

#[test]
fn list_active_user_configured_roots_returns_empty_on_empty_db() {
    let db = Database::open_in_memory().expect("db");
    let roots = source_queries::list_active_user_configured_roots(db.conn()).expect("list");
    assert!(roots.is_empty());
}

#[test]
fn list_active_user_configured_roots_filters_inactive_and_non_user() {
    let db = Database::open_in_memory().expect("db");
    let now = 1000i64;
    // Inactive — should be excluded.
    db.conn()
        .execute(
            "INSERT INTO log_sources \
         (id, agent, source_type, root_path, status, configured_by_user, \
          first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms) \
         VALUES ('s1', 'claude_code', 'jsonl', '/r1', 'paused', 1, \
                 ?1, ?1, ?1, ?1)",
            rusqlite::params![now],
        )
        .expect("seed");
    // Not user-configured — should be excluded.
    db.conn()
        .execute(
            "INSERT INTO log_sources \
         (id, agent, source_type, root_path, status, configured_by_user, \
          first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms) \
         VALUES ('s2', 'codex', 'jsonl', '/r2', 'active', 0, \
                 ?1, ?1, ?1, ?1)",
            rusqlite::params![now],
        )
        .expect("seed");
    // Active + user-configured — should be included.
    db.conn()
        .execute(
            "INSERT INTO log_sources \
         (id, agent, source_type, root_path, status, configured_by_user, \
          first_seen_at_ms, last_seen_at_ms, created_at_ms, updated_at_ms) \
         VALUES ('s3', 'claude_code', 'jsonl', '/r3', 'active', 1, \
                 ?1, ?1, ?1, ?1)",
            rusqlite::params![now],
        )
        .expect("seed");

    let roots = source_queries::list_active_user_configured_roots(db.conn()).expect("list");
    assert_eq!(
        roots.len(),
        1,
        "only the active + user-configured root should return"
    );
    assert_eq!(roots[0].source_id, "s3");
    assert_eq!(roots[0].agent, "claude_code");
    assert_eq!(roots[0].root_path.display().to_string(), "/r3");
}
