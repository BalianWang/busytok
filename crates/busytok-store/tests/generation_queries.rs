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
use busytok_store::generation_queries;
use busytok_store::Database;

#[test]
fn read_active_generation_returns_none_on_empty_db() {
    let db = Database::open_in_memory().expect("db");
    let result = generation_queries::read_active_generation(db.conn()).expect("read");
    assert!(result.is_none());
}

#[test]
fn generation_is_promoted_active_returns_false_for_nonexistent() {
    let db = Database::open_in_memory().expect("db");
    let result =
        generation_queries::generation_is_promoted_active(db.conn(), "nonexistent").expect("check");
    assert!(!result);
}

#[test]
fn has_blocking_degradation_diagnostic_returns_false_on_empty_db() {
    let db = Database::open_in_memory().expect("db");
    let result = generation_queries::has_blocking_degradation_diagnostic(db.conn()).expect("check");
    assert!(!result);
}

#[test]
fn service_state_is_ready_exact_returns_false_on_empty_db() {
    let db = Database::open_in_memory().expect("db");
    let result =
        generation_queries::service_state_is_ready_exact_for_generation(db.conn(), "any-gen")
            .expect("check");
    assert!(!result);
}
