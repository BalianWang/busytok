#![allow(unused_imports)]
use busytok_domain::{ModelCatalogFilter, ProfileModelRef, ProviderKind};
use busytok_store::{
    CreateModelReq, CreateProviderReq, ModelCatalogEntry, Provider, ProviderSummary,
    UpdateModelPatch, UpdateProviderPatch,
};
use busytok_store::Database;

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
        .update_provider(&created.id, UpdateProviderPatch {
            name: Some("Updated".to_string()),
            base_url: None,
            enabled: None,
            api_key: Some(Some("sk-new-key".to_string())),
        })
        .unwrap();
    assert_eq!(updated.name, "Updated");

    let with_secret = db.get_provider_with_secret(&created.id).unwrap().unwrap();
    assert_eq!(with_secret.api_key.as_deref(), Some("sk-new-key"));

    db.delete_provider(&created.id, &[]).unwrap();
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
        })
        .unwrap();
    assert_eq!(model.model_id, "gpt-4o");

    // Duplicate (provider_id, model_id) rejected
    let dup = db.create_model(CreateModelReq {
        provider_id: provider.id.clone(),
        model_id: "gpt-4o".to_string(),
        enabled: true,
        tags: vec![],
    });
    assert!(dup.is_err());

    // List tags
    let tags = db.list_tags().unwrap();
    assert!(tags.contains(&"fast".to_string()));
    assert!(tags.contains(&"cheap".to_string()));

    // Delete model cascades tags
    db.delete_model(&model.id, &[]).unwrap();
    let entries = db.list_models_filtered(ModelCatalogFilter::default()).unwrap();
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
    }).unwrap();
    db.create_model(CreateModelReq {
        provider_id: provider.id.clone(),
        model_id: "gpt-4o-mini".into(),
        enabled: true,
        tags: vec!["fast".into()],
    }).unwrap();

    // AND semantics: only model with both tags
    let entries = db.list_models_filtered(ModelCatalogFilter {
        provider_id: None,
        tags: vec!["fast".into(), "cheap".into()],
        include_disabled: false,
    }).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].model_id, "gpt-4o");
}

#[test]
fn include_disabled_filters_both_provider_and_model() {
    let db = Database::open_in_memory().unwrap();
    let p_enabled = db.create_provider(CreateProviderReq {
        name: "Enabled".into(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://a.com".into(),
        enabled: true,
        api_key: Some("k".into()),
    }).unwrap();
    let p_disabled = db.create_provider(CreateProviderReq {
        name: "Disabled".into(),
        provider_kind: ProviderKind::OpenAiCompatible,
        base_url: "https://b.com".into(),
        enabled: false,
        api_key: None,
    }).unwrap();

    db.create_model(CreateModelReq {
        provider_id: p_enabled.id.clone(), model_id: "m-enabled".into(),
        enabled: true, tags: vec![],
    }).unwrap();
    db.create_model(CreateModelReq {
        provider_id: p_enabled.id.clone(), model_id: "m-disabled".into(),
        enabled: false, tags: vec![],
    }).unwrap();
    db.create_model(CreateModelReq {
        provider_id: p_disabled.id.clone(), model_id: "m-under-disabled".into(),
        enabled: true, tags: vec![],
    }).unwrap();

    // include_disabled=false: only enabled provider + enabled model
    let entries = db.list_models_filtered(ModelCatalogFilter {
        provider_id: None, tags: vec![], include_disabled: false,
    }).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].model_id, "m-enabled");

    // include_disabled=true: all 3
    let entries = db.list_models_filtered(ModelCatalogFilter {
        provider_id: None, tags: vec![], include_disabled: true,
    }).unwrap();
    assert_eq!(entries.len(), 3);
}

#[test]
fn provider_delete_blocked_by_profile_reference() {
    let db = Database::open_in_memory().unwrap();
    let provider = db.create_provider(sample_provider_req()).unwrap();
    let pid = provider.id.clone();
    let model = db.create_model(CreateModelReq {
        provider_id: pid.clone(), model_id: "gpt-4o".into(),
        enabled: true, tags: vec![],
    }).unwrap();

    let refs = vec![ProfileModelRef {
        provider_id: pid.clone(),
        model_id: "gpt-4o".into(),
    }];

    // Blocked
    let err = db.delete_provider(&pid, &refs);
    assert!(err.is_err());

    // Not blocked when refs empty
    db.delete_provider(&pid, &[]).unwrap();
    let _ = model; // suppress unused
}

#[test]
fn model_delete_blocked_by_profile_reference() {
    let db = Database::open_in_memory().unwrap();
    let provider = db.create_provider(sample_provider_req()).unwrap();
    let pid = provider.id.clone();
    let model = db.create_model(CreateModelReq {
        provider_id: pid.clone(), model_id: "gpt-4o".into(),
        enabled: true, tags: vec![],
    }).unwrap();

    let refs = vec![ProfileModelRef {
        provider_id: pid.clone(),
        model_id: "gpt-4o".into(),
    }];

    let err = db.delete_model(&model.id, &refs);
    assert!(err.is_err());

    db.delete_model(&model.id, &[]).unwrap();
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

    let model = db.create_model(CreateModelReq {
        provider_id: pid.clone(),
        model_id: "gpt-4o".into(),
        enabled: true,
        tags: vec!["fast".into()],
    }).unwrap();

    // get_model_by_provider_and_model_id finds it
    let found = db
        .get_model_by_provider_and_model_id(&pid, "gpt-4o")
        .unwrap()
        .expect("model should be found by (provider_id, model_id)");
    assert_eq!(found.id, model.id);

    // update_model: disable
    let disabled = db
        .update_model(&model.id, UpdateModelPatch { enabled: Some(false) })
        .unwrap();
    assert!(!disabled.enabled);

    // update_model: re-enable
    let reenabled = db
        .update_model(&model.id, UpdateModelPatch { enabled: Some(true) })
        .unwrap();
    assert!(reenabled.enabled);

    // update_model: no-op patch (enabled=None) still returns the model
    let noop = db
        .update_model(&model.id, UpdateModelPatch { enabled: None })
        .unwrap();
    assert!(noop.enabled);

    // update_model on missing id errors
    let err = db.update_model("nonexistent", UpdateModelPatch { enabled: Some(true) });
    assert!(err.is_err());

    // update_model no-op on missing id also errors (falls through to get_model_by_id)
    let err = db.update_model("nonexistent", UpdateModelPatch { enabled: None });
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
            api_key: None,
        },
    );
    assert!(err.is_err());

    // delete_provider on missing id errors
    let err = db.delete_provider("nonexistent", &[]);
    assert!(err.is_err());

    // get_model_by_id on missing id returns None
    assert!(db.get_model_by_id("nonexistent").unwrap().is_none());

    // get_model_by_provider_and_model_id on missing returns None
    assert!(db
        .get_model_by_provider_and_model_id(&pid, "no-such-model")
        .unwrap()
        .is_none());

    // get_provider_with_secret on missing id returns None
    assert!(db.get_provider_with_secret("nonexistent").unwrap().is_none());
}
