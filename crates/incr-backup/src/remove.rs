//! # Remove Module — Purge Files/Directories from All Snapshots
//!
//! Enables removing a specific file or directory tree from all snapshots
//! (both current mirror and historical diff snapshots) and updating all
//! manifests accordingly.
//!
//! ## Analogy
//! Like `git filter-repo --path <path> --invert-paths` or `restic forget --prune`,
//! but tailored for reverse-incremental cloud backups:
//! - Deletes matching files from `current/`
//! - Deletes matching files from all historical diff directories `diffs/*`
//! - Rewrites all manifests in `.meta/manifests/` to remove the file entries
//!   and adjust file counts and total sizes
//! - Acquires remote advisory cloud locks during operation to prevent race conditions
//!
//! ## Common Use Cases
//! - **Accidental Sensitive Data Backup**: An API key, `.env` file, or password
//!   file was committed and backed up, and must be completely erased from all
//!   backup history.
//! - **Compliance / GDPR "Right to be Forgotten"**: User data must be purged
//!   from all backup records across all time.
//! - **Space Reclamation**: A large temporary directory (e.g. `build/`, `video/`)
//!   was backed up accidentally and needs to be reclaimed across all snapshots.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{IncrError, Result};
use crate::lock::LockManager;
use crate::manifest::Manifest;
use crate::rclone::Rclone;
use crate::scanner::matches_glob;

// ─── Options & Results ──────────────────────────────────────────────────────

/// Options controlling the snapshot removal operation (`remove_snapshots`).
#[derive(Debug, Clone)]
pub struct RemoveSnapshotsOptions {
    /// End of snapshot timestamp range (inclusive: all snapshots on or before this timestamp are deleted).
    pub to: String,

    /// If `true`, break an existing advisory lock if present.
    pub force_lock: bool,

    /// If `true`, simulate the removal without modifying remote files or manifests.
    pub dry_run: bool,

    /// If `true`, produce output in JSON format.
    pub json: bool,
}

/// Summary result of a completed (or dry-run) `remove_snapshots` operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoveSnapshotsResult {
    /// The target end timestamp / snapshot name requested.
    pub to: String,

    /// The resolved UTC cutoff timestamp (inclusive).
    pub to_timestamp: DateTime<Utc>,

    /// Total number of snapshot manifests scanned.
    pub snapshots_scanned: usize,

    /// Number of snapshots deleted (or would be deleted in dry-run).
    pub snapshots_removed: usize,

    /// Names of snapshots deleted (or would be deleted in dry-run).
    pub snapshots: Vec<String>,

    /// Diff directories purged (including diff directory of the first surviving snapshot, if any).
    pub diff_dirs_purged: Vec<String>,

    /// Remaining snapshot count after removal.
    pub snapshots_remaining: usize,

    /// Whether this was a dry-run simulation.
    pub dry_run: bool,
}

/// Options controlling the remove operation.
#[derive(Debug, Clone)]
pub struct RemoveOptions {
    /// Relative path(s) or pattern(s) to remove (e.g., `"secret.txt"`, `"docs/private"`).
    pub targets: Vec<String>,

    /// If `true`, break an existing advisory lock if present.
    pub force_lock: bool,

    /// If `true`, simulate the removal without modifying remote files or manifests.
    pub dry_run: bool,

    /// If `true`, produce output in JSON format.
    pub json: bool,
}

/// Details of files removed from a specific snapshot manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotRemoveDetail {
    /// Snapshot identifier (e.g. `"current"`, `"hourly.0"`, `"daily.1"`).
    pub snapshot: String,

    /// Number of matching files removed from this snapshot.
    pub files_removed: usize,

    /// Total bytes freed in this snapshot manifest.
    pub bytes_removed: u64,

    /// List of relative file paths removed.
    pub files: Vec<String>,
}

/// Summary result of a completed (or dry-run) remove operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveResult {
    /// Target patterns/paths requested to be removed.
    pub targets: Vec<String>,

    /// Total number of snapshot manifests scanned.
    pub snapshots_scanned: usize,

    /// Names of snapshots that contained matching files and were modified.
    pub snapshots_affected: Vec<String>,

    /// Count of distinct unique file paths removed across all snapshots.
    pub unique_files_removed: usize,

    /// Distinct unique file paths removed across all snapshots.
    pub unique_paths: Vec<String>,

    /// Total count of file entries removed across all snapshot manifests.
    pub total_file_instances_removed: usize,

    /// Total byte size of all removed file entries across all snapshot manifests.
    pub total_bytes_removed: u64,

    /// Breakdown of removals per snapshot.
    pub details: Vec<SnapshotRemoveDetail>,

    /// Whether this was a dry-run simulation.
    pub dry_run: bool,
}

// ─── Path & Target Normalization ────────────────────────────────────────────

/// Normalize a target path: trim whitespace, strip leading `./` or `/`,
/// and strip trailing slashes.
///
/// Disallows empty paths or `.` which would match the root directory.
pub fn normalize_target(target: &str) -> Result<String> {
    let mut s = target.trim();
    while s.starts_with("./") {
        s = &s[2..];
    }
    s = s.trim_start_matches('/');
    s = s.trim_end_matches('/');

    if s.is_empty() || s == "." {
        return Err(IncrError::InvalidTarget(
            "cannot remove root directory or empty path from snapshots".to_string(),
        ));
    }

    Ok(s.to_string())
}

/// Check if a relative file path matches a normalized target pattern or path.
///
/// Matching rules:
/// 1. Exact path match: `file_path == norm_target` (e.g. `"secret.txt" == "secret.txt"`)
/// 2. Directory prefix match: `file_path` starts with `"norm_target/"` (e.g. `"docs/secret/key.pem"`)
/// 3. Wildcard / glob matching:
///    - If `norm_target` contains `*` or `?`, matches against `file_path`
///    - Also matches against the filename/basename if `norm_target` has no directory component
pub fn matches_target(file_path: &str, norm_target: &str) -> bool {
    if norm_target.is_empty() {
        return false;
    }

    // 1. Exact match
    if file_path == norm_target {
        return true;
    }

    // 2. Directory prefix match (files inside target directory)
    let dir_prefix = format!("{norm_target}/");
    if file_path.starts_with(&dir_prefix) {
        return true;
    }

    // 3. Glob matching if pattern contains wildcards
    if norm_target.contains('*') || norm_target.contains('?') {
        if matches_glob(file_path, norm_target) {
            return true;
        }
        // Match just the filename if pattern is a file glob (e.g., "*.key")
        if !norm_target.contains('/')
            && let Some(basename) = file_path.rsplit('/').next()
            && matches_glob(basename, norm_target)
        {
            return true;
        }
    }

    false
}

// ─── Main Pipeline ──────────────────────────────────────────────────────────

/// Execute the removal of a directory or file(s) across all snapshots.
///
/// # Workflow
/// 1. Normalize and validate target paths.
/// 2. Acquire remote advisory lock.
/// 3. Scan all manifests in `.meta/manifests/`. For each manifest containing
///    matching files:
///    - Filter out matching files.
///    - Recompute `file_count` and `total_size`.
///    - Overwrite remote manifest with updated JSON (unless dry-run).
/// 4. Delete physical files from `current/` and `diffs/*` using rclone.
/// 5. Clean up any empty directory structures using `rclone rmdirs --leave-root`.
/// 6. Release advisory lock and return summary.
pub fn run_remove(
    config: &Config,
    rclone: &Rclone,
    options: &RemoveOptions,
) -> Result<RemoveResult> {
    if options.targets.is_empty() {
        return Err(IncrError::InvalidTarget(
            "no target paths specified to remove".to_string(),
        ));
    }

    // Step 1: Normalize all target paths
    let mut norm_targets = Vec::with_capacity(options.targets.len());
    for t in &options.targets {
        norm_targets.push(normalize_target(t)?);
    }
    // Deduplicate normalized targets
    norm_targets.sort();
    norm_targets.dedup();

    info!(
        "Preparing to remove target(s) across all snapshots: {:?}",
        norm_targets
    );

    // Step 2: Acquire advisory cloud lock
    let lock_mgr = LockManager::new(rclone, &config.remote_path, config.timezone);
    if options.dry_run {
        info!("[DRY-RUN] Would acquire remote lock");
    } else {
        lock_mgr.acquire(options.force_lock)?;
        debug!("Remote lock acquired");
    }

    // Execute the removal, ensuring lock release on finish or error
    let result = run_remove_inner(config, rclone, options, &norm_targets);

    if !options.dry_run {
        if let Err(e) = lock_mgr.release() {
            warn!("Failed to release remote lock: {e}");
        } else {
            debug!("Remote lock released");
        }
    }

    result
}

fn run_remove_inner(
    config: &Config,
    rclone: &Rclone,
    options: &RemoveOptions,
    norm_targets: &[String],
) -> Result<RemoveResult> {
    let remote_base = &config.remote_path;
    let manifests_dir = format!("{remote_base}/.meta/manifests/");

    // Step 3: Discover all manifest files
    let manifest_files = match rclone.lsf(&manifests_dir) {
        Ok(files) => files,
        Err(_) => {
            info!("No manifests directory found at {}", manifests_dir);
            Vec::new()
        }
    };

    let mut manifest_files: Vec<String> = manifest_files
        .into_iter()
        .filter(|f| f.ends_with(".json"))
        .collect();
    manifest_files.sort();

    let snapshots_scanned = manifest_files.len();
    let mut snapshots_affected = Vec::new();
    let mut details = Vec::new();
    let mut unique_files_set = HashSet::new();
    let mut total_file_instances = 0usize;
    let mut total_bytes = 0u64;

    // Process each manifest file
    for file in &manifest_files {
        let remote_manifest_path = format!("{manifests_dir}{file}");
        let content = match rclone.cat(&remote_manifest_path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Could not read manifest {remote_manifest_path}: {e}");
                continue;
            }
        };

        let mut manifest = match Manifest::from_json(&content) {
            Ok(m) => m,
            Err(e) => {
                warn!("Could not parse manifest {remote_manifest_path}: {e}");
                continue;
            }
        };

        let mut removed_for_snap = Vec::new();
        let mut bytes_for_snap = 0u64;
        let mut kept_files = Vec::with_capacity(manifest.files.len());

        for entry in manifest.files {
            if norm_targets.iter().any(|t| matches_target(&entry.path, t)) {
                bytes_for_snap += entry.size;
                unique_files_set.insert(entry.path.clone());
                removed_for_snap.push(entry.path);
            } else {
                kept_files.push(entry);
            }
        }

        if !removed_for_snap.is_empty() {
            let snap_name = file.strip_suffix(".json").unwrap_or(file).to_string();
            snapshots_affected.push(snap_name.clone());
            total_file_instances += removed_for_snap.len();
            total_bytes += bytes_for_snap;

            manifest.files = kept_files;
            manifest.file_count = manifest.files.len();
            manifest.total_size = manifest.files.iter().map(|f| f.size).sum();

            if options.dry_run {
                info!(
                    "[DRY-RUN] Manifest '{}': would remove {} file(s) ({})",
                    snap_name,
                    removed_for_snap.len(),
                    crate::manifest::format_size(bytes_for_snap)
                );
            } else {
                let updated_json = manifest.to_json()?;
                rclone.rcat(&remote_manifest_path, &updated_json)?;
                info!(
                    "Updated manifest '{}': removed {} file(s)",
                    snap_name,
                    removed_for_snap.len()
                );
            }

            details.push(SnapshotRemoveDetail {
                snapshot: snap_name,
                files_removed: removed_for_snap.len(),
                bytes_removed: bytes_for_snap,
                files: removed_for_snap,
            });
        }
    }

    // Step 4: Delete matching physical files on the remote storage
    // Build include patterns for rclone
    let mut includes = Vec::new();
    for t in norm_targets {
        includes.push(t.clone());
        if !t.contains('*') && !t.contains('?') {
            includes.push(format!("{t}/**"));
        }
    }

    // 4a. Delete matching files from current/ (file-level deletion, no rmdirs on current/)
    let current_dir = format!("{remote_base}/current");
    debug!("Purging matching files from {}", current_dir);
    rclone.delete(&current_dir, &includes)?;
    for t in norm_targets {
        let _ = rclone.rmdir_specific(&format!("{current_dir}/{t}"));
    }

    // 4b. Delete matching files from all diffs/*/ directories (file-level deletion)
    let diffs_base = format!("{remote_base}/diffs/");
    if let Ok(diff_dirs) = rclone.lsd(&diffs_base) {
        for diff_dir in diff_dirs {
            let diff_snap_path = format!("{remote_base}/diffs/{diff_dir}");
            debug!("Purging matching files from {}", diff_snap_path);
            rclone.delete(&diff_snap_path, &includes)?;
            for t in norm_targets {
                let _ = rclone.rmdir_specific(&format!("{diff_snap_path}/{t}"));
            }
        }
    }

    let mut unique_paths: Vec<String> = unique_files_set.into_iter().collect();
    unique_paths.sort();

    Ok(RemoveResult {
        targets: norm_targets.to_vec(),
        snapshots_scanned,
        snapshots_affected,
        unique_files_removed: unique_paths.len(),
        unique_paths,
        total_file_instances_removed: total_file_instances,
        total_bytes_removed: total_bytes,
        details,
        dry_run: options.dry_run,
    })
}

// ─── Remove Snapshots Pipeline ──────────────────────────────────────────────

/// Execute the removal of all snapshots up to and including a given cutoff timestamp.
///
/// # Workflow
/// 1. Validate `--to` cutoff parameter.
/// 2. Acquire remote advisory lock.
/// 3. Load snapshot catalog from `.meta/snapshots.jsonl` (or `.meta/snapshot.jsonl`), automatically rebuilding it from manifests if missing.
/// 4. Resolve cutoff timestamp via `parse_bound_timestamp`.
/// 5. Filter snapshots with `created_at <= cutoff`.
/// 6. Determine diff directories to purge (including diff directory of the first surviving snapshot,
///    which contains only diffs against the deleted history).
/// 7. If dry-run, log actions and return summary without modifications.
/// 8. Purge diff directories, delete remote manifests, and clean up empty directories.
/// 9. Update snapshot catalog, release lock, and return summary.
pub fn run_remove_snapshots(
    config: &Config,
    rclone: &Rclone,
    options: &RemoveSnapshotsOptions,
) -> Result<RemoveSnapshotsResult> {
    if options.to.trim().is_empty() {
        return Err(IncrError::InvalidTarget(
            "--to timestamp cannot be empty".to_string(),
        ));
    }

    let remote_base = &config.remote_path;

    // Step 1: Acquire advisory cloud lock
    let lock_mgr = LockManager::new(rclone, remote_base, config.timezone);
    if options.dry_run {
        info!("[DRY-RUN] Would acquire remote lock");
    } else {
        lock_mgr.acquire(options.force_lock)?;
        debug!("Remote lock acquired");
    }

    let result = run_remove_snapshots_inner(config, rclone, options);

    if !options.dry_run {
        if let Err(e) = lock_mgr.release() {
            warn!("Failed to release remote lock: {e}");
        } else {
            debug!("Remote lock released");
        }
    }

    result
}

fn run_remove_snapshots_inner(
    config: &Config,
    rclone: &Rclone,
    options: &RemoveSnapshotsOptions,
) -> Result<RemoveSnapshotsResult> {
    let remote_clean = config.remote_path.trim_end_matches('/');

    // Step 2: Load snapshots from the catalog (.meta/snapshots.jsonl or .meta/snapshot.jsonl)
    // If the catalog is not available, load_snapshot_catalog rebuilds it and saves it.
    let mut all_snapshots = crate::catalog::load_snapshot_catalog(rclone, remote_clean)?;
    all_snapshots.sort_by_key(|s| s.created_at);

    let snapshots_scanned = all_snapshots.len();

    // Step 3: Parse cutoff timestamp
    let cutoff_dt = crate::changelog::parse_bound_timestamp(&options.to, true, &all_snapshots)?;

    // Step 4: Split snapshots into to_remove (<= cutoff_dt) and to_keep (> cutoff_dt)
    let mut to_remove = Vec::new();
    let mut to_keep = Vec::new();

    for snap in all_snapshots {
        if snap.created_at <= cutoff_dt {
            to_remove.push(snap);
        } else {
            to_keep.push(snap);
        }
    }

    let snapshots_removed = to_remove.len();
    let snapshots_remaining = to_keep.len();
    let snapshot_names: Vec<String> = to_remove.iter().map(|s| s.name.clone()).collect();

    // Step 5: Determine diff directories to purge
    let mut diff_dirs_to_purge = Vec::new();

    let diffs_base = format!("{remote_clean}/diffs");
    let existing_diff_dirs: HashSet<String> = rclone.lsd(&diffs_base).unwrap_or_default().into_iter().collect();

    let resolve_diff_dirs_for_snap = |snap: &crate::catalog::SnapshotSummary| -> Vec<String> {
        let mut found = Vec::new();
        let stem = snap.manifest_name.trim_end_matches(".json");

        // 1. Direct snap.diff_dir if populated in catalog
        if let Some(ref d) = snap.diff_dir {
            let clean = d.trim_matches('/');
            if (existing_diff_dirs.is_empty() || existing_diff_dirs.contains(clean))
                && !found.contains(&clean.to_string())
            {
                found.push(clean.to_string());
            }
        }

        // 2. Exact matches in existing_diff_dirs
        if existing_diff_dirs.contains(stem) && !found.contains(&stem.to_string()) {
            found.push(stem.to_string());
        }
        if !snap.name.is_empty() && existing_diff_dirs.contains(&snap.name) && !found.contains(&snap.name) {
            found.push(snap.name.clone());
        }

        // 3. Prefix matches in existing_diff_dirs (e.g. hourly.0__2026...)
        let prefix_stem = format!("{stem}__");
        let prefix_name = format!("{}__", snap.name);
        for d in &existing_diff_dirs {
            if (d.starts_with(&prefix_stem) || (!snap.name.is_empty() && d.starts_with(&prefix_name)))
                && !found.contains(d)
            {
                found.push(d.clone());
            }
        }

        // 4. Restore helper find_snapshot_diff_dir
        if found.is_empty() {
            if let Some(d) = crate::restore::find_snapshot_diff_dir(rclone, remote_clean, &snap.name) {
                if !found.contains(&d) {
                    found.push(d);
                }
            }
            if let Some(d) = crate::restore::find_snapshot_diff_dir(rclone, remote_clean, stem) {
                if !found.contains(&d) {
                    found.push(d);
                }
            }
        }

        // 5. Fallback if existing_diff_dirs was empty (e.g. lsd failed on remote)
        if found.is_empty() && existing_diff_dirs.is_empty() {
            if let Some(ref d) = snap.diff_dir {
                found.push(d.trim_matches('/').to_string());
            } else if !snap.name.is_empty() {
                found.push(snap.name.clone());
            } else if !stem.is_empty() {
                found.push(stem.to_string());
            }
        }

        found
    };

    // 5a. Diffs belonging to the snapshots being deleted
    for snap in &to_remove {
        for dir in resolve_diff_dirs_for_snap(snap) {
            if !diff_dirs_to_purge.contains(&dir) {
                diff_dirs_to_purge.push(dir);
            }
        }
    }

    // 5b. If we removed at least one snapshot, and there is a surviving snapshot,
    // its diff directory contains diffs against the deleted history.
    // Purge it so the surviving snapshot becomes a clean baseline with zero orphaned files.
    if !to_remove.is_empty() && let Some(first_survivor) = to_keep.first() {
        for dir in resolve_diff_dirs_for_snap(first_survivor) {
            if !diff_dirs_to_purge.contains(&dir) {
                diff_dirs_to_purge.push(dir);
            }
        }
    }

    diff_dirs_to_purge.sort();

    // Step 6: If dry-run, return plan without mutating
    if options.dry_run {
        info!(
            "[DRY-RUN] Would delete {} snapshot(s) up to {} (cutoff: {})",
            snapshots_removed, options.to, cutoff_dt
        );
        for name in &snapshot_names {
            info!("[DRY-RUN] Would delete snapshot '{}'", name);
        }
        for dir in &diff_dirs_to_purge {
            info!("[DRY-RUN] Would purge diffs/{}", dir);
        }
        return Ok(RemoveSnapshotsResult {
            to: options.to.clone(),
            to_timestamp: cutoff_dt,
            snapshots_scanned,
            snapshots_removed,
            snapshots: snapshot_names,
            diff_dirs_purged: diff_dirs_to_purge,
            snapshots_remaining,
            dry_run: true,
        });
    }

    // Step 7: Delete physical diff directories on remote
    for dir in &diff_dirs_to_purge {
        let clean_dir = dir.trim_matches('/');
        let diff_path = format!("{remote_clean}/diffs/{clean_dir}");
        info!("Deleting files in diff directory {}", diff_path);
        if let Err(e) = rclone.delete(&diff_path, &[]) {
            warn!("Failed to delete files in diff directory {}: {}", diff_path, e);
        }
        if let Err(e) = rclone.rmdir_specific(&diff_path) {
            warn!(
                "[WARN] Could not remove empty diff directory '{}' ({e}). Please manually remove it if needed: rclone rmdirs \"{}\"",
                diff_path, diff_path
            );
        }
    }

    // Step 8: Delete manifests of removed snapshots
    let manifests_dir = format!("{remote_clean}/.meta/manifests/");
    let mut names_to_remove = HashSet::new();
    for snap in &to_remove {
        let manifest_file_path = format!("{manifests_dir}{}", snap.manifest_name);
        debug!("Deleting manifest {}", manifest_file_path);
        rclone.delete_file(&manifest_file_path)?;
        names_to_remove.insert(snap.name.clone());
        names_to_remove.insert(snap.manifest_name.clone());
        names_to_remove.insert(snap.manifest_name.trim_end_matches(".json").to_string());
    }

    // Update remote snapshot catalog
    let purged_diff_set: HashSet<String> = diff_dirs_to_purge.iter().cloned().collect();
    let _ = crate::catalog::remove_from_catalog(rclone, remote_clean, &names_to_remove, &purged_diff_set);

    info!(
        "Successfully deleted {} snapshot(s) up to {}. Remaining snapshots: {}",
        snapshots_removed, options.to, snapshots_remaining
    );

    Ok(RemoveSnapshotsResult {
        to: options.to.clone(),
        to_timestamp: cutoff_dt,
        snapshots_scanned,
        snapshots_removed,
        snapshots: snapshot_names,
        diff_dirs_purged: diff_dirs_to_purge,
        snapshots_remaining,
        dry_run: false,
    })
}

// ─── Unit Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_target_basic() {
        assert_eq!(normalize_target("docs/secret.txt").unwrap(), "docs/secret.txt");
        assert_eq!(normalize_target("/docs/secret.txt").unwrap(), "docs/secret.txt");
        assert_eq!(normalize_target("./docs/secret.txt").unwrap(), "docs/secret.txt");
        assert_eq!(normalize_target("docs/secret/").unwrap(), "docs/secret");
        assert_eq!(normalize_target("./docs/secret/").unwrap(), "docs/secret");
    }

    #[test]
    fn test_normalize_target_root_rejected() {
        assert!(normalize_target("").is_err());
        assert!(normalize_target("/").is_err());
        assert!(normalize_target(".").is_err());
        assert!(normalize_target("./").is_err());
    }

    #[test]
    fn test_matches_target_exact_file() {
        assert!(matches_target("secret.txt", "secret.txt"));
        assert!(matches_target("docs/report.pdf", "docs/report.pdf"));
        assert!(!matches_target("other/secret.txt", "secret.txt"));
    }

    #[test]
    fn test_matches_target_directory_prefix() {
        assert!(matches_target("docs/secret/key.pem", "docs/secret"));
        assert!(matches_target("docs/secret/sub/pass.txt", "docs/secret"));
        assert!(matches_target("secret_dir/data.csv", "secret_dir"));
        // Boundary check: docs/secret_notes.txt should NOT match docs/secret
        assert!(!matches_target("docs/secret_notes.txt", "docs/secret"));
    }

    #[test]
    fn test_matches_target_wildcards() {
        assert!(matches_target("secret.key", "*.key"));
        assert!(matches_target("certs/private.key", "*.key"));
        assert!(matches_target("logs/app.log", "logs/*.log"));
        assert!(!matches_target("other/app.log", "logs/*.log"));
        assert!(!matches_target("certs/public.crt", "*.key"));
    }

    #[test]
    fn test_remove_snapshots_empty_target_fails() {
        let config = Config::default();
        let rclone = Rclone::new(&config);
        let options = RemoveSnapshotsOptions {
            to: "".to_string(),
            force_lock: false,
            dry_run: true,
            json: false,
        };
        assert!(run_remove_snapshots(&config, &rclone, &options).is_err());
    }
}