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
use std::collections::HashMap;

use busytok_pricing::{
    estimate_cost_with_catalog, init_catalog, load_catalog, try_reload_catalog, CostMode,
    ModelPrice, PriceCatalog, PriceTier, ReloadResult, TierMode, TokenUsage,
};
use serial_test::serial;

fn zero_usage() -> TokenUsage {
    TokenUsage {
        input_tokens: 0,
        output_tokens: 0,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        reasoning_tokens: 0,
    }
}

fn make_catalog(model: &str, input_rate: f64, output_rate: f64) -> PriceCatalog {
    PriceCatalog {
        schema_version: "3".to_string(),
        version: "test".to_string(),
        updated: "test".to_string(),
        aliases: HashMap::new(),
        prices: vec![ModelPrice {
            provider: "test".to_string(),
            model: model.to_string(),
            currency: "USD".to_string(),
            effective_date: "2099-01-01".to_string(),
            fast_multiplier: None,
            tier_mode: TierMode::Marginal,
            tiers: vec![PriceTier {
                from_tokens: 0,
                input_per_million: input_rate,
                output_per_million: output_rate,
                cached_input_per_million: None,
                cache_write_per_million: None,
                cache_storage_per_million_hour: None,
                reasoning_per_million: None,
            }],
        }],
    }
}

fn write_with_distinct_mtime(path: &std::path::Path, content: &str) {
    let _ = std::fs::remove_file(path);
    std::fs::write(path, content).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
}

#[test]
#[serial]
fn prefers_agent_provided_cost_when_available() {
    let catalog = make_catalog("gpt-5", 1.25, 10.0);
    let usage = TokenUsage {
        input_tokens: 1_000,
        output_tokens: 1_000,
        ..zero_usage()
    };
    assert_eq!(
        estimate_cost_with_catalog(&catalog, "gpt-5", usage, Some(0.42), None, CostMode::Auto)
            .unwrap(),
        0.42
    );
}

#[test]
#[serial]
fn clamps_cached_tokens_to_input_tokens() {
    let catalog = make_catalog("gpt-5", 1.25, 10.0);
    let usage = TokenUsage {
        input_tokens: 100,
        output_tokens: 0,
        cached_input_tokens: 200,
        ..zero_usage()
    };
    let cost =
        estimate_cost_with_catalog(&catalog, "gpt-5", usage, None, None, CostMode::Auto).unwrap();
    // cached clamped to 100, non_cached = 0, catalog has no cached_input rate → cost = 0
    assert!((cost - 0.0).abs() < 0.0001);
}

#[test]
#[serial]
fn unknown_model_returns_none() {
    let catalog = make_catalog("gpt-5", 1.25, 10.0);
    assert!(estimate_cost_with_catalog(
        &catalog,
        "unknown-model-xyz",
        zero_usage(),
        None,
        None,
        CostMode::Auto
    )
    .is_none());
}

#[test]
#[serial]
fn catalog_loads_from_embedded_json() {
    let catalog = load_catalog();
    assert!(!catalog.prices.is_empty());
    assert_eq!(catalog.schema_version, "3");
}

#[test]
#[serial]
fn embedded_catalog_includes_openai_56_and_claude_5_models() {
    let catalog = load_catalog();

    let sol = catalog
        .resolve_model("gpt-5.6-sol")
        .expect("gpt-5.6-sol should exist");
    assert_eq!(sol.provider, "openai");
    assert_eq!(sol.tiers[0].input_per_million, 5.0);
    assert_eq!(sol.tiers[0].output_per_million, 25.0);
    assert_eq!(sol.tiers[0].cached_input_per_million, Some(0.5));
    assert_eq!(sol.tiers[0].cache_write_per_million, Some(6.25));

    let sonnet = catalog
        .resolve_model("claude-5-sonnet")
        .expect("claude-5-sonnet alias should resolve");
    assert_eq!(sonnet.model, "claude-sonnet-5");
    assert_eq!(sonnet.provider, "anthropic");
    assert_eq!(sonnet.tiers[0].input_per_million, 2.0);
    assert_eq!(sonnet.tiers[0].output_per_million, 10.0);
    assert_eq!(sonnet.tiers[0].cached_input_per_million, Some(0.2));
    assert_eq!(sonnet.tiers[0].cache_write_per_million, Some(2.5));

    let mythos = catalog
        .resolve_model("claude-mythos-5")
        .expect("claude-mythos-5 should exist");
    assert_eq!(mythos.provider, "anthropic");
    assert_eq!(mythos.tiers[0].input_per_million, 7.0);
    assert_eq!(mythos.tiers[0].output_per_million, 35.0);
    assert_eq!(mythos.tiers[0].cached_input_per_million, Some(0.7));
    assert_eq!(mythos.tiers[0].cache_write_per_million, Some(8.75));
}

#[test]
#[serial]
fn reasoning_tokens_not_double_counted() {
    let catalog = PriceCatalog {
        schema_version: "3".to_string(),
        version: "test".to_string(),
        updated: "test".to_string(),
        aliases: HashMap::new(),
        prices: vec![ModelPrice {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            currency: "USD".to_string(),
            effective_date: "2099-01-01".to_string(),
            fast_multiplier: None,
            tier_mode: TierMode::Marginal,
            tiers: vec![PriceTier {
                from_tokens: 0,
                input_per_million: 3.0,
                output_per_million: 15.0,
                cached_input_per_million: Some(0.3),
                cache_write_per_million: Some(3.75),
                cache_storage_per_million_hour: None,
                reasoning_per_million: None,
            }],
        }],
    };
    let usage = TokenUsage {
        input_tokens: 100,
        output_tokens: 50,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        reasoning_tokens: 200,
    };
    let cost = estimate_cost_with_catalog(
        &catalog,
        "claude-sonnet-4-20250514",
        usage,
        None,
        None,
        CostMode::Auto,
    )
    .unwrap();
    let expected = 100.0 * 3.0 / 1_000_000.0 + 50.0 * 15.0 / 1_000_000.0;
    assert!((cost - expected).abs() < 0.0001);
}

#[test]
#[serial]
fn alias_maps_to_correct_model() {
    let catalog = PriceCatalog {
        schema_version: "3".to_string(),
        version: "test".to_string(),
        updated: "test".to_string(),
        aliases: HashMap::from([("gpt-5-codex".to_string(), "gpt-5".to_string())]),
        prices: vec![ModelPrice {
            provider: "openai".to_string(),
            model: "gpt-5".to_string(),
            currency: "USD".to_string(),
            effective_date: "2099-01-01".to_string(),
            fast_multiplier: None,
            tier_mode: TierMode::Marginal,
            tiers: vec![PriceTier {
                from_tokens: 0,
                input_per_million: 1.0,
                output_per_million: 2.0,
                cached_input_per_million: None,
                cache_write_per_million: None,
                cache_storage_per_million_hour: None,
                reasoning_per_million: None,
            }],
        }],
    };
    let usage = TokenUsage {
        input_tokens: 1_000,
        output_tokens: 500,
        ..zero_usage()
    };
    let alias_cost =
        estimate_cost_with_catalog(&catalog, "gpt-5-codex", usage, None, None, CostMode::Auto)
            .unwrap();
    let direct_cost =
        estimate_cost_with_catalog(&catalog, "gpt-5", usage, None, None, CostMode::Auto).unwrap();
    assert!((alias_cost - direct_cost).abs() < f64::EPSILON);
}

#[test]
#[serial]
fn unknown_model_not_fuzzy_matched() {
    let catalog = make_catalog("gpt-5.3-codex", 1.75, 14.0);
    let usage = TokenUsage {
        input_tokens: 1_000,
        output_tokens: 500,
        ..zero_usage()
    };
    assert!(estimate_cost_with_catalog(
        &catalog,
        "gpt-5.3-codex-experimental",
        usage,
        None,
        None,
        CostMode::Auto
    )
    .is_none());
}

#[test]
#[serial]
fn cache_write_pricing_separate_from_cache_read() {
    let catalog = PriceCatalog {
        schema_version: "3".to_string(),
        version: "test".to_string(),
        updated: "test".to_string(),
        aliases: HashMap::new(),
        prices: vec![ModelPrice {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            currency: "USD".to_string(),
            effective_date: "2099-01-01".to_string(),
            fast_multiplier: None,
            tier_mode: TierMode::Marginal,
            tiers: vec![PriceTier {
                from_tokens: 0,
                input_per_million: 3.0,
                output_per_million: 15.0,
                cached_input_per_million: Some(0.3),
                cache_write_per_million: Some(3.75),
                cache_storage_per_million_hour: None,
                reasoning_per_million: None,
            }],
        }],
    };
    let usage = TokenUsage {
        input_tokens: 300_000,
        output_tokens: 0,
        cached_input_tokens: 100_000,
        cache_creation_tokens: 50_000,
        reasoning_tokens: 0,
    };
    let cost = estimate_cost_with_catalog(
        &catalog,
        "claude-sonnet-4-20250514",
        usage,
        None,
        None,
        CostMode::Calculate,
    )
    .unwrap();
    let expected = 150_000.0 * 3.0 / 1_000_000.0
        + 100_000.0 * 0.3 / 1_000_000.0
        + 50_000.0 * 3.75 / 1_000_000.0;
    assert!((cost - expected).abs() < 0.0001);
}

#[test]
#[serial]
fn reload_lifecycle_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("price-catalog.json");

    init_catalog(None::<&std::path::Path>);
    assert_eq!(load_catalog().schema_version, "3");

    let v1 = r#"{"schema_version":"3","version":"2099-e2e","updated":"2099-06-01","aliases":{},"prices":[{"provider":"test","model":"test-hot","currency":"USD","effective_date":"2099-06-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":5.0,"output_per_million":10.0,"cached_input_per_million":null,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
    write_with_distinct_mtime(&path, v1);
    match try_reload_catalog(&path) {
        ReloadResult::Reloaded { version } => assert_eq!(version, "2099-e2e"),
        other => panic!("expected Reloaded, got {:?}", other),
    }
    assert_eq!(load_catalog().version, "2099-e2e");

    let usage = TokenUsage {
        input_tokens: 1_000_000,
        output_tokens: 0,
        ..zero_usage()
    };
    let cost = estimate_cost_with_catalog(
        &load_catalog(),
        "test-hot",
        usage,
        None,
        None,
        CostMode::Calculate,
    )
    .unwrap();
    assert!((cost - 5.0).abs() < 0.0001);

    assert!(matches!(try_reload_catalog(&path), ReloadResult::Unchanged));

    write_with_distinct_mtime(&path, "broken{{{");
    assert!(matches!(
        try_reload_catalog(&path),
        ReloadResult::ParseError { .. }
    ));
    assert_eq!(load_catalog().version, "2099-e2e");

    let missing = tmp.path().join("does-not-exist.json");
    assert!(matches!(
        try_reload_catalog(&missing),
        ReloadResult::Missing
    ));

    let dir = tmp.path().join("a-directory");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(matches!(
        try_reload_catalog(&dir),
        ReloadResult::IoError { .. }
    ));

    init_catalog(None::<&std::path::Path>);
}

#[test]
#[serial]
fn whole_request_tier_pricing() {
    let catalog = PriceCatalog {
        schema_version: "3".to_string(),
        version: "test".to_string(),
        updated: "test".to_string(),
        aliases: HashMap::new(),
        prices: vec![ModelPrice {
            provider: "test".to_string(),
            model: "test-whole-request".to_string(),
            currency: "USD".to_string(),
            effective_date: "2099-01-01".to_string(),
            fast_multiplier: None,
            tier_mode: TierMode::WholeRequest,
            tiers: vec![
                PriceTier {
                    from_tokens: 0,
                    input_per_million: 1.25,
                    output_per_million: 10.0,
                    cached_input_per_million: Some(0.125),
                    cache_write_per_million: None,
                    cache_storage_per_million_hour: None,
                    reasoning_per_million: None,
                },
                PriceTier {
                    from_tokens: 200001,
                    input_per_million: 2.50,
                    output_per_million: 15.0,
                    cached_input_per_million: Some(0.25),
                    cache_write_per_million: None,
                    cache_storage_per_million_hour: None,
                    reasoning_per_million: None,
                },
            ],
        }],
    };

    // Below 200k: all tokens at tier 0 rates
    let usage_below = TokenUsage {
        input_tokens: 100_000,
        output_tokens: 50_000,
        ..zero_usage()
    };
    let cost_below = estimate_cost_with_catalog(
        &catalog,
        "test-whole-request",
        usage_below,
        None,
        None,
        CostMode::Calculate,
    )
    .unwrap();
    let expected_below = 100_000.0 * 1.25 / 1_000_000.0 + 50_000.0 * 10.0 / 1_000_000.0;
    assert!((cost_below - expected_below).abs() < 0.0001);

    // Above 200k: all tokens at tier 1 rates (not segmented)
    let usage_above = TokenUsage {
        input_tokens: 300_000,
        output_tokens: 50_000,
        ..zero_usage()
    };
    let cost_above = estimate_cost_with_catalog(
        &catalog,
        "test-whole-request",
        usage_above,
        None,
        None,
        CostMode::Calculate,
    )
    .unwrap();
    let expected_above = 300_000.0 * 2.50 / 1_000_000.0 + 50_000.0 * 15.0 / 1_000_000.0;
    assert!((cost_above - expected_above).abs() < 0.0001);
}
