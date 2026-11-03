//! Pairview catalog: a SQLite cache of locations, folders, files, Photos and their metadata
//! (docs/IMPLEMENTATION_PLAN.md, A3 and M1-01).

// The scanner, pairing engine and commands (M1-03..09) are the first users.
#![allow(dead_code)]

mod migrate;
mod schema;

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::Connection;

pub use migrate::current_version;

/// File name of the catalog inside the app-data directory.
pub const FILE_NAME: &str = "pairview-catalog.sqlite3";

pub struct Catalog {
    conn: Connection,
}

impl Catalog {
    /// Opens (or creates) the catalog at `path` and migrates it to the latest schema.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("cannot open catalog {}", path.display()))?;
        // WAL lets background indexers write while the UI reads.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        migrate::migrate(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    pub fn schema_version(&self) -> Result<u32> {
        current_version(&self.conn)
    }
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::schema::LATEST_VERSION;
    use super::*;
    use std::time::Instant;

    fn insert_location(cat: &Catalog, path: &str) -> i64 {
        cat.conn()
            .execute(
                "INSERT INTO locations (name, path, added_at) VALUES ('Test', ?1, 0)",
                [path],
            )
            .unwrap();
        cat.conn().last_insert_rowid()
    }

    fn insert_folder(cat: &Catalog, location_id: i64, rel_path: &str) -> i64 {
        cat.conn()
            .execute(
                "INSERT INTO folders (location_id, rel_path, name) VALUES (?1, ?2, ?2)",
                (location_id, rel_path),
            )
            .unwrap();
        cat.conn().last_insert_rowid()
    }

    fn insert_file(cat: &Catalog, folder_id: i64, name: &str, kind: &str) -> i64 {
        let (stem, ext) = name.rsplit_once('.').unwrap();
        cat.conn()
            .execute(
                "INSERT INTO files (folder_id, name, stem_key, ext, kind, size, mtime)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, 0)",
                (
                    folder_id,
                    name,
                    stem.to_lowercase(),
                    ext.to_lowercase(),
                    kind,
                ),
            )
            .unwrap();
        cat.conn().last_insert_rowid()
    }

    fn count(cat: &Catalog, table: &str) -> i64 {
        cat.conn()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn creates_latest_schema_from_empty_database() {
        // v0 = a database with no tables at all (first launch).
        let dir = tempfile::tempdir().unwrap();
        let cat = Catalog::open(&dir.path().join(FILE_NAME)).unwrap();
        assert_eq!(cat.schema_version().unwrap(), LATEST_VERSION);
        for table in [
            "locations",
            "folders",
            "files",
            "photos",
            "photo_members",
            "exif",
            "metadata",
            "ops_log",
        ] {
            assert_eq!(count(&cat, table), 0, "{table}");
        }
    }

    #[test]
    fn reopening_keeps_data_and_does_not_reapply_migrations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        {
            let cat = Catalog::open(&path).unwrap();
            insert_location(&cat, "/Volumes/Public/Photos");
        }
        let cat = Catalog::open(&path).unwrap();
        assert_eq!(count(&cat, "locations"), 1);
        assert_eq!(count(&cat, "schema_migrations"), LATEST_VERSION as i64);
    }

    #[test]
    fn upgrades_one_version_at_a_time() {
        let mut conn = Connection::open_in_memory().unwrap();
        let v1 = "CREATE TABLE a (x INTEGER);";
        let v2 = "ALTER TABLE a ADD COLUMN y INTEGER;";
        assert_eq!(migrate::migrate_with(&mut conn, &[v1]).unwrap(), 1);
        conn.execute("INSERT INTO a (x) VALUES (7)", []).unwrap();
        assert_eq!(migrate::migrate_with(&mut conn, &[v1, v2]).unwrap(), 2);
        let (x, y): (i64, Option<i64>) = conn
            .query_row("SELECT x, y FROM a", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((x, y), (7, None));
    }

    #[test]
    fn failed_migration_rolls_back_and_keeps_old_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        let v1 = "CREATE TABLE a (x INTEGER);";
        let bad = "CREATE TABLE b (x INTEGER); THIS IS NOT SQL;";
        migrate::migrate_with(&mut conn, &[v1]).unwrap();
        assert!(migrate::migrate_with(&mut conn, &[v1, bad]).is_err());
        assert_eq!(current_version(&conn).unwrap(), 1);
        let b_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'b'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(b_exists, 0);
    }

    #[test]
    fn refuses_catalog_from_newer_build() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        drop(Catalog::open(&path).unwrap());
        Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, 0)",
                [LATEST_VERSION + 1],
            )
            .unwrap();
        let err = Catalog::open(&path).err().unwrap().to_string();
        assert!(err.contains("newer"), "{err}");
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let cat = Catalog::open_in_memory().unwrap();
        let orphan_file = cat.conn().execute(
            "INSERT INTO files (folder_id, name, stem_key, ext, kind, size, mtime)
             VALUES (999, 'DSC00001.ARW', 'dsc00001', 'arw', 'raw', 1, 0)",
            [],
        );
        assert!(orphan_file.is_err());
    }

    #[test]
    fn removing_a_location_cascades_to_everything_under_it() {
        let cat = Catalog::open_in_memory().unwrap();
        let loc = insert_location(&cat, "/fixtures/a");
        let shoot = insert_folder(&cat, loc, "2026-10-23_10261023");
        let raw = insert_file(&cat, shoot, "DSC03545.ARW", "raw");
        let hif = insert_file(&cat, shoot, "DSC03545.HIF", "display");
        cat.conn()
            .execute(
                "INSERT INTO photos (shoot_folder_id, stem_key, state, representative_file_id,
                                     display_file_id, created_at, updated_at)
                 VALUES (?1, 'dsc03545', 'raw_display', ?2, ?3, 0, 0)",
                (shoot, raw, hif),
            )
            .unwrap();
        let photo = cat.conn().last_insert_rowid();
        cat.conn()
            .execute(
                "INSERT INTO photo_members (photo_id, file_id, role)
                 VALUES (?1, ?2, 'raw'), (?1, ?3, 'display')",
                (photo, raw, hif),
            )
            .unwrap();
        cat.conn()
            .execute(
                "INSERT INTO metadata (photo_id, rating, updated_at) VALUES (?1, 4, 0)",
                [photo],
            )
            .unwrap();
        cat.conn()
            .execute(
                "INSERT INTO ops_log (id, kind, status, location_id, payload_json, created_at)
                 VALUES ('op-1', 'delete', 'committed', ?1, '{}', 0)",
                [loc],
            )
            .unwrap();

        cat.conn()
            .execute("DELETE FROM locations WHERE id = ?1", [loc])
            .unwrap();
        for table in ["folders", "files", "photos", "photo_members", "metadata"] {
            assert_eq!(count(&cat, table), 0, "{table}");
        }
        // The op history survives, detached from the removed location.
        let op_location: Option<i64> = cat
            .conn()
            .query_row("SELECT location_id FROM ops_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(op_location, None);
    }

    #[test]
    fn file_belongs_to_at_most_one_photo() {
        let cat = Catalog::open_in_memory().unwrap();
        let loc = insert_location(&cat, "/fixtures/a");
        let shoot = insert_folder(&cat, loc, "");
        let raw = insert_file(&cat, shoot, "DSC00001.ARW", "raw");
        for _ in 0..2 {
            cat.conn()
                .execute(
                    "INSERT INTO photos (shoot_folder_id, stem_key, state, created_at, updated_at)
                     VALUES (?1, 'dsc00001', 'raw_only', 0, 0)",
                    [shoot],
                )
                .unwrap();
        }
        let insert_member = |photo: i64| {
            cat.conn().execute(
                "INSERT INTO photo_members (photo_id, file_id, role) VALUES (?1, ?2, 'raw')",
                (photo, raw),
            )
        };
        assert!(insert_member(1).is_ok());
        assert!(insert_member(2).is_err());
    }

    #[test]
    fn check_constraints_reject_bad_values() {
        let cat = Catalog::open_in_memory().unwrap();
        let loc = insert_location(&cat, "/fixtures/a");
        let shoot = insert_folder(&cat, loc, "");
        assert!(
            cat.conn()
                .execute(
                    "INSERT INTO files (folder_id, name, stem_key, ext, kind, size, mtime)
                     VALUES (?1, 'a.xyz', 'a', 'xyz', 'photo', 1, 0)",
                    [shoot],
                )
                .is_err()
        );
        cat.conn()
            .execute(
                "INSERT INTO photos (shoot_folder_id, stem_key, state, created_at, updated_at)
                 VALUES (?1, 'a', 'display_only', 0, 0)",
                [shoot],
            )
            .unwrap();
        let photo = cat.conn().last_insert_rowid();
        let set_meta = |column: &str, value: &dyn rusqlite::ToSql| {
            cat.conn().execute(
                &format!(
                    "INSERT OR REPLACE INTO metadata (photo_id, {column}, updated_at)
                     VALUES (?1, ?2, 0)"
                ),
                (photo, value),
            )
        };
        assert!(set_meta("rating", &6).is_err());
        assert!(set_meta("flag", &"maybe").is_err());
        assert!(set_meta("keep_raw", &"later").is_err());
        assert!(set_meta("rating", &5).is_ok());
        assert!(set_meta("keep_raw", &"keep").is_ok());
    }

    #[test]
    fn open_and_close_is_fast() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        drop(Catalog::open(&path).unwrap());
        let start = Instant::now();
        drop(Catalog::open(&path).unwrap());
        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_millis(50), "{elapsed:?}");
    }
}
