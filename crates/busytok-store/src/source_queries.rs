//! Source-related database queries.

use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::Connection;

/// A user-configured log source root read from the database.
/// Agent is kept as a raw string — the caller parses it so that
/// soft-skip + observability (event_code / AppEvent) remains in runtime.
pub struct UserConfiguredSourceRoot {
    pub source_id: String,
    pub agent: String,
    pub root_path: PathBuf,
}

/// List active user-configured log source roots.
///
/// Filters to rows where `configured_by_user != 0` and `status = 'active'`.
/// The caller is responsible for agent string parsing and per-agent discovery.
pub fn list_active_user_configured_roots(
    conn: &Connection,
) -> Result<Vec<UserConfiguredSourceRoot>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, agent, root_path FROM log_sources \
             WHERE configured_by_user != 0 AND status = 'active'",
        )
        .context("failed to prepare user-configured roots query")?;
    let rows = stmt.query_map([], |row| {
        Ok(UserConfiguredSourceRoot {
            source_id: row.get(0)?,
            agent: row.get(1)?,
            root_path: PathBuf::from(row.get::<_, String>(2)?),
        })
    })?;
    let mut sources = Vec::new();
    for row in rows {
        sources.push(row?);
    }
    Ok(sources)
}
