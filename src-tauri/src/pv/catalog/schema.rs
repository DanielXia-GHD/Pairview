//! Catalog schema, as an ordered list of migrations.
//!
//! Rules:
//! - Never edit a migration that has shipped. Add a new one at the end.
//! - Times are Unix epoch milliseconds (`INTEGER`). Relative paths use `/` separators.
//! - The catalog is a cache (docs/IMPLEMENTATION_PLAN.md, A3): folders and sidecars stay the
//!   source of truth, so rows can always be rebuilt by a rescan.

/// `MIGRATIONS[i]` upgrades the catalog from version `i` to version `i + 1`.
pub const MIGRATIONS: &[&str] = &[V1, V2];

/// The schema version this build writes.
pub const LATEST_VERSION: u32 = MIGRATIONS.len() as u32;

const V1: &str = r#"
-- Library Locations (PRD §6.1.1). `path` is where the location is mounted now; the volume
-- identity lets M1-02 find the same folder again at a new mount path.
CREATE TABLE locations (
    id              INTEGER PRIMARY KEY,
    name            TEXT    NOT NULL,
    path            TEXT    NOT NULL UNIQUE,
    volume_id       TEXT,                          -- volume UUID, or smb://server/share
    volume_rel_path TEXT,                          -- location path relative to the volume root
    sort_order      INTEGER NOT NULL DEFAULT 0,
    settings_json   TEXT    NOT NULL DEFAULT '{}', -- excludes, poll interval, index scope, watch
    added_at        INTEGER NOT NULL,
    last_scan_at    INTEGER
);

-- Every folder under a location. The location root itself has rel_path ''.
CREATE TABLE folders (
    id            INTEGER PRIMARY KEY,
    location_id   INTEGER NOT NULL REFERENCES locations(id) ON DELETE CASCADE,
    parent_id     INTEGER REFERENCES folders(id) ON DELETE CASCADE,
    rel_path      TEXT    NOT NULL,
    name          TEXT    NOT NULL,
    format_subdir INTEGER NOT NULL DEFAULT 0 CHECK (format_subdir IN (0, 1)), -- arw/, hif/, … (PRD §5.1)
    date_hint     TEXT,                                                       -- YYYY-MM-DD parsed from the name
    mtime         INTEGER,
    scanned_at    INTEGER,
    UNIQUE (location_id, rel_path)
);
CREATE INDEX folders_parent ON folders(parent_id);

-- Every file the scanner listed (L0: listing only, no reads).
CREATE TABLE files (
    id        INTEGER PRIMARY KEY,
    folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    name      TEXT    NOT NULL,
    stem_key  TEXT    NOT NULL, -- lower-cased name without extension: the pairing key (PRD §5.3)
    ext       TEXT    NOT NULL, -- lower-cased, without the dot
    kind      TEXT    NOT NULL CHECK (kind IN ('raw', 'display', 'video', 'sidecar', 'other')),
    size      INTEGER NOT NULL,
    mtime     INTEGER NOT NULL,
    UNIQUE (folder_id, name)
);
CREATE INDEX files_stem ON files(folder_id, stem_key);

-- One Photo per frame (PRD §5.2). The shoot is the folder that owns the format subfolders.
-- A stem can map to two Photos when the timestamp guard splits a pair, so (shoot, stem) is
-- not unique.
CREATE TABLE photos (
    id                     INTEGER PRIMARY KEY,
    shoot_folder_id        INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    stem_key               TEXT    NOT NULL,
    state                  TEXT    NOT NULL CHECK (state IN
                               ('raw_display', 'edited', 'display_only', 'raw_only', 'orphan_raw')),
    representative_file_id INTEGER REFERENCES files(id) ON DELETE SET NULL, -- RAW if present (A4)
    display_file_id        INTEGER REFERENCES files(id) ON DELETE SET NULL, -- what the viewer shows (A5)
    captured_at            TEXT,    -- local capture time, ISO 8601 without zone
    date_source            TEXT    CHECK (date_source IN ('exif', 'folder', 'mtime')),
    date_uncertain         INTEGER NOT NULL DEFAULT 0 CHECK (date_uncertain IN (0, 1)),
    guard_mismatch         INTEGER NOT NULL DEFAULT 0 CHECK (guard_mismatch IN (0, 1)),
    created_at             INTEGER NOT NULL,
    updated_at             INTEGER NOT NULL
);
CREATE INDEX photos_shoot ON photos(shoot_folder_id, stem_key);
CREATE INDEX photos_captured ON photos(captured_at);

-- Files that make up a Photo. A file belongs to at most one Photo.
CREATE TABLE photo_members (
    photo_id INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    file_id  INTEGER NOT NULL UNIQUE REFERENCES files(id) ON DELETE CASCADE,
    role     TEXT    NOT NULL CHECK (role IN ('raw', 'display', 'extra', 'sidecar')),
    PRIMARY KEY (photo_id, file_id)
);

-- L1 metadata per file. `file_mtime` is the file's mtime when this row was read, so a changed
-- file is re-read.
CREATE TABLE exif (
    file_id         INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    file_mtime      INTEGER NOT NULL,
    date_original   TEXT,    -- DateTimeOriginal, ISO 8601 without zone
    subsec_ms       INTEGER,
    offset_original TEXT,    -- OffsetTimeOriginal, e.g. +09:00
    make            TEXT,
    model           TEXT,
    serial          TEXT,
    lens            TEXT,
    orientation     INTEGER,
    width           INTEGER,
    height          INTEGER,
    iso             INTEGER,
    exposure_s      REAL,
    f_number        REAL,
    focal_mm        REAL,
    gps_lat         REAL,
    gps_lon         REAL,
    gps_alt         REAL,
    read_at         INTEGER NOT NULL
);

-- Human, Photo-level metadata (A6). XMP is the source of truth; this mirrors it for queries.
-- AI results get their own tables later, never these columns (PRD §6.5 hard rule).
CREATE TABLE metadata (
    photo_id   INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    rating     INTEGER NOT NULL DEFAULT 0 CHECK (rating BETWEEN 0 AND 5),
    flag       TEXT    NOT NULL DEFAULT 'none' CHECK (flag IN ('none', 'pick', 'reject')),
    label      TEXT,
    keep_raw   TEXT    NOT NULL DEFAULT 'auto' CHECK (keep_raw IN ('auto', 'keep', 'drop')),
    xmp_mtime  INTEGER,
    updated_at INTEGER NOT NULL
);

-- Write-ahead log for file operations (A7, A8). Rows outlive their location so the history of
-- what happened to files is kept.
CREATE TABLE ops_log (
    id           TEXT    PRIMARY KEY, -- op id, also the .pairview-deleted/<op-id> folder name
    kind         TEXT    NOT NULL CHECK (kind IN
                     ('delete', 'drop_raw', 'overwrite', 'rename', 'tidy', 'restore', 'purge')),
    status       TEXT    NOT NULL CHECK (status IN ('pending', 'committed', 'rolled_back', 'failed')),
    location_id  INTEGER REFERENCES locations(id) ON DELETE SET NULL,
    payload_json TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    finished_at  INTEGER
);
CREATE INDEX ops_log_status ON ops_log(status);
"#;

const V2: &str = r#"
-- Image statistics behind the RAW-value signals (M2-07), from the file the grid shows. Cached
-- per source file version; the verdict itself is derived on request with ratings, flags, EXIF
-- and the lock, so human input always counts at once.
CREATE TABLE raw_signals (
    photo_id       INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    source_file_id INTEGER NOT NULL,
    source_mtime   INTEGER NOT NULL,
    clipped        REAL    NOT NULL, -- share of pixels with a channel at the top (0..1)
    shadows        REAL    NOT NULL, -- share of pixels near black (0..1)
    luma_low       INTEGER NOT NULL, -- 1st percentile of luma (0..255)
    luma_high      INTEGER NOT NULL, -- 99th percentile of luma (0..255)
    red_blue       REAL    NOT NULL, -- mean R / mean B over mid-tones (gray-world white balance)
    green_magenta  REAL    NOT NULL, -- mean G / mean (R+B)/2 over mid-tones
    computed_at    INTEGER NOT NULL
);
"#;
