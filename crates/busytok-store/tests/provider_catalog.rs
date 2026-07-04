#![allow(unused_imports)]
use busytok_domain::{ModelCatalogFilter, ProviderKind};
use busytok_store::Database;
use busytok_store::{
    CreateModelReq, CreateProviderReq, ModelCatalogEntry, Provider, ProviderSummary,
    UpdateModelPatch, UpdateProviderPatch,
};

fn sample_provider_req() -> CreateProviderReq {
    CreateProviderReq {
        name: "Test Provider".to_string(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://api.test.com".to_string(),
        enabled: true,
        api_key: Some("sk-test-key".to_string()),
    }
}

#[test]
fn provider_crud_round_trip() {
    let db = Database::open_in_memory().unwrap();
    let created = db.create_provider(sample_provider_req()).unwrap();
    // id is system-generated (UUID v4)
    assert!(!created.id.is_empty());
    assert!(created.api_key.is_some());

    let summary = db.list_providers().unwrap();
    assert_eq!(summary.len(), 1);
    assert!(summary[0].has_api_key);

    let updated = db
        .update_provider(
            &created.id,
            UpdateProviderPatch {
                name: Some("Updated".to_string()),
                base_url: None,
                enabled: None,
                provider_kind: None,
                api_key: Some(Some("sk-new-key".to_string())),
            },
        )
        .unwrap();
    assert_eq!(updated.name, "Updated");

    let with_secret = db.get_provider_with_secret(&created.id).unwrap().unwrap();
    assert_eq!(with_secret.api_key.as_deref(), Some("sk-new-key"));

    db.delete_provider(&created.id).unwrap();
    assert!(db.list_providers().unwrap().is_empty());
}

#[test]
fn model_crud_and_cascade_tags() {
    let db = Database::open_in_memory().unwrap();
    let provider = db.create_provider(sample_provider_req()).unwrap();

    let model = db
        .create_model(CreateModelReq {
            provider_id: provider.id.clone(),
            model_id: "gpt-4o".to_string(),
            enabled: true,
            tags: vec!["fast".to_string(), "cheap".to_string()],
            display_name: None,
            reasoning: None,
            context_window: None,
            max_tokens: None,
        })
        .unwrap();
    assert_eq!(model.model_id, "gpt-4o");

    // Duplicate (provider_id, model_id) rejected
    let dup = db.create_model(CreateModelReq {
        provider_id: provider.id.clone(),
        model_id: "gpt-4o".to_string(),
        enabled: true,
        tags: vec![],
        display_name: None,
        reasoning: None,
        context_window: None,
        max_tokens: None,
    });
    assert!(dup.is_err());

    // List tags
    let tags = db.list_tags().unwrap();
    assert!(tags.contains(&"fast".to_string()));
    assert!(tags.contains(&"cheap".to_string()));

    // Delete model cascades tags
    db.delete_model(&model.id).unwrap();
    let entries = db
        .list_models_filtered(ModelCatalogFilter::default())
        .unwrap();
    assert!(entries.is_empty());
}

#[test]
fn list_models_filtered_by_multiple_tags_and_semantics() {
    let db = Database::open_in_memory().unwrap();
    let provider = db.create_provider(sample_provider_req()).unwrap();

    db.create_model(CreateModelReq {
        provider_id: provider.id.clone(),
        model_id: "gpt-4o".into(),
        enabled: true,
        tags: vec!["fast".into(), "cheap".into()],
        display_name: None,
        reasoning: None,
        context_window: None,
        max_tokens: None,
    })
    .unwrap();
    db.create_model(CreateModelReq {
        provider_id: provider.id.clone(),
        model_id: "gpt-4o-mini".into(),
        enabled: true,
        tags: vec!["fast".into()],
        display_name: None,
        reasoning: None,
        context_window: None,
        max_tokens: None,
    })
    .unwrap();

    // AND semantics: only model with both tags
    let entries = db
        .list_models_filtered(ModelCatalogFilter {
            provider_id: None,
            tags: vec!["fast".into(), "cheap".into()],
            include_disabled: false,
        })
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].model_id, "gpt-4o");
}

#[test]
fn include_disabled_filters_both_provider_and_model() {
    let db = Database::open_in_memory().unwrap();
    let p_enabled = db
        .create_provider(CreateProviderReq {
            name: "Enabled".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://a.com".into(),
            enabled: true,
            api_key: Some("k".into()),
        })
        .unwrap();
    let p_disabled = db
        .create_provider(CreateProviderReq {
            name: "Disabled".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://b.com".into(),
            enabled: false,
            api_key: None,
        })
        .unwrap();

    db.create_model(CreateModelReq {
        provider_id: p_enabled.id.clone(),
        model_id: "m-enabled".into(),
        enabled: true,
        tags: vec![],
        display_name: None,
        reasoning: None,
        context_window: None,
        max_tokens: None,
    })
    .unwrap();
    db.create_model(CreateModelReq {
        provider_id: p_enabled.id.clone(),
        model_id: "m-disabled".into(),
        enabled: false,
        tags: vec![],
        display_name: None,
        reasoning: None,
        context_window: None,
        max_tokens: None,
    })
    .unwrap();
    db.create_model(CreateModelReq {
        provider_id: p_disabled.id.clone(),
        model_id: "m-under-disabled".into(),
        enabled: true,
        tags: vec![],
        display_name: None,
        reasoning: None,
        context_window: None,
        max_tokens: None,
    })
    .unwrap();

    // include_disabled=false: only enabled provider + enabled model
    let entries = db
        .list_models_filtered(ModelCatalogFilter {
            provider_id: None,
            tags: vec![],
            include_disabled: false,
        })
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].model_id, "m-enabled");

    // include_disabled=true: all 3
    let entries = db
        .list_models_filtered(ModelCatalogFilter {
            provider_id: None,
            tags: vec![],
            include_disabled: true,
        })
        .unwrap();
    assert_eq!(entries.len(), 3);
}

#[test]
fn delete_provider_succeeds_even_when_subagent_bound() {
    // Task 3 spec §7.5: deleting a provider is always allowed even when a
    // subagent references it via `bound_provider_id`. The resulting dangling
    // binding is reported at delegate time (fail-fast). The store layer no
    // longer takes a `profile_refs` parameter — the blocking-check helpers
    // (`provider_has_profile_references` / `model_has_profile_references`)
    // and `ProfileModelRef` have been deleted.
    let db = Database::open_in_memory().unwrap();
    let provider = db.create_provider(sample_provider_req()).unwrap();
    let pid = provider.id.clone();
    let model = db
        .create_model(CreateModelReq {
            provider_id: pid.clone(),
            model_id: "gpt-4o".into(),
            enabled: true,
            tags: vec![],
            display_name: None,
            reasoning: None,
            context_window: None,
            max_tokens: None,
        })
        .unwrap();

    // Insert a subagent that binds to this provider (dangling allowed).
    db.subagent_upsert_logical(&busytok_store::SubagentLogicalSubagentRow {
        id: "sub-1".into(),
        name: "bound".into(),
        project_id: "h".into(),
        repo_path: "/tmp".into(),
        repo_hash: "h".into(),
        branch: None,
        intent: None,
        default_profile: "pi/search-cheap".into(),
        bound_provider_id: pid.clone(),
        bound_model_id: "gpt-4o".into(),
        status: "cold".into(),
        created_at_ms: 1000,
        updated_at_ms: 1000,
        last_active_at_ms: None,
    })
    .unwrap();

    // delete_model should succeed (dangling binding allowed per spec §7.5).
    // Must run before delete_provider: models.provider_id has ON DELETE
    // CASCADE, so deleting the provider first would remove the model row
    // and make the subsequent delete_model fail with "model not found".
    db.delete_model(&model.id).unwrap();
    // Subagent row remains — its `bound_model_id` is now dangling.
    assert!(db.subagent_get_logical("sub-1").unwrap().is_some());

    // Same semantics for delete_provider: dangling binding allowed.
    db.delete_provider(&pid).unwrap();
    assert!(db.subagent_get_logical("sub-1").unwrap().is_some());
}

#[test]
fn model_update_lookup_by_provider_and_tags_setter() {
    // Covers update_model (enabled flip + no-op patch),
    // get_model_by_provider_and_model_id, list_models_by_provider,
    // set_model_tags (add/remove/no-op), update_provider api_key clear/no-op
    // branches, and update_provider base_url/enabled patches.
    let db = Database::open_in_memory().unwrap();
    let provider = db.create_provider(sample_provider_req()).unwrap();
    let pid = provider.id.clone();

    let model = db
        .create_model(CreateModelReq {
            provider_id: pid.clone(),
            model_id: "gpt-4o".into(),
            enabled: true,
            tags: vec!["fast".into()],
            display_name: None,
            reasoning: None,
            context_window: None,
            max_tokens: None,
        })
        .unwrap();

    // get_model_by_provider_and_model_id finds it
    let found = db
        .get_model_by_provider_and_model_id(&pid, "gpt-4o")
        .unwrap()
        .expect("model should be found by (provider_id, model_id)");
    assert_eq!(found.id, model.id);

    // update_model: disable
    let disabled = db
        .update_model(
            &model.id,
            UpdateModelPatch {
                enabled: Some(false),
                display_name: None,
                reasoning: None,
                context_window: None,
                max_tokens: None,
            },
        )
        .unwrap();
    assert!(!disabled.enabled);

    // update_model: re-enable
    let reenabled = db
        .update_model(
            &model.id,
            UpdateModelPatch {
                enabled: Some(true),
                display_name: None,
                reasoning: None,
                context_window: None,
                max_tokens: None,
            },
        )
        .unwrap();
    assert!(reenabled.enabled);

    // update_model: empty patch (all None) bails with "model update patch is empty"
    let err = db.update_model(
        &model.id,
        UpdateModelPatch {
            enabled: None,
            display_name: None,
            reasoning: None,
            context_window: None,
            max_tokens: None,
        },
    );
    assert!(err.is_err());

    // update_model on missing id errors
    let err = db.update_model(
        "nonexistent",
        UpdateModelPatch {
            enabled: Some(true),
            display_name: None,
            reasoning: None,
            context_window: None,
            max_tokens: None,
        },
    );
    assert!(err.is_err());

    // update_model empty patch on missing id also errors (bails before existence check)
    let err = db.update_model(
        "nonexistent",
        UpdateModelPatch {
            enabled: None,
            display_name: None,
            reasoning: None,
            context_window: None,
            max_tokens: None,
        },
    );
    assert!(err.is_err());

    // list_models_by_provider returns the model (include_disabled=true)
    let by_provider = db.list_models_by_provider(&pid).unwrap();
    assert_eq!(by_provider.len(), 1);
    assert_eq!(by_provider[0].model_id, "gpt-4o");

    // set_model_tags: add a new tag, keep existing
    db.set_model_tags(&model.id, &["fast".into(), "cheap".into()])
        .unwrap();
    let entries = db
        .list_models_filtered(ModelCatalogFilter {
            provider_id: None,
            tags: vec!["fast".into(), "cheap".into()],
            include_disabled: false,
        })
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].tags.len(), 2);

    // set_model_tags: remove a tag
    db.set_model_tags(&model.id, &["fast".into()]).unwrap();
    let entries = db
        .list_models_filtered(ModelCatalogFilter {
            provider_id: None,
            tags: vec!["cheap".into()],
            include_disabled: false,
        })
        .unwrap();
    // "cheap" was removed, so AND-filter for ["cheap"] yields nothing
    assert!(entries.is_empty());

    // set_model_tags: no-op (same set) must not bump timestamp
    let before = db.get_model_by_id(&model.id).unwrap().unwrap();
    db.set_model_tags(&model.id, &["fast".into()]).unwrap();
    let after = db.get_model_by_id(&model.id).unwrap().unwrap();
    assert_eq!(before.updated_at_ms, after.updated_at_ms);

    // update_provider: clear api_key (Some(None))
    let cleared = db
        .update_provider(
            &pid,
            UpdateProviderPatch {
                name: None,
                base_url: None,
                enabled: None,
                provider_kind: None,
                api_key: Some(None),
            },
        )
        .unwrap();
    assert!(cleared.api_key.is_none());

    // update_provider: no-op api_key (None) keeps cleared state
    let unchanged = db
        .update_provider(
            &pid,
            UpdateProviderPatch {
                name: None,
                base_url: None,
                enabled: None,
                provider_kind: None,
                api_key: None,
            },
        )
        .unwrap();
    assert!(unchanged.api_key.is_none());

    // update_provider: base_url + enabled patches
    let patched = db
        .update_provider(
            &pid,
            UpdateProviderPatch {
                name: None,
                base_url: Some("https://new.url".into()),
                enabled: Some(false),
                provider_kind: None,
                api_key: None,
            },
        )
        .unwrap();
    assert_eq!(patched.base_url, "https://new.url");
    assert!(!patched.enabled);

    // update_provider on missing id errors (even with empty patch)
    let err = db.update_provider(
        "nonexistent",
        UpdateProviderPatch {
            name: None,
            base_url: None,
            enabled: None,
            provider_kind: None,
            api_key: None,
        },
    );
    assert!(err.is_err());

    // delete_provider on missing id errors
    let err = db.delete_provider("nonexistent");
    assert!(err.is_err());

    // get_model_by_id on missing id returns None
    assert!(db.get_model_by_id("nonexistent").unwrap().is_none());

    // get_model_by_provider_and_model_id on missing returns None
    assert!(db
        .get_model_by_provider_and_model_id(&pid, "no-such-model")
        .unwrap()
        .is_none());

    // get_provider_with_secret on missing id returns None
    assert!(db
        .get_provider_with_secret("nonexistent")
        .unwrap()
        .is_none());
}

#[test]
fn row_to_provider_defaults_on_invalid_provider_kind() {
    // Covers the row_to_provider fallback: when the provider_kind column
    // holds an unparseable JSON string, the row mapper logs a warning and
    // defaults to OpenAiCompatible instead of panicking.
    let db = Database::open_in_memory().unwrap();
    db.conn()
        .execute(
            "INSERT INTO providers (id, name, provider_kind, base_url, enabled, \
             api_key, created_at_ms, updated_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (
                "p-bad",
                "Bad Kind Provider",
                "not-valid-json",
                "https://api.test.com",
                1i64,
                "sk-test",
                1000i64,
                1000i64,
            ),
        )
        .unwrap();
    let provider = db
        .get_provider_with_secret("p-bad")
        .unwrap()
        .expect("provider should exist");
    assert_eq!(provider.provider_kind, ProviderKind::OpenAiCompatible);
}

#[test]
fn set_model_tags_errors_on_nonexistent_model() {
    let db = Database::open_in_memory().unwrap();
    // No model with this id exists — set_model_tags must reject, not
    // silently succeed (especially when the tag diff is empty).
    let err = db.set_model_tags("nonexistent-id", &[]).unwrap_err();
    assert!(err.to_string().contains("model not found"));
}

#[test]
fn create_model_dedupes_duplicate_tags() {
    let db = Database::open_in_memory().unwrap();
    db.create_provider(CreateProviderReq {
        name: "P".into(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://api.test.com".into(),
        enabled: true,
        api_key: Some("sk-test".into()),
    })
    .unwrap();
    let provider = db.list_providers().unwrap().pop().unwrap();
    // Duplicate tags in the input — store must dedup, not hit UNIQUE.
    let model = db
        .create_model(CreateModelReq {
            provider_id: provider.id.clone(),
            model_id: "m-1".into(),
            enabled: true,
            tags: vec!["chat".into(), "chat".into(), "fast".into()],
            display_name: None,
            reasoning: None,
            context_window: None,
            max_tokens: None,
        })
        .unwrap();
    let entries = db
        .list_models_filtered(ModelCatalogFilter {
            provider_id: None,
            tags: vec![],
            include_disabled: true,
        })
        .unwrap();
    let entry = entries.iter().find(|e| e.model_db_id == model.id).unwrap();
    assert_eq!(entry.tags, vec!["chat".to_string(), "fast".to_string()]);
}

#[test]
fn model_metadata_round_trip() {
    let db = Database::open_in_memory().unwrap();
    let provider = db
        .create_provider(CreateProviderReq {
            name: "test".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: true,
            api_key: Some("sk-test".into()),
        })
        .unwrap();
    let m = db
        .create_model(CreateModelReq {
            provider_id: provider.id.clone(),
            model_id: "claude-sonnet-4-5".into(),
            enabled: true,
            tags: vec![],
            display_name: Some("Claude Sonnet 4.5".into()),
            reasoning: Some(true),
            context_window: Some(200000),
            max_tokens: Some(8192),
        })
        .unwrap();
    assert_eq!(m.display_name.as_deref(), Some("Claude Sonnet 4.5"));
    assert!(m.reasoning);
    assert_eq!(m.context_window, Some(200000));
    assert_eq!(m.max_tokens, Some(8192));
    let fetched = db.get_model_by_id(&m.id).unwrap().unwrap();
    assert_eq!(fetched.display_name, m.display_name);
    assert_eq!(fetched.context_window, m.context_window);

    // update_model can patch metadata fields
    let patched = db
        .update_model(
            &m.id,
            UpdateModelPatch {
                enabled: None,
                display_name: Some("Claude Sonnet 4.5 (renamed)".into()),
                reasoning: Some(false),
                context_window: Some(180000),
                max_tokens: Some(4096),
            },
        )
        .unwrap();
    assert_eq!(
        patched.display_name.as_deref(),
        Some("Claude Sonnet 4.5 (renamed)")
    );
    assert!(!patched.reasoning);
    assert_eq!(patched.context_window, Some(180000));
    assert_eq!(patched.max_tokens, Some(4096));

    // list_models_filtered surfaces metadata in the joined row
    let entries = db
        .list_models_filtered(ModelCatalogFilter {
            provider_id: Some(provider.id.clone()),
            tags: vec![],
            include_disabled: true,
        })
        .unwrap();
    let entry = entries.iter().find(|e| e.model_db_id == m.id).unwrap();
    assert_eq!(
        entry.display_name.as_deref(),
        Some("Claude Sonnet 4.5 (renamed)")
    );
    assert!(!entry.reasoning);
    assert_eq!(entry.context_window, Some(180000));
    assert_eq!(entry.max_tokens, Some(4096));
}

#[test]
fn update_provider_persists_provider_kind_patch() {
    let db = Database::open_in_memory().unwrap();
    let provider = db
        .create_provider(CreateProviderReq {
            name: "P1".into(),
            provider_kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.test.com".into(),
            enabled: true,
            api_key: Some("sk-test".into()),
        })
        .unwrap();
    let updated = db
        .update_provider(
            &provider.id,
            UpdateProviderPatch {
                name: None,
                base_url: None,
                enabled: None,
                provider_kind: Some(ProviderKind::AnthropicCompatible),
                api_key: None,
            },
        )
        .unwrap();
    assert_eq!(updated.provider_kind, ProviderKind::AnthropicCompatible);
    // Verify round-trip via a fresh read.
    let fetched = db.get_provider_with_secret(&provider.id).unwrap().unwrap();
    assert_eq!(fetched.provider_kind, ProviderKind::AnthropicCompatible);
}
