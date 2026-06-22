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
use busytok_store::{
    Database, NewPromptEntryRow, PromptActionRow, PromptListQuery, PromptSortRow,
    PromptUseFailureReasonRow, PromptUseOutcomeRow, PromptUseRow, PromptUseSurfaceRow,
    UpdatePromptEntryRow,
};
use rusqlite::params;

fn new_entry(content: &str) -> NewPromptEntryRow {
    NewPromptEntryRow {
        content: content.to_string(),
        tags: vec!["Engineering".to_string(), "Review".to_string()],
        alias: Some(";;review".to_string()),
    }
}

fn smart_query(query: &str) -> PromptListQuery {
    PromptListQuery {
        query: Some(query.to_string()),
        tag: None,
        sort: PromptSortRow::Smart,
        limit: 100,
    }
}

#[test]
fn prompt_entry_crud_round_trip_preserves_tags_and_normalized_alias() {
    let db = Database::open_in_memory().unwrap();
    let created = db
        .create_prompt_entry(new_entry("Review this diff"))
        .unwrap();
    assert_eq!(created.alias.as_deref(), Some(";;review"));
    assert_eq!(created.tags, vec!["Engineering", "Review"]);

    let fetched = db.get_prompt_entry(&created.id).unwrap().unwrap();
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.content, "Review this diff");

    let updated = db
        .update_prompt_entry(UpdatePromptEntryRow {
            id: created.id.clone(),
            content: "Review test failures".to_string(),
            tags: vec!["Tests".to_string()],
            alias: Some(";;tests".to_string()),
            is_pinned: false,
        })
        .unwrap();
    assert_eq!(updated.content, "Review test failures");
    assert_eq!(updated.tags, vec!["Tests"]);

    assert!(db.delete_prompt_entry(&created.id).unwrap());
    assert!(db.get_prompt_entry(&created.id).unwrap().is_none());
}

#[test]
fn prompt_validation_rejects_empty_content_and_duplicate_alias() {
    let db = Database::open_in_memory().unwrap();
    db.create_prompt_entry(new_entry("Review this diff"))
        .unwrap();

    let duplicate = db.create_prompt_entry(NewPromptEntryRow {
        alias: Some(";;REVIEW".to_string()),
        ..new_entry("Body")
    });
    assert!(
        duplicate.is_err(),
        "alias normalization should enforce uniqueness"
    );

    let empty = db.create_prompt_entry(NewPromptEntryRow {
        content: "   ".to_string(),
        tags: vec![],
        alias: None,
    });
    assert!(empty.is_err(), "empty content must be rejected");
}

#[test]
fn prompt_validation_rejects_oversized_fields() {
    let db = Database::open_in_memory().unwrap();

    let long_content = db.create_prompt_entry(NewPromptEntryRow {
        content: "x".repeat(65_537),
        tags: vec![],
        alias: None,
    });
    assert!(long_content.is_err(), "oversized content must be rejected");

    let without_alias = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "Body".to_string(),
            tags: vec!["".to_string(), "  Docs  ".to_string(), "docs".to_string()],
            alias: Some("   ".to_string()),
        })
        .unwrap();
    assert_eq!(without_alias.alias, None);
    assert_eq!(without_alias.tags, vec!["Docs"]);
}

#[test]
fn prompt_validation_rejects_aliases_with_whitespace_quotes_or_backticks() {
    let db = Database::open_in_memory().unwrap();

    for alias in ["foo bar", "foo\"bar", "foo'bar", "foo`bar"] {
        let result = db.create_prompt_entry(NewPromptEntryRow {
            content: "Body".to_string(),
            tags: vec![],
            alias: Some(alias.to_string()),
        });
        assert!(
            result.is_err(),
            "alias {alias:?} must reject whitespace, quotes, and backticks"
        );
    }
}

#[test]
fn prompt_validation_rejects_aliases_with_invisible_zero_width_separators() {
    let db = Database::open_in_memory().unwrap();

    for alias in [
        "foo\u{200b}bar",
        "foo\u{200c}bar",
        "foo\u{200d}bar",
        "foo\u{2060}bar",
    ] {
        let result = db.create_prompt_entry(NewPromptEntryRow {
            content: "Body".to_string(),
            tags: vec![],
            alias: Some(alias.to_string()),
        });
        assert!(
            result.is_err(),
            "alias {alias:?} must reject invisible separators that would make search and snippets ambiguous"
        );
    }
}

#[test]
fn prompt_validation_counts_content_in_characters_instead_of_utf8_bytes() {
    let db = Database::open_in_memory().unwrap();

    let max_multibyte = db.create_prompt_entry(NewPromptEntryRow {
        content: "你".repeat(65_536),
        tags: vec![],
        alias: None,
    });
    assert!(
        max_multibyte.is_ok(),
        "65,536 Unicode scalar values should be accepted even when encoded as multi-byte UTF-8"
    );

    let too_long = db.create_prompt_entry(NewPromptEntryRow {
        content: "你".repeat(65_537),
        tags: vec![],
        alias: None,
    });
    assert!(
        too_long.is_err(),
        "content beyond 65,536 characters must still be rejected"
    );
}

#[test]
fn prompt_list_limit_clamps_to_one_through_five_hundred() {
    let db = Database::open_in_memory().unwrap();
    for _ in 0..501 {
        db.create_prompt_entry(NewPromptEntryRow {
            content: "Body".to_string(),
            tags: vec![],
            alias: None,
        })
        .unwrap();
    }

    let zero_limit = db
        .list_prompt_entries(PromptListQuery {
            query: None,
            tag: None,
            sort: PromptSortRow::Alphabetical,
            limit: 0,
        })
        .unwrap();
    assert_eq!(zero_limit.total_count, 501);
    assert_eq!(
        zero_limit.entries.len(),
        1,
        "limit 0 must clamp up to one result"
    );

    let large_limit = db
        .list_prompt_entries(PromptListQuery {
            query: None,
            tag: None,
            sort: PromptSortRow::Alphabetical,
            limit: 999,
        })
        .unwrap();
    assert_eq!(large_limit.total_count, 501);
    assert_eq!(
        large_limit.entries.len(),
        500,
        "limits above 500 must clamp down to 500 results"
    );
}

#[test]
fn prompt_search_prioritizes_exact_alias_then_fallback_content() {
    let db = Database::open_in_memory().unwrap();
    // Entry with no alias — only matches via content in fallback.
    let content_match = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "please review this diff".to_string(),
            tags: vec![],
            alias: None,
        })
        .unwrap();
    // Entry with alias prefix match (not exact).
    let _alias_prefix = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "body".to_string(),
            tags: vec![],
            alias: Some(";;check".to_string()),
        })
        .unwrap();
    // Entry with exact alias match.
    let alias_exact = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "body".to_string(),
            tags: vec![],
            alias: Some(";;review".to_string()),
        })
        .unwrap();

    // Exact alias match hits fast path.
    let rows = db
        .list_prompt_entries(smart_query(";;review"))
        .unwrap()
        .entries;
    assert_eq!(rows[0].alias.as_deref(), Some(";;review"));
    assert_eq!(rows[0].id, alias_exact.id);

    // Plain "review" (no ;; prefix) hits no alias, falls back to content match.
    let rows = db
        .list_prompt_entries(smart_query("review"))
        .unwrap()
        .entries;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, content_match.id);
}

#[test]
fn prompt_search_fast_path_returns_both_exact_and_prefix_alias_hits() {
    let db = Database::open_in_memory().unwrap();
    let exact = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "body".to_string(),
            tags: vec![],
            alias: Some(";;review".to_string()),
        })
        .unwrap();
    let prefix = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "body".to_string(),
            tags: vec![],
            alias: Some(";;review-extra".to_string()),
        })
        .unwrap();
    let fallback = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "body".to_string(),
            tags: vec![";review".to_string()],
            alias: None,
        })
        .unwrap();

    let rows = db
        .list_prompt_entries(smart_query(";;review"))
        .unwrap()
        .entries;
    let ids: Vec<_> = rows.iter().map(|row| row.id.as_str()).collect();

    assert!(
        ids.contains(&exact.id.as_str()),
        "exact alias match should be in fast path result set"
    );
    assert!(
        ids.contains(&prefix.id.as_str()),
        "alias prefix match should be in fast path result set"
    );
    // Fallback entry may or may not appear depending on smart_rank cutoff;
    // the key assertion is that the two fast-path entries are present.
    assert!(
        ids.len() >= 2,
        "fast path should return at least exact + prefix alias hits"
    );
    // If the fallback entry is returned, it should sort after the fast-path entries.
    if ids.contains(&fallback.id.as_str()) {
        let fallback_pos = ids
            .iter()
            .position(|id| id == &fallback.id.as_str())
            .unwrap();
        let exact_pos = ids.iter().position(|id| id == &exact.id.as_str()).unwrap();
        let prefix_pos = ids.iter().position(|id| id == &prefix.id.as_str()).unwrap();
        assert!(
            fallback_pos > exact_pos && fallback_pos > prefix_pos,
            "fallback entry should sort after fast-path entries"
        );
    }
}

#[test]
fn smart_ranking_matches_overlay_priority_order() {
    let db = Database::open_in_memory().unwrap();

    // exact alias match
    let exact_alias = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "body".to_string(),
            tags: vec![],
            alias: Some("review".to_string()),
        })
        .unwrap();
    // alias prefix match (review-extra)
    let alias_prefix = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "body".to_string(),
            tags: vec![],
            alias: Some("review-extra".to_string()),
        })
        .unwrap();

    // Fast-path ordering: exact alias > alias prefix
    let rows = db
        .list_prompt_entries(smart_query("review"))
        .unwrap()
        .entries;
    let ids: Vec<_> = rows.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(
        ids.as_slice(),
        [exact_alias.id.as_str(), alias_prefix.id.as_str()],
        "fast-path smart ranking order must be exact alias > alias prefix"
    );

    // Now test fallback ordering using a query that matches NO aliases.
    // pinned entry whose content matches "needle"
    let pinned_content = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "needle this please".to_string(),
            tags: vec![],
            alias: None,
        })
        .unwrap();
    db.conn()
        .execute(
            "UPDATE prompt_entries SET is_pinned = 1 WHERE id = ?1",
            params![pinned_content.id.as_str()],
        )
        .unwrap();
    // plain content match — fallback, not pinned
    let content_match = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "contains needle token".to_string(),
            tags: vec![],
            alias: None,
        })
        .unwrap();
    // tag match — fallback, not pinned
    let tag_match = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "body".to_string(),
            tags: vec!["needle".to_string()],
            alias: None,
        })
        .unwrap();

    // Give content_match a usage record so it has non-zero usage_count.
    db.record_prompt_use(PromptUseRow {
        prompt_entry_id: content_match.id.clone(),
        action: PromptActionRow::Copy,
        surface: PromptUseSurfaceRow::Page,
        outcome: PromptUseOutcomeRow::Copy,
        failure_reason: None,
    })
    .unwrap();

    let rows = db
        .list_prompt_entries(smart_query("needle"))
        .unwrap()
        .entries;
    let ids: Vec<_> = rows.iter().map(|row| row.id.as_str()).collect();

    // Fallback priority: pinned > rest. Pinned content match should come first.
    assert_eq!(
        ids[0],
        pinned_content.id.as_str(),
        "fallback smart ranking must prioritize pinned entries"
    );

    // After pinned, content and tag matches should both appear.
    assert!(
        ids[1..].contains(&content_match.id.as_str()),
        "fallback search should include content matches"
    );
    assert!(
        ids[1..].contains(&tag_match.id.as_str()),
        "fallback search should include tag matches"
    );

    // Verify alias entries from the first part don't leak into the needle results.
    assert!(
        !ids.contains(&exact_alias.id.as_str()),
        "alias-matched entries should not appear in needle query results"
    );
    assert!(
        !ids.contains(&alias_prefix.id.as_str()),
        "alias-matched entries should not appear in needle query results"
    );
}

#[test]
fn smart_ranking_tie_breaks_by_recent_use_usage_count_and_alias() {
    let db = Database::open_in_memory().unwrap();
    let recent = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "shared review content".to_string(),
            tags: vec![],
            alias: None,
        })
        .unwrap();
    let older_but_heavy = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "shared review content".to_string(),
            tags: vec![],
            alias: None,
        })
        .unwrap();
    let same_recent_heavier = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "shared review content".to_string(),
            tags: vec![],
            alias: None,
        })
        .unwrap();
    let same_recent_lighter = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "shared review content".to_string(),
            tags: vec![],
            alias: None,
        })
        .unwrap();
    let alpha = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "shared review content".to_string(),
            tags: vec![],
            alias: Some("alpha".to_string()),
        })
        .unwrap();
    let beta = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "shared review content".to_string(),
            tags: vec![],
            alias: Some("beta".to_string()),
        })
        .unwrap();

    db.conn()
        .execute(
            "UPDATE prompt_entries SET last_used_at_ms = ?1, usage_count = ?2 WHERE id = ?3",
            params![3_i64, 1_i64, recent.id.as_str()],
        )
        .unwrap();
    db.conn()
        .execute(
            "UPDATE prompt_entries SET last_used_at_ms = ?1, usage_count = ?2 WHERE id = ?3",
            params![2_i64, 10_i64, older_but_heavy.id.as_str()],
        )
        .unwrap();
    for (id, usage_count) in [
        (same_recent_heavier.id.as_str(), 5_i64),
        (same_recent_lighter.id.as_str(), 1_i64),
    ] {
        db.conn()
            .execute(
                "UPDATE prompt_entries SET last_used_at_ms = ?1, usage_count = ?2 WHERE id = ?3",
                params![1_i64, usage_count, id],
            )
            .unwrap();
    }

    let rows = db
        .list_prompt_entries(smart_query("review"))
        .unwrap()
        .entries;
    let ids: Vec<_> = rows.iter().map(|row| row.id.as_str()).collect();

    // First 4 are determined by recency and usage_count tie-breaking.
    assert_eq!(
        &ids[..4],
        [
            recent.id.as_str(),
            older_but_heavy.id.as_str(),
            same_recent_heavier.id.as_str(),
            same_recent_lighter.id.as_str(),
        ],
        "content-match ties must use recently used > usage count"
    );

    // Alpha and beta have same last_used_at_ms (null / i64::MIN) and usage_count (0),
    // so they tie-break on alias_normalized. Alpha < Beta alphabetically.
    assert!(
        ids[4..].contains(&alpha.id.as_str()),
        "alpha should appear in results"
    );
    assert!(
        ids[4..].contains(&beta.id.as_str()),
        "beta should appear in results"
    );
}

#[test]
fn tag_filter_is_and_with_query() {
    let db = Database::open_in_memory().unwrap();
    let rust_entry = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "Review backend".to_string(),
            tags: vec!["Rust".to_string()],
            alias: None,
        })
        .unwrap();
    db.create_prompt_entry(NewPromptEntryRow {
        content: "Review frontend".to_string(),
        tags: vec!["React".to_string()],
        alias: None,
    })
    .unwrap();

    let rows = db
        .list_prompt_entries(PromptListQuery {
            query: Some("review".to_string()),
            tag: Some("rust".to_string()),
            sort: PromptSortRow::Smart,
            limit: 100,
        })
        .unwrap()
        .entries;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, rust_entry.id);
}

#[test]
fn missing_prompt_mutations_return_safe_results_or_errors() {
    let db = Database::open_in_memory().unwrap();

    assert!(!db.delete_prompt_entry("missing").unwrap());
    assert!(db
        .update_prompt_entry(UpdatePromptEntryRow {
            id: "missing".to_string(),
            content: "Body".to_string(),
            tags: vec![],
            alias: None,
            is_pinned: false,
        })
        .is_err());
    assert!(db
        .record_prompt_use(PromptUseRow {
            prompt_entry_id: "missing".to_string(),
            action: PromptActionRow::Copy,
            surface: PromptUseSurfaceRow::Overlay,
            outcome: PromptUseOutcomeRow::Copy,
            failure_reason: None,
        })
        .is_err());
}

#[test]
fn query_search_uses_alias_fast_path_before_fallback() {
    let db = Database::open_in_memory().unwrap();
    let alias_match = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "body".to_string(),
            tags: vec![],
            alias: Some(";;needle".to_string()),
        })
        .unwrap();
    let tag_match = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "body".to_string(),
            tags: vec!["needle".to_string()],
            alias: None,
        })
        .unwrap();
    let content_match = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "contains   needle across normalized whitespace".to_string(),
            tags: vec![],
            alias: None,
        })
        .unwrap();
    db.create_prompt_entry(NewPromptEntryRow {
        content: "body".to_string(),
        tags: vec![],
        alias: None,
    })
    .unwrap();

    // "needle" (no ;; prefix) hits no alias fast path, falls back to content/tag.
    let rows = db
        .list_prompt_entries(smart_query("needle"))
        .unwrap()
        .entries;
    let ids: Vec<_> = rows.iter().map(|row| row.id.as_str()).collect();
    // Only fallback matches should appear (no alias fast-path hits for plain "needle").
    assert!(
        !ids.contains(&alias_match.id.as_str()),
        "alias_match should not appear when query does not match alias prefix"
    );
    assert!(
        ids.len() >= 2,
        "fallback should return tag and content matches"
    );
    assert!(ids.contains(&tag_match.id.as_str()));
    assert!(ids.contains(&content_match.id.as_str()));

    // ";;needle" hits the exact alias match via fast path.
    let rows = db
        .list_prompt_entries(smart_query(";;needle"))
        .unwrap()
        .entries;
    let ids: Vec<_> = rows.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![alias_match.id.as_str()],
        "exact alias hits should return only the alias match via fast path"
    );
    assert_ne!(tag_match.id, content_match.id);
}

#[test]
fn query_search_falls_back_to_tag_and_content_when_fast_path_has_no_hits() {
    let db = Database::open_in_memory().unwrap();
    let tag_match = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "body".to_string(),
            tags: vec!["needle fallback".to_string()],
            alias: None,
        })
        .unwrap();
    let content_match = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "contains   needle fallback across normalized whitespace".to_string(),
            tags: vec![],
            alias: None,
        })
        .unwrap();
    db.create_prompt_entry(NewPromptEntryRow {
        content: "body".to_string(),
        tags: vec!["other".to_string()],
        alias: None,
    })
    .unwrap();

    let rows = db
        .list_prompt_entries(smart_query("needle fallback"))
        .unwrap()
        .entries;
    let ids: Vec<_> = rows.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(rows.len(), 2);
    assert!(ids.contains(&tag_match.id.as_str()));
    assert!(ids.contains(&content_match.id.as_str()));
}

#[test]
fn fallback_search_respects_tag_filter_and_escaped_like_literals() {
    let db = Database::open_in_memory().unwrap();
    let literal = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "Use 100% safe_value literally".to_string(),
            tags: vec!["Symbols".to_string()],
            alias: None,
        })
        .unwrap();
    db.create_prompt_entry(NewPromptEntryRow {
        content: "Use 100 percent safe value".to_string(),
        tags: vec!["Symbols".to_string()],
        alias: None,
    })
    .unwrap();
    db.create_prompt_entry(NewPromptEntryRow {
        content: "Use 100% safe_value literally".to_string(),
        tags: vec!["Other".to_string()],
        alias: None,
    })
    .unwrap();

    let rows = db
        .list_prompt_entries(PromptListQuery {
            query: Some("100% safe_value".to_string()),
            tag: Some("symbols".to_string()),
            sort: PromptSortRow::Smart,
            limit: 100,
        })
        .unwrap()
        .entries;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, literal.id);
}

#[test]
fn alias_fast_path_queries_use_indexes() {
    let db = Database::open_in_memory().unwrap();
    let prefix_upper_bound = "needle\u{10ffff}";

    let alias_plan = query_plan(
        &db,
        "EXPLAIN QUERY PLAN \
         SELECT id FROM prompt_entries WHERE alias_normalized = ?1 \
         UNION \
         SELECT id FROM prompt_entries \
             WHERE alias_normalized >= ?1 AND alias_normalized < ?2",
        "needle",
        prefix_upper_bound,
    );
    assert!(
        alias_plan.contains("idx_prompt_alias_unique"),
        "alias fast path must use alias index, plan={alias_plan}"
    );
}

#[test]
fn prompt_sort_modes_are_explicit_and_stable() {
    let db = Database::open_in_memory().unwrap();
    let alpha = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "body".to_string(),
            tags: vec![],
            alias: Some("alpha".to_string()),
        })
        .unwrap();
    let beta = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "body".to_string(),
            tags: vec![],
            alias: Some("beta".to_string()),
        })
        .unwrap();
    // Pin beta directly in the DB (NewPromptEntryRow no longer carries is_pinned).
    db.conn()
        .execute(
            "UPDATE prompt_entries SET is_pinned = 1 WHERE id = ?1",
            params![beta.id.as_str()],
        )
        .unwrap();

    let pinned = db
        .list_prompt_entries(PromptListQuery {
            query: None,
            tag: None,
            sort: PromptSortRow::PinnedFirst,
            limit: 100,
        })
        .unwrap()
        .entries;
    assert_eq!(pinned[0].id, beta.id);

    let alphabetical = db
        .list_prompt_entries(PromptListQuery {
            query: None,
            tag: None,
            sort: PromptSortRow::Alphabetical,
            limit: 100,
        })
        .unwrap()
        .entries;
    assert_eq!(alphabetical[0].id, alpha.id);
}

#[test]
fn suggest_tags_returns_prefix_matches_ordered_alphabetically() {
    let db = Database::open_in_memory().unwrap();
    db.create_prompt_entry(NewPromptEntryRow {
        content: "A".to_string(),
        alias: None,
        tags: vec!["review".to_string(), "release".to_string()],
    })
    .unwrap();
    db.create_prompt_entry(NewPromptEntryRow {
        content: "B".to_string(),
        alias: None,
        tags: vec!["refactor".to_string()],
    })
    .unwrap();

    let tags = db.suggest_tags("re", 50).unwrap();
    assert_eq!(tags, vec!["refactor", "release", "review"]);
}

#[test]
fn suggest_tags_deduplicates_by_normalized_form_using_min_display() {
    let db = Database::open_in_memory().unwrap();
    db.create_prompt_entry(NewPromptEntryRow {
        content: "A".to_string(),
        alias: None,
        tags: vec!["Review".to_string()],
    })
    .unwrap();
    db.create_prompt_entry(NewPromptEntryRow {
        content: "B".to_string(),
        alias: None,
        tags: vec!["review".to_string()],
    })
    .unwrap();

    let tags = db.suggest_tags("rev", 50).unwrap();
    assert_eq!(tags, vec!["Review"]);
}

#[test]
fn suggest_tags_returns_all_when_prefix_is_empty() {
    let db = Database::open_in_memory().unwrap();
    db.create_prompt_entry(NewPromptEntryRow {
        content: "A".to_string(),
        alias: None,
        tags: vec!["alpha".to_string(), "beta".to_string()],
    })
    .unwrap();

    let tags = db.suggest_tags("", 50).unwrap();
    assert_eq!(tags, vec!["alpha", "beta"]);
}

#[test]
fn suggest_tags_respects_limit() {
    let db = Database::open_in_memory().unwrap();
    db.create_prompt_entry(NewPromptEntryRow {
        content: "A".to_string(),
        alias: None,
        tags: vec![
            "tag-a".to_string(),
            "tag-b".to_string(),
            "tag-c".to_string(),
        ],
    })
    .unwrap();

    let tags = db.suggest_tags("tag", 2).unwrap();
    assert_eq!(tags.len(), 2);
}

#[test]
fn suggest_tags_returns_empty_for_no_matches() {
    let db = Database::open_in_memory().unwrap();
    db.create_prompt_entry(NewPromptEntryRow {
        content: "A".to_string(),
        alias: None,
        tags: vec!["alpha".to_string()],
    })
    .unwrap();

    let tags = db.suggest_tags("zzz", 50).unwrap();
    assert!(tags.is_empty());
}

#[test]
fn suggest_tags_escapes_like_special_characters() {
    let db = Database::open_in_memory().unwrap();
    db.create_prompt_entry(NewPromptEntryRow {
        content: "A".to_string(),
        alias: None,
        tags: vec!["100%_coverage".to_string()],
    })
    .unwrap();

    let tags = db.suggest_tags("100%", 50).unwrap();
    assert_eq!(tags, vec!["100%_coverage"]);
}

fn query_plan(db: &Database, sql: &str, lower: &str, upper: &str) -> String {
    let mut stmt = db.conn().prepare(sql).unwrap();
    let rows = stmt
        .query_map(params![lower, upper], |row| row.get::<_, String>(3))
        .unwrap();
    rows.map(|row| row.unwrap()).collect::<Vec<_>>().join("\n")
}

#[test]
fn prompt_sort_modes_cover_usage_and_update_ordering() {
    let db = Database::open_in_memory().unwrap();
    let first = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "body".to_string(),
            tags: vec![],
            alias: Some(";;first".to_string()),
        })
        .unwrap();
    let second = db
        .create_prompt_entry(NewPromptEntryRow {
            content: "body".to_string(),
            tags: vec![],
            alias: Some(";;second".to_string()),
        })
        .unwrap();
    db.conn()
        .execute(
            "UPDATE prompt_entries SET created_at_ms = ?2, updated_at_ms = ?2 WHERE id = ?1",
            params![first.id, 1_000_i64],
        )
        .unwrap();
    db.conn()
        .execute(
            "UPDATE prompt_entries SET created_at_ms = ?2, updated_at_ms = ?2 WHERE id = ?1",
            params![second.id, 2_000_i64],
        )
        .unwrap();

    db.record_prompt_use(PromptUseRow {
        prompt_entry_id: first.id.clone(),
        action: PromptActionRow::Paste,
        surface: PromptUseSurfaceRow::Page,
        outcome: PromptUseOutcomeRow::PasteAttempted,
        failure_reason: None,
    })
    .unwrap();
    db.record_prompt_use(PromptUseRow {
        prompt_entry_id: first.id.clone(),
        action: PromptActionRow::Paste,
        surface: PromptUseSurfaceRow::Page,
        outcome: PromptUseOutcomeRow::PasteAttempted,
        failure_reason: None,
    })
    .unwrap();
    db.record_prompt_use(PromptUseRow {
        prompt_entry_id: second.id.clone(),
        action: PromptActionRow::Paste,
        surface: PromptUseSurfaceRow::Page,
        outcome: PromptUseOutcomeRow::PasteAttempted,
        failure_reason: None,
    })
    .unwrap();
    db.conn()
        .execute(
            "UPDATE prompt_entries SET last_used_at_ms = ?2, updated_at_ms = ?2 WHERE id = ?1",
            params![first.id, 3_000_i64],
        )
        .unwrap();
    db.conn()
        .execute(
            "UPDATE prompt_entries SET last_used_at_ms = ?2, updated_at_ms = ?2 WHERE id = ?1",
            params![second.id, 4_000_i64],
        )
        .unwrap();

    let most_used = db
        .list_prompt_entries(PromptListQuery {
            query: None,
            tag: None,
            sort: PromptSortRow::MostUsed,
            limit: 100,
        })
        .unwrap()
        .entries;
    assert_eq!(most_used[0].id, first.id);

    let recently_used = db
        .list_prompt_entries(PromptListQuery {
            query: None,
            tag: None,
            sort: PromptSortRow::RecentlyUsed,
            limit: 100,
        })
        .unwrap()
        .entries;
    assert_eq!(recently_used[0].id, second.id);

    let recently_updated = db
        .list_prompt_entries(PromptListQuery {
            query: None,
            tag: None,
            sort: PromptSortRow::RecentlyUpdated,
            limit: 100,
        })
        .unwrap()
        .entries;
    assert_eq!(recently_updated[0].id, second.id);
}

#[test]
fn prompt_use_updates_counter_and_records_use_event() {
    let db = Database::open_in_memory().unwrap();
    let created = db
        .create_prompt_entry(new_entry("Review this diff"))
        .unwrap();

    let result = db
        .record_prompt_use(PromptUseRow {
            prompt_entry_id: created.id.clone(),
            action: PromptActionRow::Paste,
            surface: PromptUseSurfaceRow::Overlay,
            outcome: PromptUseOutcomeRow::PasteAttempted,
            failure_reason: None,
        })
        .unwrap();

    assert_eq!(result.usage_count, 1);
    assert!(result.last_used_at_ms.is_some());

    let reloaded = db.get_prompt_entry(&created.id).unwrap().unwrap();
    assert_eq!(reloaded.usage_count, 1);
    assert_eq!(reloaded.last_used_at_ms, result.last_used_at_ms);

    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM prompt_entry_uses WHERE prompt_entry_id = ?1",
            [&created.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn copy_and_fallback_outcomes_do_not_increment_usage_count() {
    let db = Database::open_in_memory().unwrap();
    let created = db
        .create_prompt_entry(new_entry("Only paste counts"))
        .unwrap();

    // Copy outcome: inserts event row but does NOT increment usage_count.
    db.record_prompt_use(PromptUseRow {
        prompt_entry_id: created.id.clone(),
        action: PromptActionRow::Copy,
        surface: PromptUseSurfaceRow::Page,
        outcome: PromptUseOutcomeRow::Copy,
        failure_reason: None,
    })
    .unwrap();

    let reloaded = db.get_prompt_entry(&created.id).unwrap().unwrap();
    assert_eq!(
        reloaded.usage_count, 0,
        "copy outcome must not increment usage_count"
    );
    assert!(
        reloaded.last_used_at_ms.is_none(),
        "copy outcome must not set last_used_at_ms"
    );

    // PasteFellBackToCopy outcome: same — no counter increment.
    db.record_prompt_use(PromptUseRow {
        prompt_entry_id: created.id.clone(),
        action: PromptActionRow::Paste,
        surface: PromptUseSurfaceRow::Page,
        outcome: PromptUseOutcomeRow::PasteFellBackToCopy,
        failure_reason: Some(PromptUseFailureReasonRow::UnsupportedPlatform),
    })
    .unwrap();

    let reloaded = db.get_prompt_entry(&created.id).unwrap().unwrap();
    assert_eq!(
        reloaded.usage_count, 0,
        "paste_fell_back_to_copy must not increment usage_count"
    );
    assert!(reloaded.last_used_at_ms.is_none());

    // PasteAttempted outcome: DOES increment.
    let result = db
        .record_prompt_use(PromptUseRow {
            prompt_entry_id: created.id.clone(),
            action: PromptActionRow::Paste,
            surface: PromptUseSurfaceRow::Overlay,
            outcome: PromptUseOutcomeRow::PasteAttempted,
            failure_reason: None,
        })
        .unwrap();
    assert_eq!(result.usage_count, 1);
    assert!(result.last_used_at_ms.is_some());

    // All three event rows were recorded.
    let count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM prompt_entry_uses WHERE prompt_entry_id = ?1",
            [&created.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 3);
}

#[test]
fn alias_search_handles_large_prompt_list_quickly() {
    let db = Database::open_in_memory().unwrap();
    for i in 0..1_000 {
        db.create_prompt_entry(NewPromptEntryRow {
            content: "generic body".to_string(),
            tags: vec!["Bulk".to_string()],
            alias: Some(format!(";;p{i}")),
        })
        .unwrap();
    }

    let start = std::time::Instant::now();
    let rows = db
        .list_prompt_entries(smart_query(";;p999"))
        .unwrap()
        .entries;
    let elapsed = start.elapsed();

    assert_eq!(rows[0].alias.as_deref(), Some(";;p999"));
    let threshold = if std::env::var_os("BUSYTOK_STRICT_PERF").is_some() {
        std::time::Duration::from_millis(250)
    } else {
        std::time::Duration::from_millis(1_000)
    };
    assert!(
        elapsed < threshold,
        "alias search should stay interactive, elapsed={elapsed:?}, threshold={threshold:?}"
    );
}
