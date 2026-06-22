use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, bail, Context, Result};
use busytok_domain::now_ms;
use rusqlite::{params, params_from_iter, Connection, ErrorCode, OptionalExtension, Row};
use tracing::{debug, info};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptActionRow {
    Copy,
    Paste,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptSortRow {
    Smart,
    RecentlyUsed,
    MostUsed,
    RecentlyUpdated,
    Alphabetical,
    PinnedFirst,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptUseSurfaceRow {
    Overlay,
    Page,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptUseOutcomeRow {
    Copy,
    PasteAttempted,
    PasteFellBackToCopy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptUseFailureReasonRow {
    PermissionMissing,
    FocusLost,
    InjectionFailed,
    UnsupportedPlatform,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptEntryRow {
    pub id: String,
    pub content: String,
    content_normalized: String,
    pub alias: Option<String>,
    alias_normalized: Option<String>,
    pub tags: Vec<String>,
    tags_normalized: Vec<String>,
    pub is_pinned: bool,
    pub usage_count: i64,
    pub last_used_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct NewPromptEntryRow {
    pub content: String,
    pub alias: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct UpdatePromptEntryRow {
    pub id: String,
    pub content: String,
    pub alias: Option<String>,
    pub tags: Vec<String>,
    pub is_pinned: bool,
}

#[derive(Debug, Clone)]
pub struct PromptListQuery {
    pub query: Option<String>,
    pub tag: Option<String>,
    pub sort: PromptSortRow,
    pub limit: i64,
}

#[derive(Debug, Clone)]
pub struct PromptListResult {
    pub entries: Vec<PromptEntryRow>,
    pub total_count: i64,
}

#[derive(Debug, Clone)]
pub struct PromptUseRow {
    pub prompt_entry_id: String,
    pub action: PromptActionRow,
    pub surface: PromptUseSurfaceRow,
    pub outcome: PromptUseOutcomeRow,
    pub failure_reason: Option<PromptUseFailureReasonRow>,
}

#[derive(Debug, Clone)]
pub struct PromptUseResultRow {
    pub usage_count: i64,
    pub last_used_at_ms: Option<i64>,
}

pub fn normalize_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

const ALIAS_MAX: usize = 80;
const CONTENT_MAX: usize = 65_536;
const MAX_LIMIT: i64 = 500;

#[derive(Debug, Clone)]
struct NormalizedTag {
    tag: String,
    tag_normalized: String,
}

fn validate_content(content: &str) -> Result<String> {
    if content.trim().is_empty() {
        bail!("prompt content must not be empty");
    }
    if content.chars().count() > CONTENT_MAX {
        bail!("prompt content must be at most {CONTENT_MAX} characters");
    }
    Ok(content.to_string())
}

fn validate_alias(alias: Option<String>) -> Result<Option<(String, String)>> {
    let Some(alias) = alias else {
        return Ok(None);
    };
    let alias = alias.trim();
    if alias.is_empty() {
        return Ok(None);
    }
    if alias.chars().any(is_forbidden_alias_char) {
        bail!("alias must not contain whitespace, quotes, or backticks");
    }
    if alias.chars().count() > ALIAS_MAX {
        bail!("alias must be at most {ALIAS_MAX} characters");
    }
    let alias_normalized = normalize_text(&alias);
    if alias_normalized.is_empty() {
        return Ok(None);
    }
    Ok(Some((alias.to_string(), alias_normalized)))
}

fn normalize_tags(tags: Vec<String>) -> Vec<NormalizedTag> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();

    for tag in tags {
        let tag = tag.split_whitespace().collect::<Vec<_>>().join(" ");
        if tag.is_empty() {
            continue;
        }
        let tag_normalized = normalize_text(&tag);
        if seen.insert(tag_normalized.clone()) {
            normalized.push(NormalizedTag {
                tag,
                tag_normalized,
            });
        }
    }

    normalized
}

fn is_forbidden_alias_char(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '"' | '\'' | '`' | '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}' | '\u{feff}'
        )
}

fn bounded_limit(limit: i64) -> i64 {
    limit.clamp(1, MAX_LIMIT)
}

pub fn create_prompt_entry(conn: &Connection, row: NewPromptEntryRow) -> Result<PromptEntryRow> {
    let id = format!("prompt_{}", uuid::Uuid::new_v4());
    let content = validate_content(&row.content)?;
    let content_normalized = normalize_text(&content);
    let alias = validate_alias(row.alias)?;
    let tags = normalize_tags(row.tags);
    let now = now_ms();

    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO prompt_entries (
            id, content, content_normalized, alias, alias_normalized,
            is_pinned, usage_count, last_used_at_ms, created_at_ms, updated_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, 0, 0, NULL, ?6, ?6)",
        params![
            id,
            content,
            content_normalized,
            alias.as_ref().map(|(value, _)| value.as_str()),
            alias.as_ref().map(|(_, normalized)| normalized.as_str()),
            now,
        ],
    )
    .map_err(map_alias_constraint_error)?;

    for tag in &tags {
        tx.execute(
            "INSERT INTO prompt_entry_tags (prompt_entry_id, tag, tag_normalized) VALUES (?1, ?2, ?3)",
            params![id, tag.tag, tag.tag_normalized],
        )?;
    }
    tx.commit()?;

    info!(
        operation = "prompts.create",
        prompt_entry_id = %id,
        "created prompt entry"
    );

    let entry = get_prompt_entry(conn, &id)?
        .ok_or_else(|| anyhow!("prompt entry {id} not found after insert"))?;
    Ok(entry)
}

pub fn update_prompt_entry(conn: &Connection, row: UpdatePromptEntryRow) -> Result<PromptEntryRow> {
    let content = validate_content(&row.content)?;
    let content_normalized = normalize_text(&content);
    let alias = validate_alias(row.alias)?;
    let tags = normalize_tags(row.tags);
    let now = now_ms();

    let tx = conn.unchecked_transaction()?;
    let affected = tx
        .execute(
            "UPDATE prompt_entries SET
            content = ?2,
            content_normalized = ?3,
            alias = ?4,
            alias_normalized = ?5,
            is_pinned = ?6,
            updated_at_ms = ?7
        WHERE id = ?1",
            params![
                row.id,
                content,
                content_normalized,
                alias.as_ref().map(|(value, _)| value.as_str()),
                alias.as_ref().map(|(_, normalized)| normalized.as_str()),
                row.is_pinned as i64,
                now,
            ],
        )
        .map_err(map_alias_constraint_error)?;

    if affected == 0 {
        bail!("prompt entry {} not found", row.id);
    }

    tx.execute(
        "DELETE FROM prompt_entry_tags WHERE prompt_entry_id = ?1",
        params![row.id],
    )?;
    for tag in &tags {
        tx.execute(
            "INSERT INTO prompt_entry_tags (prompt_entry_id, tag, tag_normalized) VALUES (?1, ?2, ?3)",
            params![row.id, tag.tag, tag.tag_normalized],
        )?;
    }
    tx.commit()?;

    info!(
        operation = "prompts.update",
        prompt_entry_id = %row.id,
        "updated prompt entry"
    );

    let entry = get_prompt_entry(conn, &row.id)?
        .ok_or_else(|| anyhow!("prompt entry {} not found after update", row.id))?;
    Ok(entry)
}

pub fn get_prompt_entry(conn: &Connection, id: &str) -> Result<Option<PromptEntryRow>> {
    let sql = PROMPT_ENTRY_SELECT_SQL.to_string() + " WHERE p.id = ?1";
    let mut stmt = conn
        .prepare(&sql)
        .context("failed to prepare prompt entry query")?;
    let mut entry = stmt
        .query_row(params![id], row_to_prompt_entry)
        .optional()
        .context("failed to query prompt entry")?;

    if let Some(entry) = entry.as_mut() {
        attach_tags(conn, std::slice::from_mut(entry))?;
    }

    Ok(entry)
}

pub fn delete_prompt_entry(conn: &Connection, id: &str) -> Result<bool> {
    let tx = conn
        .unchecked_transaction()
        .context("failed to start prompt delete transaction")?;
    let changed = tx
        .execute("DELETE FROM prompt_entries WHERE id = ?1", params![id])
        .context("failed to delete prompt entry")?;
    tx.commit()
        .context("failed to commit prompt delete transaction")?;

    let deleted = changed > 0;
    info!(
        operation = "prompts.delete",
        prompt_entry_id = %id,
        deleted,
        "deleted prompt entry"
    );

    Ok(deleted)
}

pub fn list_prompt_entries(conn: &Connection, query: PromptListQuery) -> Result<PromptListResult> {
    let limit = bounded_limit(query.limit);
    let query_text = query
        .query
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let query_normalized = query_text.as_deref().map(normalize_text);
    let tag_text = query
        .tag
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let tag_normalized = tag_text.as_deref().map(normalize_text);

    let mut entries =
        load_candidate_entries(conn, tag_normalized.as_deref(), query_normalized.as_deref())?;

    let total_count = entries.len() as i64;
    sort_entries(&mut entries, &query.sort, query_normalized.as_deref());
    entries.truncate(limit as usize);

    debug!(
        has_query = query_text.is_some(),
        query_len = query_text.as_ref().map(|value| value.chars().count()).unwrap_or(0),
        has_tag = tag_text.is_some(),
        tag_len = tag_text.as_ref().map(|value| value.chars().count()).unwrap_or(0),
        sort = ?query.sort,
        limit,
        total_count,
        hit_count = entries.len(),
        "ranked prompt entries"
    );

    Ok(PromptListResult {
        entries,
        total_count,
    })
}

pub fn suggest_tags(conn: &Connection, prefix: &str, limit: i64) -> Result<Vec<String>> {
    let limit = limit.clamp(1, 50);
    let prefix = prefix.trim();
    let prefix_normalized = normalize_text(prefix);
    let like_pattern = if prefix_normalized.is_empty() {
        "%".to_string()
    } else {
        let mut pattern = String::with_capacity(prefix_normalized.len() + 2);
        for ch in prefix_normalized.chars() {
            if matches!(ch, '%' | '_' | '\\') {
                pattern.push('\\');
            }
            pattern.push(ch);
        }
        pattern.push('%');
        pattern
    };

    let sql = "\
        SELECT MIN(tag) AS display_tag \
        FROM prompt_entry_tags \
        WHERE tag_normalized LIKE ? ESCAPE '\\' \
        GROUP BY tag_normalized \
        ORDER BY display_tag \
        LIMIT ?";

    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![like_pattern, limit], |row| row.get::<_, String>(0))?;
    let mut tags = Vec::new();
    for row in rows {
        tags.push(row?);
    }

    debug!(
        prefix = %prefix,
        prefix_normalized = %prefix_normalized,
        limit,
        match_count = tags.len(),
        "suggested tags"
    );

    Ok(tags)
}

pub fn record_prompt_use(conn: &Connection, row: PromptUseRow) -> Result<PromptUseResultRow> {
    let use_id = format!("prompt_use_{}", uuid::Uuid::new_v4());
    let now = now_ms();
    let action = prompt_action_as_str(&row.action);
    let surface = prompt_use_surface_as_str(&row.surface);
    let outcome = prompt_use_outcome_as_str(&row.outcome);
    let failure_reason = row
        .failure_reason
        .as_ref()
        .map(prompt_use_failure_reason_as_str);

    let tx = conn
        .unchecked_transaction()
        .context("failed to start prompt use transaction")?;
    let exists = tx
        .query_row(
            "SELECT 1 FROM prompt_entries WHERE id = ?1",
            params![row.prompt_entry_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .context("failed to check prompt entry existence")?
        .is_some();
    if !exists {
        bail!("prompt entry not found: {}", row.prompt_entry_id);
    }

    tx.execute(
        "INSERT INTO prompt_entry_uses (\
            id, prompt_entry_id, action, surface, outcome, failure_reason, used_at_ms, created_at_ms\
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![
            use_id,
            row.prompt_entry_id,
            action,
            surface,
            outcome,
            failure_reason,
            now,
        ],
    )
    .context("failed to insert prompt use")?;

    if row.outcome == PromptUseOutcomeRow::PasteAttempted {
        tx.execute(
            "UPDATE prompt_entries SET \
                usage_count = usage_count + 1, \
                last_used_at_ms = ?2, \
                updated_at_ms = ?2 \
             WHERE id = ?1",
            params![row.prompt_entry_id, now],
        )
        .context("failed to update prompt usage counters")?;
    }

    let result = tx
        .query_row(
            "SELECT usage_count, last_used_at_ms FROM prompt_entries WHERE id = ?1",
            params![row.prompt_entry_id],
            |row| {
                Ok(PromptUseResultRow {
                    usage_count: row.get(0)?,
                    last_used_at_ms: row.get(1)?,
                })
            },
        )
        .context("failed to query prompt usage counters")?;
    tx.commit()
        .context("failed to commit prompt use transaction")?;

    info!(
        operation = "prompts.use",
        prompt_entry_id = %row.prompt_entry_id,
        action,
        surface,
        outcome,
        failure_reason,
        usage_count = result.usage_count,
        "recorded prompt use"
    );

    Ok(result)
}

const PROMPT_ENTRY_SELECT_SQL: &str = "\
    SELECT \
        p.id, p.content, p.content_normalized, \
        p.alias, p.alias_normalized, \
        p.is_pinned, p.usage_count, p.last_used_at_ms, \
        p.created_at_ms, p.updated_at_ms \
    FROM prompt_entries p";

fn load_candidate_entries(
    conn: &Connection,
    tag_normalized: Option<&str>,
    query_normalized: Option<&str>,
) -> Result<Vec<PromptEntryRow>> {
    if let Some(query_normalized) = query_normalized {
        let entries = load_query_candidate_entries(conn, tag_normalized, query_normalized)?;
        if !entries.is_empty() {
            return Ok(entries);
        }
        return load_fallback_candidate_entries(conn, tag_normalized, query_normalized);
    }

    let mut sql = PROMPT_ENTRY_SELECT_SQL.to_string();
    let mut query_params = Vec::new();

    if let Some(tag_normalized) = tag_normalized {
        sql.push_str(
            " WHERE EXISTS (\
                SELECT 1 FROM prompt_entry_tags ft \
                WHERE ft.prompt_entry_id = p.id AND ft.tag_normalized = ?\
            )",
        );
        query_params.push(tag_normalized.to_string());
    }

    load_entries(conn, &sql, query_params)
}

fn load_query_candidate_entries(
    conn: &Connection,
    tag_normalized: Option<&str>,
    query_normalized: &str,
) -> Result<Vec<PromptEntryRow>> {
    let prefix_upper_bound = prefix_upper_bound(query_normalized);
    let mut sql = format!(
        "{PROMPT_ENTRY_SELECT_SQL} \
         WHERE p.id IN (\
            SELECT id FROM prompt_entries WHERE alias_normalized = ? \
            UNION \
            SELECT id FROM prompt_entries \
                WHERE alias_normalized >= ? AND alias_normalized < ? \
         )"
    );
    let mut query_params = vec![
        query_normalized.to_string(),
        query_normalized.to_string(),
        prefix_upper_bound,
    ];

    if let Some(tag_normalized) = tag_normalized {
        sql.push_str(
            " AND EXISTS (\
                SELECT 1 FROM prompt_entry_tags ft \
                WHERE ft.prompt_entry_id = p.id AND ft.tag_normalized = ?\
            )",
        );
        query_params.push(tag_normalized.to_string());
    }

    load_entries(conn, &sql, query_params)
}

fn load_fallback_candidate_entries(
    conn: &Connection,
    tag_normalized: Option<&str>,
    query_normalized: &str,
) -> Result<Vec<PromptEntryRow>> {
    let contains_pattern = contains_like_pattern(query_normalized);
    let mut sql = format!(
        "{PROMPT_ENTRY_SELECT_SQL} \
         WHERE p.id IN (\
            SELECT id FROM prompt_entries \
                WHERE content_normalized LIKE ? ESCAPE '\\' \
            UNION \
            SELECT prompt_entry_id FROM prompt_entry_tags \
                WHERE tag_normalized LIKE ? ESCAPE '\\'\
         )"
    );
    let mut query_params = vec![contains_pattern.clone(), contains_pattern];

    if let Some(tag_normalized) = tag_normalized {
        sql.push_str(
            " AND EXISTS (\
                SELECT 1 FROM prompt_entry_tags ft \
                WHERE ft.prompt_entry_id = p.id AND ft.tag_normalized = ?\
            )",
        );
        query_params.push(tag_normalized.to_string());
    }

    load_entries(conn, &sql, query_params)
}

fn load_entries(
    conn: &Connection,
    sql: &str,
    query_params: Vec<String>,
) -> Result<Vec<PromptEntryRow>> {
    let mut stmt = conn
        .prepare(sql)
        .context("failed to prepare prompt entries query")?;
    let rows = stmt.query_map(params_from_iter(query_params.iter()), row_to_prompt_entry)?;

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    drop(stmt);
    attach_tags(conn, &mut entries)?;

    Ok(entries)
}

fn attach_tags(conn: &Connection, entries: &mut [PromptEntryRow]) -> Result<()> {
    let prompt_entry_ids: Vec<_> = entries.iter().map(|entry| entry.id.as_str()).collect();
    let tags_by_id = load_tags_for_entry_ids(conn, &prompt_entry_ids)?;

    for entry in entries {
        if let Some(tags) = tags_by_id.get(&entry.id) {
            entry.tags = tags.display.clone();
            entry.tags_normalized = tags.normalized.clone();
        } else {
            entry.tags.clear();
            entry.tags_normalized.clear();
        }
    }

    Ok(())
}

fn prefix_upper_bound(prefix: &str) -> String {
    format!("{prefix}\u{10ffff}")
}

fn contains_like_pattern(value: &str) -> String {
    let mut pattern = String::with_capacity(value.len() + 2);
    pattern.push('%');
    for ch in value.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

fn cmp_alias_nulls_last(a: &Option<String>, b: &Option<String>) -> std::cmp::Ordering {
    match (a, b) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(a), Some(b)) => a.cmp(b),
    }
}

fn sort_entries(
    entries: &mut [PromptEntryRow],
    sort: &PromptSortRow,
    query_normalized: Option<&str>,
) {
    match sort {
        PromptSortRow::Smart => {
            entries.sort_by_key(|entry| smart_rank(entry, query_normalized));
        }
        PromptSortRow::PinnedFirst => entries.sort_by(|a, b| {
            b.is_pinned
                .cmp(&a.is_pinned)
                .then_with(|| {
                    b.last_used_at_ms
                        .unwrap_or(i64::MIN)
                        .cmp(&a.last_used_at_ms.unwrap_or(i64::MIN))
                })
                .then_with(|| a.alias_normalized.cmp(&b.alias_normalized))
                .then_with(|| a.id.cmp(&b.id))
        }),
        PromptSortRow::Alphabetical => {
            entries.sort_by(|a, b| {
                cmp_alias_nulls_last(&a.alias_normalized, &b.alias_normalized)
                    .then_with(|| a.content_normalized.cmp(&b.content_normalized))
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
        PromptSortRow::RecentlyUsed => entries.sort_by(|a, b| {
            b.last_used_at_ms
                .unwrap_or(i64::MIN)
                .cmp(&a.last_used_at_ms.unwrap_or(i64::MIN))
                .then_with(|| a.alias_normalized.cmp(&b.alias_normalized))
                .then_with(|| a.id.cmp(&b.id))
        }),
        PromptSortRow::MostUsed => entries.sort_by(|a, b| {
            b.usage_count
                .cmp(&a.usage_count)
                .then_with(|| {
                    b.last_used_at_ms
                        .unwrap_or(i64::MIN)
                        .cmp(&a.last_used_at_ms.unwrap_or(i64::MIN))
                })
                .then_with(|| a.alias_normalized.cmp(&b.alias_normalized))
                .then_with(|| a.id.cmp(&b.id))
        }),
        PromptSortRow::RecentlyUpdated => entries.sort_by(|a, b| {
            b.updated_at_ms
                .cmp(&a.updated_at_ms)
                .then_with(|| a.alias_normalized.cmp(&b.alias_normalized))
                .then_with(|| a.id.cmp(&b.id))
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SmartRank {
    exact_alias: i32,
    alias_prefix: i32,
    pinned: i32,
    tag: i32,
    content: i32,
    last_used_at_ms: Reverse<i64>,
    usage_count: Reverse<i64>,
    alias_normalized: String,
    id: String,
}

fn smart_rank(entry: &PromptEntryRow, query_normalized: Option<&str>) -> SmartRank {
    let exact_alias = query_normalized
        .filter(|query| {
            entry
                .alias_normalized
                .as_deref()
                .is_some_and(|alias| alias == *query)
        })
        .map(|_| 0)
        .unwrap_or(1);
    let alias_prefix = query_normalized
        .filter(|query| {
            entry
                .alias_normalized
                .as_deref()
                .is_some_and(|alias| alias.starts_with(*query))
        })
        .map(|_| 0)
        .unwrap_or(1);
    let tag = query_normalized
        .filter(|query| entry.tags_normalized.iter().any(|tag| tag.contains(*query)))
        .map(|_| 0)
        .unwrap_or(1);
    let content = query_normalized
        .filter(|query| entry.content_normalized.contains(*query))
        .map(|_| 0)
        .unwrap_or(1);

    SmartRank {
        exact_alias,
        alias_prefix,
        pinned: if entry.is_pinned { 0 } else { 1 },
        tag,
        content,
        last_used_at_ms: Reverse(entry.last_used_at_ms.unwrap_or(i64::MIN)),
        usage_count: Reverse(entry.usage_count),
        alias_normalized: entry.alias_normalized.clone().unwrap_or_default(),
        id: entry.id.clone(),
    }
}

fn row_to_prompt_entry(row: &Row<'_>) -> rusqlite::Result<PromptEntryRow> {
    Ok(PromptEntryRow {
        id: row.get(0)?,
        content: row.get(1)?,
        content_normalized: row.get(2)?,
        alias: row.get(3)?,
        alias_normalized: row.get(4)?,
        tags: Vec::new(),
        tags_normalized: Vec::new(),
        is_pinned: row.get::<_, i64>(5)? != 0,
        usage_count: row.get(6)?,
        last_used_at_ms: row.get(7)?,
        created_at_ms: row.get(8)?,
        updated_at_ms: row.get(9)?,
    })
}

#[derive(Default)]
struct LoadedTags {
    display: Vec<String>,
    normalized: Vec<String>,
}

fn load_tags_for_entry_ids(
    conn: &Connection,
    prompt_entry_ids: &[&str],
) -> Result<HashMap<String, LoadedTags>> {
    if prompt_entry_ids.is_empty() {
        return Ok(HashMap::new());
    }

    const SQLITE_CHUNK_SIZE: usize = 900;
    let mut tags_by_id: HashMap<String, LoadedTags> = HashMap::new();

    for chunk in prompt_entry_ids.chunks(SQLITE_CHUNK_SIZE) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT prompt_entry_id, tag, tag_normalized FROM prompt_entry_tags \
             WHERE prompt_entry_id IN ({placeholders}) \
             ORDER BY prompt_entry_id, rowid"
        );
        let mut stmt = conn
            .prepare(&sql)
            .context("failed to prepare prompt entry tags batch query")?;
        let rows = stmt.query_map(params_from_iter(chunk.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;

        for row in rows {
            let (prompt_entry_id, tag, tag_normalized) = row?;
            let loaded_tags = tags_by_id.entry(prompt_entry_id).or_default();
            loaded_tags.display.push(tag);
            loaded_tags.normalized.push(tag_normalized);
        }
    }

    Ok(tags_by_id)
}

fn map_alias_constraint_error(err: rusqlite::Error) -> anyhow::Error {
    if err.sqlite_error_code() == Some(ErrorCode::ConstraintViolation) {
        anyhow!("an alias with this name already exists: {err}")
    } else {
        anyhow!(err)
    }
}

fn prompt_action_as_str(action: &PromptActionRow) -> &'static str {
    match action {
        PromptActionRow::Copy => "copy",
        PromptActionRow::Paste => "paste",
    }
}

fn prompt_use_surface_as_str(surface: &PromptUseSurfaceRow) -> &'static str {
    match surface {
        PromptUseSurfaceRow::Overlay => "overlay",
        PromptUseSurfaceRow::Page => "page",
    }
}

fn prompt_use_outcome_as_str(outcome: &PromptUseOutcomeRow) -> &'static str {
    match outcome {
        PromptUseOutcomeRow::Copy => "copy",
        PromptUseOutcomeRow::PasteAttempted => "paste_attempted",
        PromptUseOutcomeRow::PasteFellBackToCopy => "paste_fell_back_to_copy",
    }
}

fn prompt_use_failure_reason_as_str(reason: &PromptUseFailureReasonRow) -> &'static str {
    match reason {
        PromptUseFailureReasonRow::PermissionMissing => "permission_missing",
        PromptUseFailureReasonRow::FocusLost => "focus_lost",
        PromptUseFailureReasonRow::InjectionFailed => "injection_failed",
        PromptUseFailureReasonRow::UnsupportedPlatform => "unsupported_platform",
    }
}
