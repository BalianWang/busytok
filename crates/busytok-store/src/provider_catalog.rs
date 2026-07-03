//! SQL repository for provider / model / model_tags catalog.
use anyhow::{anyhow, bail, Context, Result};
// `pub use` so lib.rs can re-export `ModelCatalogEntry` / `ModelCatalogFilter`
// (and the rest of the catalog types) from `busytok_store::provider_catalog`.
pub use busytok_domain::{
    Model, ModelCatalogEntry, ModelCatalogFilter, ModelTag, ProfileModelRef, Provider,
    ProviderKind, ProviderSummary,
};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use tracing::info;

// ── Input DTOs (no id/timestamps — store generates those) ──────────────

pub struct CreateProviderReq {
    pub name: String,
    pub provider_kind: ProviderKind,
    pub base_url: String,
    pub enabled: bool,
    pub api_key: Option<String>,
}

pub struct UpdateProviderPatch {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub enabled: Option<bool>,
    // None=不改, Some(None)=清除, Some(Some(k))=更新
    pub api_key: Option<Option<String>>,
}

pub struct CreateModelReq {
    pub provider_id: String,
    pub model_id: String,
    pub enabled: bool,
    pub tags: Vec<String>,
}

// UpdateModelPatch only has enabled — model_id is immutable
pub struct UpdateModelPatch {
    pub enabled: Option<bool>,
}

// ── CRUD: providers ────────────────────────────────────────────────────

pub fn create_provider(conn: &Connection, req: CreateProviderReq) -> Result<Provider> {
    let now = busytok_domain::now_ms();
    // id 由 store 层生成（UUID v4），不由用户提供。冲突概率极低，
    // 万一发生则直接返回错误（不重试以避免无限循环）。
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO providers (id, name, provider_kind, base_url, enabled, api_key, created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![
            id,
            req.name,
            serde_json::to_string(&req.provider_kind)?,
            req.base_url,
            req.enabled as i64,
            req.api_key,
            now,
        ],
    )
    .map_err(|e| {
        if e.to_string().contains("PRIMARY KEY") {
            // UUID v4 冲突概率极低，直接返回错误
            anyhow!("provider id collision, please retry: {}", id)
        } else {
            anyhow!(e)
        }
    })?;
    info!(event_code = "provider.created", provider_id = %id, "provider created");
    get_provider_with_secret(conn, &id)?
        .ok_or_else(|| anyhow!("provider {} not found after insert", id))
}

pub fn update_provider(conn: &Connection, id: &str, patch: UpdateProviderPatch) -> Result<Provider> {
    // Verify provider exists first
    let exists: bool = conn.query_row(
        "SELECT 1 FROM providers WHERE id = ?1", params![id],
        |_| Ok(true)
    ).optional()?.is_some();
    if !exists {
        bail!("provider not found: {}", id);
    }
    let now = busytok_domain::now_ms();
    let tx = conn.unchecked_transaction()?;
    if let Some(name) = &patch.name {
        tx.execute("UPDATE providers SET name = ?1, updated_at_ms = ?2 WHERE id = ?3", params![name, now, id])?;
    }
    if let Some(base_url) = &patch.base_url {
        tx.execute("UPDATE providers SET base_url = ?1, updated_at_ms = ?2 WHERE id = ?3", params![base_url, now, id])?;
    }
    if let Some(enabled) = patch.enabled {
        tx.execute("UPDATE providers SET enabled = ?1, updated_at_ms = ?2 WHERE id = ?3", params![enabled as i64, now, id])?;
    }
    match &patch.api_key {
        Some(None) => {
            tx.execute("UPDATE providers SET api_key = NULL, updated_at_ms = ?1 WHERE id = ?2", params![now, id])?;
        }
        Some(Some(api_key)) => {
            tx.execute("UPDATE providers SET api_key = ?1, updated_at_ms = ?2 WHERE id = ?3", params![api_key, now, id])?;
        }
        None => {}
    }
    tx.commit()?;
    info!(event_code = "provider.updated", provider_id = %id, "provider updated");
    get_provider_with_secret(conn, id)?.ok_or_else(|| anyhow!("provider {} not found after update", id))
}

pub fn delete_provider(conn: &Connection, id: &str, profile_refs: &[ProfileModelRef]) -> Result<()> {
    if provider_has_profile_references(id, profile_refs) {
        let count = profile_refs.iter().filter(|r| r.provider_id == id).count();
        bail!("cannot delete provider: {} profile(s) still reference it", count);
    }
    let rows = conn.execute("DELETE FROM providers WHERE id = ?1", params![id])?;
    if rows == 0 {
        bail!("provider not found: {}", id);
    }
    info!(event_code = "provider.deleted", provider_id = %id, "provider deleted");
    Ok(())
}

pub fn get_provider_with_secret(conn: &Connection, id: &str) -> Result<Option<Provider>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, provider_kind, base_url, enabled, api_key, created_at_ms, updated_at_ms
         FROM providers WHERE id = ?1",
    )?;
    let row = stmt.query_row(params![id], row_to_provider).optional()?;
    Ok(row)
}

pub fn list_providers(conn: &Connection) -> Result<Vec<ProviderSummary>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, provider_kind, base_url, enabled, api_key, created_at_ms, updated_at_ms
         FROM providers ORDER BY name",
    )?;
    let providers: Vec<Provider> = stmt.query_map([], row_to_provider)?.filter_map(|r| r.ok()).collect();
    Ok(providers.iter().map(ProviderSummary::from).collect())
}

// ── CRUD: models ───────────────────────────────────────────────────────

pub fn create_model(conn: &Connection, req: CreateModelReq) -> Result<Model> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = busytok_domain::now_ms();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO models (id, provider_id, model_id, enabled, created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![id, req.provider_id, req.model_id, req.enabled as i64, now],
    )
    .map_err(|e| {
        if e.to_string().contains("UNIQUE") {
            anyhow!("model '{}' already exists for provider '{}'", req.model_id, req.provider_id)
        } else {
            anyhow!(e)
        }
    })?;
    for tag in &req.tags {
        tx.execute(
            "INSERT INTO model_tags (model_id, tag) VALUES (?1, ?2)",
            params![id, tag],
        )?;
    }
    tx.commit()?;
    info!(event_code = "model.created", model_id = %req.model_id, provider_id = %req.provider_id, "model created");
    get_model_by_id(conn, &id)?.ok_or_else(|| anyhow!("model {} not found after insert", id))
}

pub fn update_model(conn: &Connection, id: &str, patch: UpdateModelPatch) -> Result<Model> {
    if let Some(enabled) = patch.enabled {
        let now = busytok_domain::now_ms();
        let rows = conn.execute(
            "UPDATE models SET enabled = ?1, updated_at_ms = ?2 WHERE id = ?3",
            params![enabled as i64, now, id],
        )?;
        if rows == 0 {
            bail!("model not found: {}", id);
        }
        info!(event_code = "model.updated", model_db_id = %id, enabled, "model updated");
    }
    get_model_by_id(conn, id)?.ok_or_else(|| anyhow!("model {} not found after update", id))
}

pub fn delete_model(conn: &Connection, id: &str, profile_refs: &[ProfileModelRef]) -> Result<()> {
    let model = get_model_by_id(conn, id)?
        .ok_or_else(|| anyhow!("model not found: {}", id))?;
    if model_has_profile_references(&model.provider_id, &model.model_id, profile_refs) {
        bail!("cannot delete model: profile(s) still reference it");
    }
    let rows = conn.execute("DELETE FROM models WHERE id = ?1", params![id])?;
    if rows == 0 {
        bail!("model not found: {}", id);
    }
    info!(event_code = "model.deleted", model_db_id = %id, model_id = %model.model_id, "model deleted");
    Ok(())
}

pub fn get_model_by_id(conn: &Connection, id: &str) -> Result<Option<Model>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider_id, model_id, enabled, created_at_ms, updated_at_ms
         FROM models WHERE id = ?1",
    )?;
    Ok(stmt.query_row(params![id], row_to_model).optional()?)
}

pub fn get_model_by_provider_and_model_id(
    conn: &Connection,
    provider_id: &str,
    model_id: &str,
) -> Result<Option<Model>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider_id, model_id, enabled, created_at_ms, updated_at_ms
         FROM models WHERE provider_id = ?1 AND model_id = ?2",
    )?;
    Ok(stmt.query_row(params![provider_id, model_id], row_to_model).optional()?)
}

// ── Catalog queries ────────────────────────────────────────────────────

pub fn list_models_filtered(conn: &Connection, filter: ModelCatalogFilter) -> Result<Vec<ModelCatalogEntry>> {
    let include_disabled = if filter.include_disabled { 1 } else { 0 };
    let provider_id = filter.provider_id.as_deref();

    // Dynamic tag placeholders: rusqlite cannot bind Vec to IN()
    let tag_count = filter.tags.len() as i64;
    let tag_placeholders: Vec<String> = (0..filter.tags.len()).map(|_| "?".to_string()).collect();
    let tag_clause = if tag_placeholders.is_empty() {
        String::new()
    } else {
        format!(
            "HAVING (SELECT COUNT(DISTINCT tag) FROM model_tags WHERE model_id = m.id AND tag IN ({})) = {}",
            tag_placeholders.join(", "), tag_count
        )
    };

    let sql = format!(
        "SELECT p.id, p.name, p.provider_kind, p.enabled,
                m.id, m.model_id, m.enabled,
                COALESCE(GROUP_CONCAT(mt.tag, ','), '') AS tags_csv
         FROM models m
         JOIN providers p ON p.id = m.provider_id
         LEFT JOIN model_tags mt ON mt.model_id = m.id
         WHERE (?1 = 1 OR (p.enabled = 1 AND m.enabled = 1))
           AND (?2 IS NULL OR m.provider_id = ?2)
         GROUP BY m.id
         {tag_clause}
         ORDER BY p.name, m.model_id"
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![
        Box::new(include_disabled),
        Box::new(provider_id),
    ];
    for tag in &filter.tags {
        params_vec.push(Box::new(tag.clone()));
    }
    let rows = stmt.query_map(params_from_iter(params_vec.iter().map(|b| b.as_ref())), |row| {
        let tags_csv: String = row.get(7)?;
        let tags: Vec<String> = if tags_csv.is_empty() {
            vec![]
        } else {
            tags_csv.split(',').map(|s| s.to_string()).collect()
        };
        let kind_str: String = row.get(2)?;
        let provider_kind: ProviderKind = serde_json::from_str(&kind_str).unwrap_or(ProviderKind::OpenAiCompatible);
        Ok(ModelCatalogEntry {
            provider_id: row.get(0)?,
            provider_name: row.get(1)?,
            provider_kind,
            provider_enabled: row.get(3)?,
            model_db_id: row.get(4)?,
            model_id: row.get(5)?,
            model_enabled: row.get(6)?,
            tags,
        })
    })?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    info!(event_code = "model.catalog.listed", entry_count = entries.len(), "model catalog listed");
    Ok(entries)
}

pub fn list_models_by_provider(conn: &Connection, provider_id: &str) -> Result<Vec<ModelCatalogEntry>> {
    list_models_filtered(conn, ModelCatalogFilter {
        provider_id: Some(provider_id.to_string()),
        tags: vec![],
        include_disabled: true, // by-provider view shows all models
    })
}

pub fn list_tags(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT DISTINCT tag FROM model_tags ORDER BY tag")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut tags = Vec::new();
    for row in rows {
        tags.push(row?);
    }
    Ok(tags)
}

pub fn set_model_tags(conn: &Connection, model_id: &str, tags: &[String]) -> Result<()> {
    let now = busytok_domain::now_ms();
    let tx = conn.unchecked_transaction()?;
    // Diff: delete tags not in new set, insert new tags
    let existing: std::collections::HashSet<String> = tx
        .prepare("SELECT tag FROM model_tags WHERE model_id = ?1")?
        .query_map(params![model_id], |row| row.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    let new_set: std::collections::HashSet<String> = tags.iter().cloned().collect();
    // Compute diffs first
    let to_remove: Vec<_> = existing.difference(&new_set).cloned().collect();
    let to_add: Vec<_> = new_set.difference(&existing).cloned().collect();
    if to_add.is_empty() && to_remove.is_empty() {
        return Ok(()); // no changes, skip timestamp bump
    }
    // Remove tags that are no longer present
    for tag in &to_remove {
        tx.execute("DELETE FROM model_tags WHERE model_id = ?1 AND tag = ?2", params![model_id, tag])?;
        info!(event_code = "model.tag_removed", model_db_id = %model_id, tag = %tag, "tag removed");
    }
    // Insert new tags
    for tag in &to_add {
        tx.execute(
            "INSERT OR IGNORE INTO model_tags (model_id, tag) VALUES (?1, ?2)",
            params![model_id, tag],
        )?;
        info!(event_code = "model.tag_added", model_db_id = %model_id, tag = %tag, "tag added");
    }
    tx.execute("UPDATE models SET updated_at_ms = ?1 WHERE id = ?2", params![now, model_id])?;
    tx.commit()?;
    Ok(())
}

// ── Profile reference checks (blocking deletes) ────────────────────────

pub fn provider_has_profile_references(provider_id: &str, refs: &[ProfileModelRef]) -> bool {
    refs.iter().any(|r| r.provider_id == provider_id)
}

pub fn model_has_profile_references(provider_id: &str, model_id: &str, refs: &[ProfileModelRef]) -> bool {
    refs.iter().any(|r| r.provider_id == provider_id && r.model_id == model_id)
}

// ── Row mappers ────────────────────────────────────────────────────────

fn row_to_provider(row: &rusqlite::Row) -> rusqlite::Result<Provider> {
    let kind_str: String = row.get(2)?;
    let provider_kind: ProviderKind = serde_json::from_str(&kind_str)
        .unwrap_or_else(|e| {
            tracing::warn!(kind_str = %kind_str, error = %e, "failed to parse provider_kind, defaulting to OpenAiCompatible");
            ProviderKind::OpenAiCompatible
        });
    Ok(Provider {
        id: row.get(0)?,
        name: row.get(1)?,
        provider_kind,
        base_url: row.get(3)?,
        enabled: row.get::<_, i64>(4)? != 0,
        api_key: row.get(5)?,
        created_at_ms: row.get(6)?,
        updated_at_ms: row.get(7)?,
    })
}

fn row_to_model(row: &rusqlite::Row) -> rusqlite::Result<Model> {
    Ok(Model {
        id: row.get(0)?,
        provider_id: row.get(1)?,
        model_id: row.get(2)?,
        enabled: row.get::<_, i64>(3)? != 0,
        created_at_ms: row.get(4)?,
        updated_at_ms: row.get(5)?,
    })
}
