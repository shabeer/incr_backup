//! # GC Module — Garbage Collection for Cloud Backup Storage
//!
//! Reclaims unreferenced or stale remote data, such as orphaned diff directories
//! that have no corresponding snapshot manifests in `.meta/manifests/`.

use std::collections::HashSet;

use log::{debug, info, warn};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{IncrError, Result};
use crate::lock::LockManager;
use crate::manifest::Manifest;
use crate::rclone::Rclone;

/// Options controlling garbage collection (`gc`).
#[derive(Debug, Clone)]
pub struct GcOptions {
    /// Delete orphaned diff directories that have no corresponding snapshot manifests.
    pub orphaned_diffs: bool,

    /// If `true`, break an existing advisory lock if present.
    pub force_lock: bool,

    /// If `true`, simulate the garbage collection without deleting remote files.
    pub dry_run: bool,

    /// If `true`, produce output in JSON format.
    pub json: bool,
}

/// Metadata about an identified orphaned diff directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrphanedDiffInfo {
    /// Subdirectory name under `diffs/`.
    pub dir_name: String,

    /// Relative remote path (e.g. `diffs/snap2/`).
    pub path: String,

    /// Number of files inside this orphaned diff directory.
    pub file_count: u64,

    /// Total size in bytes of files in this orphaned diff directory.
    pub size_bytes: u64,
}

/// Summary result of a completed (or dry-run) garbage collection operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GcResult {
    /// Orphaned diff directories identified.
    pub orphaned_diffs: Vec<OrphanedDiffInfo>,

    /// Total number of orphaned diff directories found.
    pub total_orphaned_dirs: usize,

    /// Total files contained in orphaned diff directories.
    pub total_files: u64,

    /// Total bytes freed (or that would be freed in dry-run).
    pub total_bytes: u64,

    /// Whether this was a dry-run simulation.
    pub dry_run: bool,
}

/// Run garbage collection on the remote backup storage.
pub fn run_gc(config: &Config, rclone: &Rclone, options: &GcOptions) -> Result<GcResult> {
    if !options.orphaned_diffs {
        return Err(IncrError::InvalidConfig(
            "No garbage collection task specified. Use --orphaned-diffs to clean up orphaned diff directories.".to_string(),
        ));
    }

    if config.remote_path.is_empty() {
        return Err(IncrError::InvalidConfig(
            "Remote path is required for gc operation. Specify [remote].path in config or use --remote.".to_string(),
        ));
    }

    let remote_base = &config.remote_path;

    // Acquire advisory cloud lock
    let lock_mgr = LockManager::new(rclone, remote_base, config.timezone);
    if options.dry_run {
        info!("[DRY-RUN] Would acquire remote lock");
        if let Ok(Some(lock)) = lock_mgr.check() {
            if !crate::lock::is_stale(&lock) {
                warn!(
                    "Backup is currently locked by '{}' (since {}). A sync or compaction operation may be in progress.",
                    lock.holder,
                    config.timezone.format_message_datetime(lock.started_at)
                );
            }
        }
    } else {
        lock_mgr.acquire(options.force_lock)?;
        debug!("Remote lock acquired");
    }

    let result = run_gc_inner(config, rclone, options);

    if !options.dry_run {
        if let Err(e) = lock_mgr.release() {
            warn!("Failed to release remote lock: {e}");
        } else {
            debug!("Remote lock released");
        }
    }

    result
}

fn run_gc_inner(config: &Config, rclone: &Rclone, options: &GcOptions) -> Result<GcResult> {
    let remote_clean = config.remote_path.trim_end_matches('/');

    // 1. Scan existing diff directories in diffs/
    let diffs_base = format!("{remote_clean}/diffs");
    let existing_diff_dirs: Vec<String> = rclone.lsd(&diffs_base).unwrap_or_default();
    if existing_diff_dirs.is_empty() {
        return Ok(GcResult {
            orphaned_diffs: Vec::new(),
            total_orphaned_dirs: 0,
            total_files: 0,
            total_bytes: 0,
            dry_run: options.dry_run,
        });
    }

    // 2. Scan valid snapshot manifests in .meta/manifests/
    let manifests_dir = format!("{remote_clean}/.meta/manifests/");
    let manifest_files = rclone.lsf(&manifests_dir)?;
    let valid_manifest_files: Vec<String> = manifest_files
        .into_iter()
        .filter(|f| f.ends_with(".json") && f != "current.json")
        .collect();

    let valid_manifest_stems: HashSet<String> = valid_manifest_files
        .iter()
        .map(|f| f.trim_end_matches(".json").to_string())
        .collect();
    let valid_manifest_set: HashSet<String> = valid_manifest_files.iter().cloned().collect();

    // 3. Load snapshot catalog if available.
    // Retain only entries whose manifest actually exists on remote.
    let catalog_entries = crate::catalog::load_snapshot_catalog(rclone, remote_clean).unwrap_or_default();
    let mut stale_catalog_names: HashSet<String> = HashSet::new();
    let mut valid_catalog_entries = Vec::new();

    for entry in catalog_entries {
        if valid_manifest_set.contains(&entry.manifest_name) || valid_manifest_stems.contains(&entry.name) {
            valid_catalog_entries.push(entry);
        } else {
            stale_catalog_names.insert(entry.name);
            stale_catalog_names.insert(entry.manifest_name);
        }
    }

    // 4. Resolve all active snapshot names and associated diff directories
    let mut active_diff_dirs: HashSet<String> = HashSet::new();
    let mut active_prefixes: Vec<String> = Vec::new();

    for file in &valid_manifest_files {
        let stem = file.trim_end_matches(".json");
        active_diff_dirs.insert(stem.to_string());
        active_prefixes.push(format!("{stem}__"));

        let manifest_path = format!("{manifests_dir}{file}");
        if let Ok(content) = rclone.cat(&manifest_path) {
            if let Ok(manifest) = Manifest::from_json(&content) {
                if !manifest.snapshot_name.is_empty() {
                    active_diff_dirs.insert(manifest.snapshot_name.clone());
                    active_prefixes.push(format!("{}__", manifest.snapshot_name));
                }
            }
        }
    }

    for entry in &valid_catalog_entries {
        active_diff_dirs.insert(entry.name.clone());
        let stem = entry.manifest_name.trim_end_matches(".json");
        active_diff_dirs.insert(stem.to_string());
        active_prefixes.push(format!("{stem}__"));
        if !entry.name.is_empty() {
            active_prefixes.push(format!("{}__", entry.name));
        }
        if let Some(ref d) = entry.diff_dir {
            active_diff_dirs.insert(d.trim_matches('/').to_string());
        }
    }

    for stem in active_diff_dirs.clone() {
        if let Some(d) = crate::restore::find_snapshot_diff_dir(rclone, remote_clean, &stem) {
            active_diff_dirs.insert(d.trim_matches('/').to_string());
        }
    }

    // 5. Identify orphaned diff directories
    let mut orphaned_dir_names: Vec<String> = Vec::new();
    for dir in existing_diff_dirs {
        let clean = dir.trim_matches('/').to_string();
        if clean.is_empty() {
            continue;
        }

        // Check exact match
        if active_diff_dirs.contains(&clean) {
            continue;
        }

        // Check prefix match
        let is_prefix_match = active_prefixes.iter().any(|prefix| clean.starts_with(prefix));
        if is_prefix_match {
            continue;
        }

        orphaned_dir_names.push(clean);
    }

    orphaned_dir_names.sort();

    // 6. Gather file count and size stats for orphaned diff directories in parallel
    let orphaned_info: Vec<OrphanedDiffInfo> = orphaned_dir_names
        .par_iter()
        .map(|dir| {
            let diff_path = format!("{remote_clean}/diffs/{dir}/");
            let size = rclone
                .size(&diff_path)
                .unwrap_or(crate::rclone::SizeOutput { count: 0, bytes: 0 });
            OrphanedDiffInfo {
                dir_name: dir.clone(),
                path: format!("diffs/{dir}/"),
                file_count: size.count,
                size_bytes: size.bytes,
            }
        })
        .collect();

    let total_orphaned_dirs = orphaned_info.len();
    let total_files: u64 = orphaned_info.iter().map(|i| i.file_count).sum();
    let total_bytes: u64 = orphaned_info.iter().map(|i| i.size_bytes).sum();

    // 7. If dry-run, return planned results
    if options.dry_run {
        info!(
            "[DRY-RUN] Found {} orphaned diff directory/ies ({} files, {} bytes)",
            total_orphaned_dirs, total_files, total_bytes
        );
        for item in &orphaned_info {
            info!("[DRY-RUN] Would purge orphaned {}", item.path);
        }
        return Ok(GcResult {
            orphaned_diffs: orphaned_info,
            total_orphaned_dirs,
            total_files,
            total_bytes,
            dry_run: true,
        });
    }

    // 8. Delete orphaned diff directories on remote
    for item in &orphaned_info {
        let diff_path = format!("{remote_clean}/diffs/{}", item.dir_name);
        info!("Deleting files in orphaned diff directory {}", diff_path);
        if let Err(e) = rclone.delete(&diff_path, &[]) {
            warn!("Failed to delete files in orphaned diff directory {}: {}", diff_path, e);
        }
        if let Err(e) = rclone.rmdir_specific(&diff_path) {
            warn!(
                "[WARN] Could not remove empty diff directory '{}' ({e}). Please manually remove it if needed: rclone rmdirs \"{}\"",
                diff_path, diff_path
            );
        }
    }

    // 9. Update remote snapshot catalog to clear purged diff directories and remove stale entries
    let purged_diff_set: HashSet<String> = orphaned_info.iter().map(|i| i.dir_name.clone()).collect();
    let _ = crate::catalog::remove_from_catalog(rclone, remote_clean, &stale_catalog_names, &purged_diff_set);

    Ok(GcResult {
        orphaned_diffs: orphaned_info,
        total_orphaned_dirs,
        total_files,
        total_bytes,
        dry_run: false,
    })
}
