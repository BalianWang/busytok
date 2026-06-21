/// Integration test verifying that `enrich_cost` uses a single catalog snapshot
/// for both cost estimation and version stamping. Runs in its own test binary
/// to avoid polluting the global catalog state for other runtime tests.
use busytok_domain::{AgentKind, NormalizedUsageEvent};
use busytok_pricing::CostMode;
use busytok_runtime::scan::enrich_cost;

#[test]
fn enrich_cost_snapshot_is_consistent_across_global_swap() {
    let tmp = tempfile::tempdir().unwrap();
    let path_a = tmp.path().join("catalog-a.json");
    let path_b = tmp.path().join("catalog-b.json");

    // Catalog A: claude-sonnet-4-20250514 at $1/m input, $1/m output.
    let json_a = r#"{"schema_version":"3","version":"catalog-A","updated":"2099-01-01","aliases":{},"prices":[{"provider":"anthropic","model":"claude-sonnet-4-20250514","currency":"USD","effective_date":"2099-01-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":1.0,"output_per_million":1.0,"cached_input_per_million":0.1,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;
    // Catalog B: same model at $999/m.
    let json_b = r#"{"schema_version":"3","version":"catalog-B","updated":"2099-06-01","aliases":{},"prices":[{"provider":"anthropic","model":"claude-sonnet-4-20250514","currency":"USD","effective_date":"2099-06-01","fast_multiplier":null,"tiers":[{"from_tokens":0,"input_per_million":999.0,"output_per_million":999.0,"cached_input_per_million":99.0,"cache_write_per_million":null,"cache_storage_per_million_hour":null,"reasoning_per_million":null}]}]}"#;

    std::fs::write(&path_a, json_a).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(&path_b, json_b).unwrap();

    // Pre-compute expected cost from catalog A.
    let catalog_a: busytok_pricing::PriceCatalog = serde_json::from_str(json_a).unwrap();
    let expected_cost = busytok_pricing::estimate_cost_with_catalog(
        &catalog_a,
        "claude-sonnet-4-20250514",
        busytok_pricing::TokenUsage {
            input_tokens: 100_000,
            output_tokens: 50_000,
            cached_input_tokens: 0,
            cache_creation_tokens: 0,
            reasoning_tokens: 0,
        },
        None,
        None,
        busytok_pricing::CostMode::Calculate,
    )
    .unwrap();

    // Set global to catalog A.
    busytok_pricing::init_catalog(Some(&path_a));

    // Call enrich_cost — snapshot should be catalog A.
    let mut event = NormalizedUsageEvent::minimal_for_test("test-snapshot", AgentKind::ClaudeCode);
    event.model = Some("claude-sonnet-4-20250514".to_string());
    event.input_tokens = 100_000;
    event.output_tokens = 50_000;
    enrich_cost(&mut event, CostMode::Auto);

    // Now swap global to catalog B.
    std::thread::sleep(std::time::Duration::from_millis(10));
    let _ = busytok_pricing::try_reload_catalog(&path_b);

    // The event was enriched BEFORE the swap to catalog B.
    let actual_cost = event.estimated_cost_usd.unwrap();
    assert!(
        (actual_cost - expected_cost).abs() < 0.0001,
        "cost should be from catalog A ({expected_cost}), got {actual_cost}"
    );
    assert_eq!(
        event.price_catalog_version.as_deref(),
        Some("catalog-A"),
        "version should be from catalog A, not catalog B"
    );

    // Restore embedded catalog.
    busytok_pricing::init_catalog(None::<&std::path::Path>);
}
