//! # Restore Module — Restore with Location Resolution
//!
//! Implements the restore algorithm: given a snapshot name, figure out where
//! each file's correct version lives on the remote (in current/ or a diffs/
//! directory), then download them to a local destination.
//!
//! ## Key Insight
//! Because rclone sync --backup-dir MOVES old versions, a file at snapshot
//! point X might be in current/ (if unchanged since X) or in any of the
//! diff directories between X and now (if changed since X).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use log::{debug, info, warn};

use crate::config::Config;
use crate::error::{IncrError, Result};
use crate::manifest::{FileEntry, Manifest};
use crate::rclone::Rclone;
use crate::scanner;
use crate::snapshot::{SnapshotInfo, SnapshotName, SnapshotTier, RotationManager};

/// Options for a restore operation.
#[derive(Debug, Clone)]
pub struct RestoreOptions {
    /// Which snapshot to restore (e.g., "hourly.2", "daily.0", "current").
    pub snapshot: String,
    /// Local directory to restore files into.
    pub destination: PathBuf,
    /// Optionally restore only a subdirectory or single file.
    pub subpath: Option<String>,
    /// If true, verify checksums after restore.
    pub verify_after: bool,
    /// If true, allow restoring to a non-empty directory.
    pub overwrite: bool,
    /// If true, only show what would be restored without downloading.
    pub dry_run: bool,
}

/// Where a file lives on the remote for a given snapshot point.
#[derive(Debug, Clone)]
pub struct ResolveEntry {
    /// The file's metadata from the target manifest.
    pub file: FileEntry,
    /// Full remote path where this version lives
    /// (e.g., "diffs/hourly.1/documents/report.pdf" or "current/file.txt").
    pub remote_path: String,
}

/// Result of a restore operation.
#[derive(Debug)]
pub struct RestoreResult {
    pub files_restored: usize,
    pub bytes_restored: u64,
    pub duration: std::time::Duration,
    pub verify_passed: Option<bool>,
}

/// Run the restore pipeline.
///
/// ## Algorithm
/// 1. Validate snapshot exists and destination is ready
/// 2. If restoring "current" → simple rclone copy
/// 3. Otherwise: load target manifest, resolve file locations, group by
///    remote directory, download each group
/// 4. Optionally verify checksums
pub fn run_restore(config: &Config, rclone: &Rclone, options: &RestoreOptions) -> Result<RestoreResult> {
    let start = Instant::now();
    let remote_base = &config.remote_path;

    info!("Restoring snapshot '{}' to '{}'", options.snapshot, options.destination.display());

    // Step 1: Validate destination
    if options.destination.exists() && !options.overwrite {
        // Check if non-empty
        let is_empty = options.destination.read_dir()
            .map(|mut d| d.next().is_none())
            .unwrap_or(false);
        if !is_empty {
            return Err(IncrError::RestoreDestNotEmpty {
                path: options.destination.clone(),
            });
        }
    }

    if !options.dry_run {
        std::fs::create_dir_all(&options.destination).map_err(|e| {
            IncrError::RestoreDestCreation {
                path: options.destination.clone(),
                source: e,
            }
        })?;
    }

    // Step 2: Restoring "current" is a simple rclone copy
    if options.snapshot == "current" {
        return restore_current(config, rclone, options, start);
    }

    // Step 3: Load target manifest
    let manifest_path = format!(
        "{}/.meta/manifests/{}.json",
        remote_base,
        options.snapshot.replace("__", ".").split("__").next().unwrap_or(&options.snapshot)
    );

    let manifest_json = rclone.cat(&manifest_path).map_err(|_| {
        IncrError::SnapshotNotFound(options.snapshot.clone())
    })?;

    let target_manifest = Manifest::from_json(&manifest_json)?;

    // Step 4: Get snapshots between target and current
    let (target_tier, target_index) = SnapshotName::parse_ref(&options.snapshot)?;
    let retention = &config.retention;
    let rotation_mgr = RotationManager::new(rclone, remote_base, retention);

    let snapshots_after = get_snapshots_after(rclone, &rotation_mgr, remote_base, &target_tier, target_index)?;

    // Step 5: Resolve file locations
    let mut resolved = resolve_locations(&target_manifest, &snapshots_after);

    // Step 6: Apply subpath filter if specified
    if let Some(ref subpath) = options.subpath {
        resolved.retain(|e| e.file.path.starts_with(subpath));
        info!("Filtered to {} files under '{}'", resolved.len(), subpath);
    }

    info!("Resolved {} files for restore", resolved.len());

    if options.dry_run {
        for entry in &resolved {
            info!("[DRY-RUN] Would restore {} from {}", entry.file.path, entry.remote_path);
        }
        return Ok(RestoreResult {
            files_restored: resolved.len(),
            bytes_restored: resolved.iter().map(|e| e.file.size).sum(),
            duration: start.elapsed(),
            verify_passed: None,
        });
    }

    // Step 7: Group by remote source directory and download
    let groups = group_by_remote_dir(&resolved, remote_base);
    let mut total_files = 0usize;
    let mut total_bytes = 0u64;

    for (remote_dir, entries) in &groups {
        // Write a files-from list to a temp file
        let files_from_path = options.destination.join(".incr-restore-files-from.txt");
        {
            let mut f = std::fs::File::create(&files_from_path)?;
            for entry in entries {
                writeln!(f, "{}", entry.file.path)?;
            }
        }

        debug!("Copying {} files from {}", entries.len(), remote_dir);
        rclone.copy_files(
            remote_dir,
            &options.destination.to_string_lossy(),
            &files_from_path.to_string_lossy(),
        )?;

        total_files += entries.len();
        total_bytes += entries.iter().map(|e| e.file.size).sum::<u64>();

        // Cleanup temp file
        let _ = std::fs::remove_file(&files_from_path);
    }

    // Step 8: Optionally verify
    let verify_passed = if options.verify_after {
        Some(verify_restore(&options.destination, &target_manifest)?)
    } else {
        None
    };

    Ok(RestoreResult {
        files_restored: total_files,
        bytes_restored: total_bytes,
        duration: start.elapsed(),
        verify_passed,
    })
}

/// Simple restore of the current state (latest full mirror).
fn restore_current(
    config: &Config,
    rclone: &Rclone,
    options: &RestoreOptions,
    start: Instant,
) -> Result<RestoreResult> {
    let remote_current = format!("{}/current/", config.remote_path);

    if options.dry_run {
        info!("[DRY-RUN] Would copy {} to {}", remote_current, options.destination.display());
        return Ok(RestoreResult {
            files_restored: 0,
            bytes_restored: 0,
            duration: start.elapsed(),
            verify_passed: None,
        });
    }

    let source = if let Some(ref subpath) = options.subpath {
        format!("{}{}", remote_current, subpath)
    } else {
        remote_current
    };

    rclone.copy(&source, &options.destination.to_string_lossy())?;

    Ok(RestoreResult {
        files_restored: 0, // Would need to count after copy
        bytes_restored: 0,
        duration: start.elapsed(),
        verify_passed: None,
    })
}

/// Snapshot diff directory with its set of files.
#[derive(Debug, Clone)]
pub struct SnapshotDiffFiles {
    pub dir_name: String,
    pub files: std::collections::HashSet<String>,
}

/// Resolve where each file lives on the remote for a given snapshot.
///
/// For each file in the target manifest, search through diff directories
/// from closest (newest) to farthest (oldest-after-target). If the file
/// was changed in a diff, its old version is there. If it wasn't changed
/// at all, it's still in current/.
///
/// ## Algorithm
/// Similar to Java's `Stream.filter().findFirst()` pattern:
/// walk through diff directories from newest-to-oldest after the target,
/// find the first one containing the file.
pub fn resolve_locations(
    target_manifest: &Manifest,
    snapshots_after_target: &[SnapshotDiffFiles],
) -> Vec<ResolveEntry> {
    target_manifest.files.iter().map(|file| {
        // Check each diff directory after the target (closest first)
        for snap in snapshots_after_target {
            if snap.files.contains(&file.path) {
                return ResolveEntry {
                    file: file.clone(),
                    remote_path: format!("diffs/{}/{}", snap.dir_name, file.path),
                };
            }
        }
        // Not in any diff → unchanged, still in current/
        ResolveEntry {
            file: file.clone(),
            remote_path: format!("current/{}", file.path),
        }
    }).collect()
}

/// Get all snapshots between the target and current, ordered closest-first.
fn get_snapshots_after(
    rclone: &Rclone,
    rotation_mgr: &RotationManager,
    remote_base: &str,
    target_tier: &SnapshotTier,
    target_index: u32,
) -> Result<Vec<SnapshotDiffFiles>> {
    let all = rotation_mgr.list_snapshots(target_tier)?;

    // Scan snapshots from 0 up to target_index
    let after: Vec<SnapshotInfo> = all
        .into_iter()
        .filter(|s| s.name.index <= target_index)
        .collect();

    let mut diff_snaps = Vec::new();
    for snap in after {
        let diff_path = format!("{}/diffs/{}", remote_base, snap.dir_name);
        let files = rclone.lsf_files_recursive(&diff_path).unwrap_or_default();
        let file_set: std::collections::HashSet<String> = files.into_iter().collect();
        diff_snaps.push(SnapshotDiffFiles {
            dir_name: snap.dir_name,
            files: file_set,
        });
    }

    Ok(diff_snaps)
}

/// Group resolved entries by their remote source directory.
///
/// Files from current/ are grouped together, files from each diffs/ directory
/// are grouped separately. This minimizes the number of rclone copy commands.
fn group_by_remote_dir<'a>(entries: &'a [ResolveEntry], remote_base: &str) -> HashMap<String, Vec<&'a ResolveEntry>> {
    let mut groups: HashMap<String, Vec<&ResolveEntry>> = HashMap::new();

    for entry in entries {
        // Extract the top-level directory (current/ or diffs/<snapshot>/)
        let dir = if entry.remote_path.starts_with("current/") {
            format!("{}/current/", remote_base)
        } else if entry.remote_path.starts_with("diffs/") {
            // Extract "diffs/<snapshot_dir>/"
            let parts: Vec<&str> = entry.remote_path.splitn(3, '/').collect();
            if parts.len() >= 2 {
                format!("{}/diffs/{}/", remote_base, parts[1])
            } else {
                format!("{}/{}", remote_base, entry.remote_path)
            }
        } else {
            format!("{}/{}", remote_base, entry.remote_path)
        };

        groups.entry(dir).or_default().push(entry);
    }

    groups
}

/// Verify restored files against the manifest by computing local MD5 hashes.
fn verify_restore(destination: &Path, manifest: &Manifest) -> Result<bool> {
    info!("Verifying restored files against manifest...");
    let mut mismatches = 0;

    for file in &manifest.files {
        let local_path = destination.join(&file.path);
        if !local_path.exists() {
            warn!("Missing: {}", file.path);
            mismatches += 1;
            continue;
        }

        let local_md5 = scanner::hash_file(&local_path)?;
        if local_md5 != file.md5 {
            warn!(
                "Hash mismatch: {} (expected {}, got {})",
                file.path, file.md5, local_md5
            );
            mismatches += 1;
        }
    }

    if mismatches == 0 {
        info!("All {} files verified successfully ✓", manifest.file_count);
        Ok(true)
    } else {
        warn!("{} files failed verification ✗", mismatches);
        Ok(false)
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use crate::manifest::FileEntry;

    fn sample_entry(path: &str, md5: &str) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            size: 100,
            md5: md5.to_string(),
            mtime: "2026-10-28T08:00:00Z".to_string(),
        }
    }

    #[test]
    fn test_resolve_locations_no_diffs() {
        // When there are no snapshots after the target, all files come from current/
        let manifest = Manifest::new(
            "hourly.2",
            vec![
                sample_entry("a.txt", "aaa"),
                sample_entry("b.txt", "bbb"),
            ],
            &Utc::now(),
        );

        let resolved = resolve_locations(&manifest, &[]);

        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].remote_path, "current/a.txt");
        assert_eq!(resolved[1].remote_path, "current/b.txt");
    }

    #[test]
    fn test_group_by_remote_dir() {
        let entries = vec![
            ResolveEntry {
                file: sample_entry("a.txt", "aaa"),
                remote_path: "current/a.txt".to_string(),
            },
            ResolveEntry {
                file: sample_entry("b.txt", "bbb"),
                remote_path: "current/b.txt".to_string(),
            },
            ResolveEntry {
                file: sample_entry("c.txt", "ccc"),
                remote_path: "diffs/hourly.0__2026-10-28T08-00/c.txt".to_string(),
            },
        ];

        let groups = group_by_remote_dir(&entries, "remote:backup");

        assert_eq!(groups.len(), 2); // current/ and diffs/hourly.0/
        assert_eq!(groups["remote:backup/current/"].len(), 2);
    }
}

