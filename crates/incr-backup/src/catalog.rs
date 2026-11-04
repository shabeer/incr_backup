//! # Catalog Module — High-Performance Remote Snapshot Catalog
//!
//! Maintains a lightweight summary catalog file at `<remote>/.meta/snapshots.jsonl`.
//!
//! ## Overview
//! Scanning hundreds or thousands of full snapshot manifest files sequentially with
//! `rclone cat` is prohibitively slow because each manifest contains thousands of
//! `FileEntry` records.
//!
//! The snapshot catalog stores each snapshot's summary header (name, creation timestamp,
//! associated diff directory, file count, and total size) as a single line of JSON in
//! `.meta/snapshots.jsonl`. Commands like `list` and bounded range queries read this
//! entire index in a single network round-trip.
//!
//! If `.meta/snapshots.jsonl` is missing or corrupted, the catalog self-heals by
//! rebuilding itself from remote manifests using parallel `ManifestHeader` scanning.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::error::{IncrError, Result};
use crate::manifest::ManifestHeader;
use crate::rclone::Rclone;

/// Lightweight summary metadata of a snapshot stored in the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotSummary {
    /// Snapshot identifier (e.g. "2026-10-29T04-28Z", "daily-2026-10-28", "snap1").
    pub name: String,

    /// Filename of the manifest in `.meta/manifests/` (e.g. "snap1.json").
    pub manifest_name: String,

    /// Timestamp of when the snapshot was taken.
    pub created_at: DateTime<Utc>,

    /// Diff directory name in `diffs/`, if it exists.
    pub diff_dir: Option<String>,

    /// Precomputed file count in this snapshot.
    pub file_count: usize,

    /// Precomputed total size in bytes of all files in this snapshot.
    pub total_size: u64,
}

/// Load the snapshot catalog from `<remote>/.meta/snapshots.jsonl` (or fallback `<remote>/.meta/snapshot.jsonl`).
///
/// If the catalog does not exist or cannot be parsed, falls back to rebuilding
/// it by scanning remote manifests and writing the new catalog file.
pub fn load_snapshot_catalog(rclone: &Rclone, remote_base: &str) -> Result<Vec<SnapshotSummary>> {
    let catalog_path = format!("{remote_base}/.meta/snapshots.jsonl");
    let alt_path = format!("{remote_base}/.meta/snapshot.jsonl");

    let loaded = match rclone.cat(&catalog_path) {
        Ok(content) if !content.trim().is_empty() => Some((catalog_path.clone(), content)),
        _ => match rclone.cat(&alt_path) {
            Ok(content) if !content.trim().is_empty() => Some((alt_path.clone(), content)),
            _ => None,
        },
    };

    if let Some((path, content)) = loaded {
        let mut entries = Vec::new();
        let mut parse_failed = false;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<SnapshotSummary>(trimmed) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    log::warn!("Corrupt line in catalog {path}: {e}");
                    parse_failed = true;
                    break;
                }
            }
        }

        if !parse_failed && !entries.is_empty() {
            entries.sort_by_key(|s| s.created_at);
            return Ok(entries);
        }

        log::info!("Catalog was empty or corrupted; rebuilding from manifests");
    }

    rebuild_snapshot_catalog(rclone, remote_base)
}

/// Rebuild the snapshot catalog by scanning `.meta/manifests/` and matching `diffs/`.
///
/// Uses `ManifestHeader` to avoid deserializing the full `files` array, and scans
/// files in parallel via Rayon. Automatically saves the rebuilt catalog to the remote.
pub fn rebuild_snapshot_catalog(
    rclone: &Rclone,
    remote_base: &str,
) -> Result<Vec<SnapshotSummary>> {
    let manifests_dir = format!("{remote_base}/.meta/manifests/");
    let diffs_dir = format!("{remote_base}/diffs/");

    let manifest_files = match rclone.lsf(&manifests_dir) {
        Ok(files) => files,
        Err(_) => Vec::new(),
    };

    let existing_diff_dirs: HashSet<String> = match rclone.lsd(&diffs_dir) {
        Ok(dirs) => dirs
            .into_iter()
            .filter(|d| !crate::snapshot::is_tombstone_diff_dir(d))
            .collect(),
        Err(_) => HashSet::new(),
    };

    let valid_files: Vec<String> = manifest_files
        .into_iter()
        .filter(|f| f.ends_with(".json") && f != "current.json")
        .collect();

    // Parallel fetch & parse using rayon
    let summaries: Vec<SnapshotSummary> = valid_files
        .par_iter()
        .filter_map(|file| {
            let remote_manifest_path = format!("{manifests_dir}{file}");
            let content = match rclone.cat(&remote_manifest_path) {
                Ok(c) => c,
                Err(e) => {
                    log::debug!("Failed to read manifest {file}: {e}");
                    return None;
                }
            };

            let header = match ManifestHeader::from_json(&content) {
                Ok(h) => h,
                Err(e) => {
                    log::warn!("Failed to parse manifest header {file}: {e}");
                    return None;
                }
            };

            let stem = file.trim_end_matches(".json").to_string();
            let name = if header.snapshot_name.is_empty() {
                stem.clone()
            } else {
                header.snapshot_name.clone()
            };

            let prefix_stem = format!("{stem}__");
            let prefix_name = format!("{name}__");
            let diff_dir = if existing_diff_dirs.contains(&stem) {
                Some(stem)
            } else if existing_diff_dirs.contains(&name) {
                Some(name.clone())
            } else if let Some(d) = existing_diff_dirs.iter().find(|d| d.starts_with(&prefix_stem)) {
                Some(d.clone())
            } else {
                existing_diff_dirs
                    .iter()
                    .find(|d| !name.is_empty() && d.starts_with(&prefix_name))
                    .cloned()
            };

            Some(SnapshotSummary {
                name,
                manifest_name: file.clone(),
                created_at: header.created_at,
                diff_dir,
                file_count: header.file_count,
                total_size: header.total_size,
            })
        })
        .collect();

    let mut sorted = summaries;
    sorted.sort_by_key(|s| s.created_at);

    // Save rebuilt catalog back to remote (ignoring errors if remote is read-only)
    let _ = save_snapshot_catalog(rclone, remote_base, &sorted);

    Ok(sorted)
}

/// Save the snapshot catalog to a specific target path.
pub fn save_snapshot_catalog_to(
    rclone: &Rclone,
    target_path: &str,
    catalog: &[SnapshotSummary],
) -> Result<()> {
    let mut lines = String::new();
    for entry in catalog {
        let line = serde_json::to_string(entry).map_err(IncrError::Json)?;
        lines.push_str(&line);
        lines.push('\n');
    }
    rclone.rcat(target_path, &lines)?;
    Ok(())
}

/// Save the snapshot catalog to `<remote>/.meta/snapshots.jsonl`.
pub fn save_snapshot_catalog(
    rclone: &Rclone,
    remote_base: &str,
    catalog: &[SnapshotSummary],
) -> Result<()> {
    let catalog_path = format!("{remote_base}/.meta/snapshots.jsonl");
    save_snapshot_catalog_to(rclone, &catalog_path, catalog)
}

/// Append or update a single snapshot in the remote catalog.
pub fn append_or_update_catalog_entry(
    rclone: &Rclone,
    remote_base: &str,
    entry: SnapshotSummary,
) -> Result<()> {
    let catalog_path = format!("{remote_base}/.meta/snapshots.jsonl");
    let alt_path = format!("{remote_base}/.meta/snapshot.jsonl");
    let (active_path, content) = match rclone.cat(&catalog_path) {
        Ok(c) if !c.trim().is_empty() => (catalog_path.clone(), Some(c)),
        _ => match rclone.cat(&alt_path) {
            Ok(c) if !c.trim().is_empty() => (alt_path.clone(), Some(c)),
            _ => (catalog_path.clone(), None),
        },
    };

    let mut entries = match content {
        Some(content) => {
            let mut list = Vec::new();
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(item) = serde_json::from_str::<SnapshotSummary>(trimmed) {
                    list.push(item);
                }
            }
            list
        }
        None => {
            // If catalog doesn't exist yet, rebuild it from manifests first
            rebuild_snapshot_catalog(rclone, remote_base).unwrap_or_default()
        }
    };

    // Replace if exists, or append
    entries.retain(|e| e.name != entry.name && e.manifest_name != entry.manifest_name);
    entries.push(entry);
    entries.sort_by_key(|s| s.created_at);

    save_snapshot_catalog_to(rclone, &active_path, &entries)?;
    if active_path == alt_path {
        let _ = save_snapshot_catalog_to(rclone, &catalog_path, &entries);
    }
    Ok(())
}

/// Remove snapshots matching the given names or manifest stems from the remote catalog,
/// and clear diff_dir for any surviving snapshots whose diff directories were purged.
pub fn remove_from_catalog(
    rclone: &Rclone,
    remote_base: &str,
    names_to_remove: &HashSet<String>,
    purged_diff_dirs: &HashSet<String>,
) -> Result<()> {
    let catalog_path = format!("{remote_base}/.meta/snapshots.jsonl");
    let alt_path = format!("{remote_base}/.meta/snapshot.jsonl");
    let (active_path, content) = match rclone.cat(&catalog_path) {
        Ok(c) if !c.trim().is_empty() => (catalog_path.clone(), Some(c)),
        _ => match rclone.cat(&alt_path) {
            Ok(c) if !c.trim().is_empty() => (alt_path.clone(), Some(c)),
            _ => (catalog_path.clone(), None),
        },
    };

    let mut entries = match content {
        Some(content) => {
            let mut list = Vec::new();
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(mut item) = serde_json::from_str::<SnapshotSummary>(trimmed) {
                    let stem = item.manifest_name.trim_end_matches(".json");
                    if !names_to_remove.contains(&item.name)
                        && !names_to_remove.contains(&item.manifest_name)
                        && !names_to_remove.contains(stem)
                    {
                        if let Some(ref d) = item.diff_dir {
                            if purged_diff_dirs.contains(d)
                                || purged_diff_dirs.contains(d.trim_end_matches(crate::snapshot::DELETED_SUFFIX))
                                || purged_diff_dirs.contains(&format!("{}{}", d, crate::snapshot::DELETED_SUFFIX))
                            {
                                item.diff_dir = None;
                            }
                        }
                        list.push(item);
                    }
                }
            }
            list
        }
        None => {
            let mut list = rebuild_snapshot_catalog(rclone, remote_base).unwrap_or_default();
            list.retain(|item| {
                let stem = item.manifest_name.trim_end_matches(".json");
                !names_to_remove.contains(&item.name)
                    && !names_to_remove.contains(&item.manifest_name)
                    && !names_to_remove.contains(stem)
            });
            for item in &mut list {
                if let Some(ref d) = item.diff_dir {
                    if purged_diff_dirs.contains(d)
                        || purged_diff_dirs.contains(d.trim_end_matches(crate::snapshot::DELETED_SUFFIX))
                        || purged_diff_dirs.contains(&format!("{}{}", d, crate::snapshot::DELETED_SUFFIX))
                    {
                        item.diff_dir = None;
                    }
                }
            }
            list
        }
    };

    entries.sort_by_key(|s| s.created_at);
    save_snapshot_catalog_to(rclone, &active_path, &entries)?;
    if active_path == alt_path {
        let _ = save_snapshot_catalog_to(rclone, &catalog_path, &entries);
    }
    Ok(())
}

/// Atomically update catalog after compaction (removes purged snapshots and optionally adds new consolidated snapshot).
pub fn update_catalog_after_compaction(
    rclone: &Rclone,
    remote_base: &str,
    new_entry: Option<SnapshotSummary>,
    purged_names: &HashSet<String>,
) -> Result<()> {
    let catalog_path = format!("{remote_base}/.meta/snapshots.jsonl");
    let alt_path = format!("{remote_base}/.meta/snapshot.jsonl");
    let (active_path, content) = match rclone.cat(&catalog_path) {
        Ok(c) if !c.trim().is_empty() => (catalog_path.clone(), Some(c)),
        _ => match rclone.cat(&alt_path) {
            Ok(c) if !c.trim().is_empty() => (alt_path.clone(), Some(c)),
            _ => (catalog_path.clone(), None),
        },
    };

    let mut entries = match content {
        Some(content) => {
            let mut list = Vec::new();
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(item) = serde_json::from_str::<SnapshotSummary>(trimmed) {
                    let stem = item.manifest_name.trim_end_matches(".json");
                    if !purged_names.contains(&item.name)
                        && !purged_names.contains(&item.manifest_name)
                        && !purged_names.contains(stem)
                    {
                        list.push(item);
                    }
                }
            }
            list
        }
        None => {
            let mut list = rebuild_snapshot_catalog(rclone, remote_base).unwrap_or_default();
            list.retain(|item| {
                let stem = item.manifest_name.trim_end_matches(".json");
                !purged_names.contains(&item.name)
                    && !purged_names.contains(&item.manifest_name)
                    && !purged_names.contains(stem)
            });
            list
        }
    };

    if let Some(entry) = new_entry {
        entries.retain(|e| e.name != entry.name && e.manifest_name != entry.manifest_name);
        entries.push(entry);
    }

    entries.sort_by_key(|s| s.created_at);
    save_snapshot_catalog_to(rclone, &active_path, &entries)?;
    if active_path == alt_path {
        let _ = save_snapshot_catalog_to(rclone, &catalog_path, &entries);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_catalog_json_lines_serialization() {
        let now = Utc::now();
        let summary1 = SnapshotSummary {
            name: "snap1".to_string(),
            manifest_name: "snap1.json".to_string(),
            created_at: now,
            diff_dir: Some("snap1".to_string()),
            file_count: 10,
            total_size: 1024,
        };

        let json = serde_json::to_string(&summary1).unwrap();
        let parsed: SnapshotSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, summary1);
    }

    #[test]
    fn test_manifest_header_deserialization_skips_files() {
        let manifest_json = r#"{
            "snapshot_name": "snap-2026",
            "created_at": "2026-11-01T12:00:00Z",
            "total_size": 999999,
            "file_count": 42,
            "files": [
                {"path": "huge/file1.bin", "size": 500000, "md5": "abc", "mtime": "2026-11-01T12:00:00Z"},
                {"path": "huge/file2.bin", "size": 499999, "md5": "def", "mtime": "2026-11-01T12:00:00Z"}
            ]
        }"#;

        let header = ManifestHeader::from_json(manifest_json).unwrap();
        assert_eq!(header.snapshot_name, "snap-2026");
        assert_eq!(header.total_size, 999999);
        assert_eq!(header.file_count, 42);
    }
}
