//! # Changelog Module — Snapshot Evolution and Delta History
//!
//! Generates a chronological changelog across backup snapshots.
//! Starting from an initial baseline snapshot (showing total file and directory counts),
//! it details the step-by-step evolution (added, deleted, and updated files and directories)
//! across each consecutive snapshot.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::compact::{discover_all_snapshots, SnapshotMeta};
use crate::config::{Config, SnapshotTimezone};
use crate::error::{IncrError, Result};
use crate::manifest::{self, FileEntry};
use crate::rclone::Rclone;

// ─── Options & Data Structures ──────────────────────────────────────────────

/// Options for the changelog command.
#[derive(Debug, Clone, Default)]
pub struct ChangelogOptions {
    /// Start of snapshot timestamp range (inclusive, baseline snapshot).
    pub from: Option<String>,
    /// End of snapshot timestamp range (inclusive).
    pub to: Option<String>,
    /// Output as JSON.
    pub json: bool,
}

/// Baseline information for the initial snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitialSnapshotInfo {
    /// Name of the initial snapshot.
    pub name: String,
    /// Timestamp when the initial snapshot was created (UTC).
    pub timestamp: DateTime<Utc>,
    /// Total number of files in the initial snapshot.
    pub file_count: usize,
    /// Total number of directories in the initial snapshot.
    pub dir_count: usize,
}

/// Changes to files between two consecutive snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChanges {
    pub added_count: usize,
    pub deleted_count: usize,
    pub updated_count: usize,
    pub added: Vec<String>,
    pub deleted: Vec<String>,
    pub updated: Vec<String>,
}

/// Changes to directories between two consecutive snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryChanges {
    pub added_count: usize,
    pub deleted_count: usize,
    pub updated_count: usize,
    pub added: Vec<String>,
    pub deleted: Vec<String>,
    pub updated: Vec<String>,
}

/// Transition delta between two consecutive snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotTransition {
    /// Name of the newer snapshot.
    pub snapshot: String,
    /// Name of the older snapshot compared against.
    pub previous_snapshot: String,
    /// Timestamp when the newer snapshot was created (UTC).
    pub timestamp: DateTime<Utc>,
    /// File changes.
    pub files: FileChanges,
    /// Directory changes.
    pub directories: DirectoryChanges,
}

/// Complete changelog result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangelogResult {
    /// The initial baseline snapshot (if any matching snapshots exist).
    pub initial: Option<InitialSnapshotInfo>,
    /// Step-by-step diff transitions between consecutive snapshots.
    pub transitions: Vec<SnapshotTransition>,
}

// ─── Directory Extraction ───────────────────────────────────────────────────

/// Extract all unique ancestor directories from a list of file entries.
///
/// Directories are derived from relative file paths (e.g., `photos/summer/pic.jpg`
/// yields `photos` and `photos/summer`). Files in the root directory have no ancestor.
pub fn extract_directories(files: &[FileEntry]) -> HashSet<String> {
    let mut dirs = HashSet::new();
    for file in files {
        let parts: Vec<&str> = file.path.split('/').collect();
        for i in 1..parts.len() {
            dirs.insert(parts[..i].join("/"));
        }
    }
    dirs
}

// ─── Core Logic ─────────────────────────────────────────────────────────────

/// Parse a boundary timestamp string or snapshot name.
pub fn parse_bound_timestamp(
    input: &str,
    is_end_bound: bool,
    snapshots: &[SnapshotMeta],
) -> Result<DateTime<Utc>> {
    // 1. Check if input matches an existing snapshot name or stem
    if let Some(snap) = snapshots
        .iter()
        .find(|s| s.name == input || s.manifest_name.trim_end_matches(".json") == input)
    {
        return Ok(snap.created_at);
    }

    // 2. Check if input is a date-only "YYYY-MM-DD"
    if let Ok(date) = chrono::NaiveDate::parse_from_str(input, "%Y-%m-%d") {
        let naive = if is_end_bound {
            date.and_hms_milli_opt(23, 59, 59, 999).unwrap()
        } else {
            date.and_hms_opt(0, 0, 0).unwrap()
        };
        return Ok(naive.and_utc());
    }

    // 3. Try standard snapshot timestamp parser (UTC Z, offset, colon-offset)
    if let Some(dt) = crate::snapshot::parse_snapshot_timestamp(input) {
        return Ok(dt);
    }

    // 4. Try standard RFC3339
    if let Ok(dt) = DateTime::parse_from_rfc3339(input) {
        return Ok(dt.with_timezone(&Utc));
    }

    Err(IncrError::InvalidConfig(format!(
        "Invalid timestamp or snapshot name: '{input}'"
    )))
}

/// Compute changes between two snapshots.
pub fn diff_snapshot_pair(
    prev: &SnapshotMeta,
    curr: &SnapshotMeta,
) -> SnapshotTransition {
    // Manifest diff: target (curr) vs base (prev)
    let manifest_diff = manifest::diff_manifests(&curr.manifest, &prev.manifest);

    let mut files_added: Vec<String> = manifest_diff.added.into_iter().map(|f| f.path).collect();
    files_added.sort();

    let mut files_deleted: Vec<String> = manifest_diff.deleted.into_iter().map(|f| f.path).collect();
    files_deleted.sort();

    let mut files_updated: Vec<String> = manifest_diff
        .modified
        .into_iter()
        .map(|(new, _old)| new.path)
        .collect();
    files_updated.sort();

    // Directories in previous and current snapshots
    let prev_dirs = extract_directories(&prev.manifest.files);
    let curr_dirs = extract_directories(&curr.manifest.files);

    // Added directories: in curr but not in prev
    let mut dirs_added: Vec<String> = curr_dirs.difference(&prev_dirs).cloned().collect();
    dirs_added.sort();

    // Deleted directories: in prev but not in curr
    let mut dirs_deleted: Vec<String> = prev_dirs.difference(&curr_dirs).cloned().collect();
    dirs_deleted.sort();

    // Updated directories: present in both, but had changes within them
    let common_dirs: HashSet<&String> = prev_dirs.intersection(&curr_dirs).collect();
    let mut updated_dirs_set = HashSet::new();

    // For any changed file (added, deleted, updated), identify ancestor directories in common_dirs
    let all_changed_files = files_added
        .iter()
        .chain(files_deleted.iter())
        .chain(files_updated.iter());

    for path in all_changed_files {
        let parts: Vec<&str> = path.split('/').collect();
        for i in 1..parts.len() {
            let ancestor = parts[..i].join("/");
            if common_dirs.contains(&ancestor) {
                updated_dirs_set.insert(ancestor);
            }
        }
    }

    let mut dirs_updated: Vec<String> = updated_dirs_set.into_iter().collect();
    dirs_updated.sort();

    SnapshotTransition {
        snapshot: curr.name.clone(),
        previous_snapshot: prev.name.clone(),
        timestamp: curr.created_at,
        files: FileChanges {
            added_count: files_added.len(),
            deleted_count: files_deleted.len(),
            updated_count: files_updated.len(),
            added: files_added,
            deleted: files_deleted,
            updated: files_updated,
        },
        directories: DirectoryChanges {
            added_count: dirs_added.len(),
            deleted_count: dirs_deleted.len(),
            updated_count: dirs_updated.len(),
            added: dirs_added,
            deleted: dirs_deleted,
            updated: dirs_updated,
        },
    }
}

/// Filter a slice of chronologically sorted snapshots by optional start (`from`) and end (`to`) bounds.
///
/// Semantics match changelog:
/// - `from`: first snapshot on or after this timestamp (inclusive).
/// - `to`: latest snapshot on or before this timestamp (inclusive).
/// - If `from` is specified and no snapshot is on or after it, returns empty.
/// - If `to` is specified and no snapshot is on or before it, returns empty.
/// - If `to` precedes `from`, returns empty.
/// - If `snapshots` is empty, returns empty.
pub fn filter_snapshots_by_bounds<'a>(
    snapshots: &'a [SnapshotMeta],
    from: Option<&str>,
    to: Option<&str>,
) -> Result<&'a [SnapshotMeta]> {
    if snapshots.is_empty() {
        return Ok(&[]);
    }

    let from_dt = match from {
        Some(s) => Some(parse_bound_timestamp(s, false, snapshots)?),
        None => None,
    };

    let to_dt = match to {
        Some(s) => Some(parse_bound_timestamp(s, true, snapshots)?),
        None => None,
    };

    // Find the first snapshot on or after from_dt (or earliest snapshot if omitted)
    let start_idx = match from_dt {
        Some(f_dt) => snapshots.iter().position(|s| s.created_at >= f_dt),
        None => Some(0),
    };

    // Find the latest snapshot on or before to_dt (or latest snapshot if omitted)
    let end_idx = match to_dt {
        Some(t_dt) => snapshots.iter().rposition(|s| s.created_at <= t_dt),
        None => snapshots.len().checked_sub(1),
    };

    let (Some(start_idx), Some(end_idx)) = (start_idx, end_idx) else {
        return Ok(&[]);
    };

    if end_idx < start_idx {
        return Ok(&[]);
    }

    Ok(&snapshots[start_idx..=end_idx])
}

/// Run the changelog operation.
pub fn run_changelog(
    config: &Config,
    rclone: &Rclone,
    options: &ChangelogOptions,
) -> Result<ChangelogResult> {
    let all_snapshots = discover_all_snapshots(rclone, &config.remote_path)?;

    let in_range = filter_snapshots_by_bounds(
        &all_snapshots,
        options.from.as_deref(),
        options.to.as_deref(),
    )?;

    if in_range.is_empty() {
        return Ok(ChangelogResult {
            initial: None,
            transitions: Vec::new(),
        });
    }

    let initial_snap = &in_range[0];
    let initial_dirs = extract_directories(&initial_snap.manifest.files);

    let initial = InitialSnapshotInfo {
        name: initial_snap.name.clone(),
        timestamp: initial_snap.created_at,
        file_count: initial_snap.manifest.file_count,
        dir_count: initial_dirs.len(),
    };

    let mut transitions = Vec::new();
    for window in in_range.windows(2) {
        let prev = &window[0];
        let curr = &window[1];
        transitions.push(diff_snapshot_pair(prev, curr));
    }

    Ok(ChangelogResult {
        initial: Some(initial),
        transitions,
    })
}

/// Format the changelog result as human-readable text.
pub fn format_changelog(result: &ChangelogResult, timezone: SnapshotTimezone) -> String {
    let mut out = String::new();

    let initial = match &result.initial {
        Some(init) => init,
        None => return "No snapshots found in range.\n".to_string(),
    };

    let display_time = timezone.format_display_datetime(initial.timestamp);
    out.push_str(&format!(
        "Initial snapshot: {} ({})\n  Files: {}\n  Directories: {}\n",
        initial.name, display_time, initial.file_count, initial.dir_count
    ));

    if result.transitions.is_empty() {
        out.push_str("\n(no subsequent snapshots to compare)\n");
        return out;
    }

    for trans in &result.transitions {
        out.push('\n');
        let snap_time = timezone.format_display_datetime(trans.timestamp);
        out.push_str(&format!(
            "Snapshot: {} ({}) [compared to {}]\n",
            trans.snapshot, snap_time, trans.previous_snapshot
        ));

        // Files
        out.push_str(&format!(
            "  Files: {} added, {} deleted, {} updated\n",
            trans.files.added_count, trans.files.deleted_count, trans.files.updated_count
        ));
        if !trans.files.added.is_empty() {
            out.push_str(&format!("    Added ({}):\n", trans.files.added_count));
            for f in &trans.files.added {
                out.push_str(&format!("      + {f}\n"));
            }
        }
        if !trans.files.deleted.is_empty() {
            out.push_str(&format!("    Deleted ({}):\n", trans.files.deleted_count));
            for f in &trans.files.deleted {
                out.push_str(&format!("      - {f}\n"));
            }
        }
        if !trans.files.updated.is_empty() {
            out.push_str(&format!("    Updated ({}):\n", trans.files.updated_count));
            for f in &trans.files.updated {
                out.push_str(&format!("      ~ {f}\n"));
            }
        }

        // Directories
        out.push_str(&format!(
            "  Directories: {} added, {} deleted, {} updated\n",
            trans.directories.added_count, trans.directories.deleted_count, trans.directories.updated_count
        ));
        if !trans.directories.added.is_empty() {
            out.push_str(&format!("    Added ({}):\n", trans.directories.added_count));
            for d in &trans.directories.added {
                out.push_str(&format!("      + {d}\n"));
            }
        }
        if !trans.directories.deleted.is_empty() {
            out.push_str(&format!("    Deleted ({}):\n", trans.directories.deleted_count));
            for d in &trans.directories.deleted {
                out.push_str(&format!("      - {d}\n"));
            }
        }
        if !trans.directories.updated.is_empty() {
            out.push_str(&format!("    Updated ({}):\n", trans.directories.updated_count));
            for d in &trans.directories.updated {
                out.push_str(&format!("      ~ {d}\n"));
            }
        }
    }

    out
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;

    fn sample_file(path: &str, size: u64, md5: &str) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            size,
            md5: md5.to_string(),
            mtime: "2026-10-28T08:00:00Z".to_string(),
        }
    }

    fn sample_meta(name: &str, files: Vec<FileEntry>, created_at: DateTime<Utc>) -> SnapshotMeta {
        let manifest = Manifest::new(name, files, &created_at);
        SnapshotMeta {
            name: name.to_string(),
            manifest_name: format!("{name}.json"),
            created_at,
            diff_dir: Some(name.to_string()),
            manifest,
        }
    }

    #[test]
    fn test_extract_directories() {
        let files = vec![
            sample_file("file_in_root.txt", 10, "md5_1"),
            sample_file("docs/guide.md", 20, "md5_2"),
            sample_file("src/nested/mod.rs", 30, "md5_3"),
            sample_file("src/nested/util.rs", 40, "md5_4"),
        ];

        let dirs = extract_directories(&files);
        assert_eq!(dirs.len(), 3);
        assert!(dirs.contains("docs"));
        assert!(dirs.contains("src"));
        assert!(dirs.contains("src/nested"));
    }

    #[test]
    fn test_diff_snapshot_pair() {
        let t1 = DateTime::parse_from_rfc3339("2026-10-28T08:00:00Z").unwrap().with_timezone(&Utc);
        let t2 = DateTime::parse_from_rfc3339("2026-10-28T09:00:00Z").unwrap().with_timezone(&Utc);

        let snap1 = sample_meta(
            "snap1",
            vec![
                sample_file("file1.txt", 10, "hash1"),
                sample_file("src/file2.txt", 20, "hash2_old"),
                sample_file("to_delete/del.txt", 30, "hash3"),
            ],
            t1,
        );

        let snap2 = sample_meta(
            "snap2",
            vec![
                sample_file("file1.txt", 10, "hash1"),           // unchanged
                sample_file("src/file2.txt", 25, "hash2_new"),   // updated
                sample_file("added_dir/new.txt", 40, "hash4"),   // added
            ],
            t2,
        );

        let transition = diff_snapshot_pair(&snap1, &snap2);

        assert_eq!(transition.snapshot, "snap2");
        assert_eq!(transition.previous_snapshot, "snap1");

        // Files
        assert_eq!(transition.files.added, vec!["added_dir/new.txt"]);
        assert_eq!(transition.files.deleted, vec!["to_delete/del.txt"]);
        assert_eq!(transition.files.updated, vec!["src/file2.txt"]);
        assert_eq!(transition.files.added_count, 1);
        assert_eq!(transition.files.deleted_count, 1);
        assert_eq!(transition.files.updated_count, 1);

        // Directories
        assert_eq!(transition.directories.added, vec!["added_dir"]);
        assert_eq!(transition.directories.deleted, vec!["to_delete"]);
        assert_eq!(transition.directories.updated, vec!["src"]);
        assert_eq!(transition.directories.added_count, 1);
        assert_eq!(transition.directories.deleted_count, 1);
        assert_eq!(transition.directories.updated_count, 1);
    }

    #[test]
    fn test_format_changelog() {
        let t1 = DateTime::parse_from_rfc3339("2026-10-28T08:00:00Z").unwrap().with_timezone(&Utc);
        let t2 = DateTime::parse_from_rfc3339("2026-10-28T09:00:00Z").unwrap().with_timezone(&Utc);

        let snap1 = sample_meta(
            "snap1",
            vec![
                sample_file("src/main.rs", 100, "h1"),
                sample_file("README.md", 50, "h2"),
            ],
            t1,
        );

        let snap2 = sample_meta(
            "snap2",
            vec![
                sample_file("src/main.rs", 120, "h1_mod"),
                sample_file("docs/manual.md", 200, "h3"),
            ],
            t2,
        );

        let transition = diff_snapshot_pair(&snap1, &snap2);

        let result = ChangelogResult {
            initial: Some(InitialSnapshotInfo {
                name: "snap1".to_string(),
                timestamp: t1,
                file_count: 2,
                dir_count: 1,
            }),
            transitions: vec![transition],
        };

        let formatted = format_changelog(&result, SnapshotTimezone::Utc);
        assert!(formatted.contains("Initial snapshot: snap1 (2026-10-28T08:00Z)"));
        assert!(formatted.contains("Files: 2"));
        assert!(formatted.contains("Directories: 1"));
        assert!(formatted.contains("Snapshot: snap2 (2026-10-28T09:00Z) [compared to snap1]"));
        assert!(formatted.contains("+ docs/manual.md"));
        assert!(formatted.contains("- README.md"));
        assert!(formatted.contains("~ src/main.rs"));
        assert!(formatted.contains("+ docs"));
        assert!(formatted.contains("~ src"));
    }

    #[test]
    fn test_filter_snapshots_by_bounds() {
        let t1 = DateTime::parse_from_rfc3339("2026-10-28T08:00:00Z").unwrap().with_timezone(&Utc);
        let t2 = DateTime::parse_from_rfc3339("2026-10-28T09:00:00Z").unwrap().with_timezone(&Utc);
        let t3 = DateTime::parse_from_rfc3339("2026-10-28T10:00:00Z").unwrap().with_timezone(&Utc);

        let snap1 = sample_meta("snap1", vec![], t1);
        let snap2 = sample_meta("snap2", vec![], t2);
        let snap3 = sample_meta("snap3", vec![], t3);

        let snaps = vec![snap1, snap2, snap3];

        // Empty bounds returns all
        let res = filter_snapshots_by_bounds(&snaps, None, None).unwrap();
        assert_eq!(res.len(), 3);

        // Filter by name: from snap2
        let res = filter_snapshots_by_bounds(&snaps, Some("snap2"), None).unwrap();
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].name, "snap2");
        assert_eq!(res[1].name, "snap3");

        // Filter by name: to snap2
        let res = filter_snapshots_by_bounds(&snaps, None, Some("snap2")).unwrap();
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].name, "snap1");
        assert_eq!(res[1].name, "snap2");

        // Filter by name: from snap2 to snap2
        let res = filter_snapshots_by_bounds(&snaps, Some("snap2"), Some("snap2")).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].name, "snap2");

        // Filter by timestamp
        let res = filter_snapshots_by_bounds(
            &snaps,
            Some("2026-10-28T08:30:00Z"),
            Some("2026-10-28T09:30:00Z"),
        )
        .unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].name, "snap2");

        // Inverted range
        let res = filter_snapshots_by_bounds(&snaps, Some("snap3"), Some("snap1")).unwrap();
        assert!(res.is_empty());

        // Empty snapshots input
        let empty: Vec<SnapshotMeta> = vec![];
        let res = filter_snapshots_by_bounds(&empty, Some("snap1"), None).unwrap();
        assert!(res.is_empty());

        // Invalid bound error
        let err = filter_snapshots_by_bounds(&snaps, Some("non-existent"), None).unwrap_err();
        assert!(err.to_string().contains("Invalid timestamp or snapshot name"));
    }
}

