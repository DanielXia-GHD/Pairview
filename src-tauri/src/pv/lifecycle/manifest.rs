//! `manifest.json`: what one operation put in Recently Deleted (A8).
//!
//! Each operation folder `<location>/.pairview-deleted/<op-id>/` holds the files at their original
//! relative paths plus this manifest. The manifest is the record on disk: the catalog is a cache
//! (A3), so Recently Deleted can be listed, restored and purged from the manifests alone.

use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "manifest.json";
pub const VERSION: u32 = 1;

/// Why files are in Recently Deleted; the Recently Deleted view groups by it (PRD §6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// The owner deleted Photos or one of their copies.
    Deleted,
    /// RAWs dropped because the display copy is enough (Keep HIF only, slim review).
    Slimmed,
    /// Camera originals replaced by an edited render (M3-02).
    Replaced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Written before the first move. Files of a pending manifest may be half moved; the op-log
    /// replay (`lifecycle::recover`) puts them back.
    Pending,
    Committed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: u32,
    pub op_id: String,
    /// `ops_log.kind` of the operation that wrote it.
    pub kind: String,
    pub reason: Reason,
    pub status: Status,
    pub created_at: i64,
    pub committed_at: Option<i64>,
    pub location: ManifestLocation,
    pub files: Vec<ManifestFile>,
    /// The Photos the files belonged to, for grouping and thumbnails in the Recently Deleted view.
    pub photos: Vec<ManifestPhoto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestLocation {
    pub name: String,
    pub volume_id: Option<String>,
    pub volume_rel_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestFile {
    /// Original path relative to the location root, `/`-separated. The file sits at the same
    /// relative path inside the operation folder.
    pub path: String,
    pub size: u64,
    pub mtime: i64,
    /// `raw`, `display`, `extra`, `sidecar`, or `file` when it wasn't part of a Photo.
    pub role: String,
    /// Index into `photos`.
    pub photo: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestPhoto {
    /// Shoot folder, relative to the location root.
    pub shoot: String,
    pub stem: String,
    pub captured_at: Option<String>,
    pub rating: i64,
}

impl Manifest {
    pub fn read(op_dir: &Path) -> Result<Manifest> {
        let path = op_dir.join(FILE_NAME);
        let text =
            fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
        let manifest: Manifest = serde_json::from_str(&text)
            .with_context(|| format!("{} is not a Pairview manifest", path.display()))?;
        ensure!(
            manifest.version <= VERSION,
            "{} was written by a newer Pairview",
            path.display()
        );
        for f in &manifest.files {
            safe_rel(&f.path)?;
        }
        Ok(manifest)
    }

    pub fn write(&self, op_dir: &Path) -> Result<()> {
        let path = op_dir.join(FILE_NAME);
        let json = serde_json::to_vec_pretty(self)?;
        crate::file_management::write_file_atomic(&path, &json)
            .with_context(|| format!("cannot write {}", path.display()))
    }

    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// A relative path made only of normal components (no `..`, no root, no `.`), as a `PathBuf`.
pub fn safe_rel(rel: &str) -> Result<PathBuf> {
    ensure!(!rel.is_empty(), "empty path");
    let path = Path::new(rel);
    for c in path.components() {
        match c {
            Component::Normal(_) => {}
            _ => bail!("refusing path {rel:?}: not a plain relative path"),
        }
    }
    Ok(path.to_path_buf())
}

/// `base/rel`, refusing a path that leaves `base` through a symlinked folder.
pub fn safe_join(base: &Path, rel: &str) -> Result<PathBuf> {
    let rel = safe_rel(rel)?;
    let mut path = base.to_path_buf();
    let parts: Vec<_> = rel.components().collect();
    for (i, part) in parts.iter().enumerate() {
        path.push(part);
        if i + 1 < parts.len()
            && let Ok(meta) = fs::symlink_metadata(&path)
        {
            ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "refusing {}: {} is not a plain folder",
                rel.display(),
                path.display()
            );
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_relative_paths() {
        assert!(safe_rel("a/b/DSC0001.ARW").is_ok());
        for bad in ["", "/etc/passwd", "../x", "a/../../x", "./a"] {
            assert!(safe_rel(bad).is_err(), "{bad}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn join_refuses_symlinked_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("x"), b"x").unwrap();
        let base = tmp.path().join("base");
        fs::create_dir(&base).unwrap();
        std::os::unix::fs::symlink(&outside, base.join("link")).unwrap();
        assert!(safe_join(&base, "link/x").is_err());
        assert_eq!(safe_join(&base, "a/x").unwrap(), base.join("a/x"));
    }
}
