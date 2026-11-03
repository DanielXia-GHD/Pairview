//! Brings a catalog up to the schema version this build expects.

use anyhow::{Context, Result, bail};
use rusqlite::Connection;

use super::schema::MIGRATIONS;

/// Applies every missing migration, each in its own transaction, and returns the resulting
/// version. A database written by a newer Pairview is refused instead of being modified.
pub fn migrate(conn: &mut Connection) -> Result<u32> {
    migrate_with(conn, MIGRATIONS)
}

pub(super) fn migrate_with(conn: &mut Connection, migrations: &[&str]) -> Result<u32> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             version    INTEGER PRIMARY KEY,
             applied_at INTEGER NOT NULL
         );",
    )?;

    let current = current_version(conn)?;
    let latest = migrations.len() as u32;
    if current > latest {
        bail!(
            "catalog schema v{current} is newer than this build supports (v{latest}); \
             update Pairview or delete the catalog to rebuild it"
        );
    }

    for (index, sql) in migrations.iter().enumerate().skip(current as usize) {
        let version = index as u32 + 1;
        let tx = conn.transaction()?;
        tx.execute_batch(sql)
            .with_context(|| format!("catalog migration to v{version} failed"))?;
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            (version, super::now_ms()),
        )?;
        tx.commit()?;
    }

    Ok(latest)
}

pub fn current_version(conn: &Connection) -> Result<u32> {
    let version: Option<u32> =
        conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })?;
    Ok(version.unwrap_or(0))
}
