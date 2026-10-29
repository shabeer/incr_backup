//! # Sync Pipeline — End-to-End Reverse-Incremental Backup
//!
//! Under Dynamic Snapshot Consolidation, every sync operation creates an append-only,
//! discrete snapshot with its own manifest and timestamped diff directory. There are
//! no tiers, rotation slots, slot overflow, or in-place overwrites. Compaction and
//! pruning are decoupled from sync and performed safely by `compact`.
//!
//! ## Steps in a Sync Run
//! 1. Check for an interrupted journal → resume if present
//! 2. Acquire remote exclusive lock (`.meta/lock`)
//! 3. Generate snapshot name (timestamped or user-provided)
//! 4. Write write-ahead journal (`.meta/journal.json`)
//! 5. `rclone sync <source> <remote>:current/ --backup-dir <remote>:diffs/<snapshot>/`
//! 6. Scan `<remote>:current/` via `rclone lsjson` to build complete file manifest
//! 7. Upload manifest to `<remote>:.meta/manifests/<snapshot>.json` and `current.json`
//! 8. Clean up journal and release lock

use std::time::{Duration, Instant};

use chrono::Utc;
use log::{debug, info, warn};

use crate::config::Config;
use crate::error::{IncrError, Result};
use crate::journal::{JournalManager, StepStatus};
use crate::lock::LockManager;
use crate::manifest::Manifest;
use crate::rclone::Rclone;

// ─── Options & Result Structs ────────────────────────────────────────────────

/// Options controlling how a sync operation behaves.
#[derive(Debug, Clone)]
pub struct SyncOptions {
    /// Optional custom snapshot name (defaults to formatted timestamp).
    pub name: Option<String>,

    /// If `true`, forcefully break any existing lock before proceeding.
    pub force_lock: bool,

    /// If `true`, log every step without executing any mutating operations.
    pub dry_run: bool,
}

/// Summary of a completed (or dry-run) sync operation.
#[derive(Debug, Clone)]
pub struct SyncResult {
    /// Name of the snapshot that was created (e.g. `"2026-10-29T04-28Z"`).
    pub snapshot_name: String,

    /// Number of files in the snapshot manifest.
    pub files_changed: usize,

    /// Total bytes in the snapshot.
    pub bytes_transferred: u64,

    /// Wall-clock duration of the entire pipeline.
    pub duration: Duration,
}

// ─── Pipeline ────────────────────────────────────────────────────────────────

/// The main sync pipeline that executes reverse-incremental syncs.
pub struct SyncPipeline<'a> {
    config: &'a Config,
    rclone: &'a Rclone,
    lock_mgr: LockManager<'a>,
    journal_mgr: JournalManager<'a>,
}

impl<'a> SyncPipeline<'a> {
    /// Create a new pipeline from a [`Config`] and [`Rclone`] wrapper.
    pub fn new(config: &'a Config, rclone: &'a Rclone) -> Self {
        let lock_mgr = LockManager::new(rclone, &config.remote_path, config.timezone);
        let journal_mgr = JournalManager::new(rclone, &config.remote_path);

        Self {
            config,
            rclone,
            lock_mgr,
            journal_mgr,
        }
    }

    /// Execute the full sync pipeline for the given options.
    pub fn run(&self, options: &SyncOptions) -> Result<SyncResult> {
        let start = Instant::now();
        info!(
            "Starting sync pipeline: name={:?}, dry_run={}",
            options.name, options.dry_run
        );

        // Step 0: Check for existing journal (resume interrupted run)
        if let Some(journal) = self.journal_mgr.load_journal()? {
            warn!(
                "Found incomplete journal from {}. Resuming interrupted operation.",
                self.config.timezone.format_message_datetime(journal.started_at)
            );
            return self.resume_from_journal(&journal, options);
        }

        // Step 1: Acquire lock
        if options.dry_run {
            info!("[DRY-RUN] Would acquire lock on remote");
        } else {
            self.lock_mgr.acquire(options.force_lock)?;
            debug!("Lock acquired");
        }

        let result = self.run_inner(options, start);

        // Release lock
        if !options.dry_run
            && let Err(e) = self.lock_mgr.release()
        {
            warn!("Failed to release lock: {e}");
        }

        result
    }

    /// Inner pipeline logic, wrapped in lock acquire/release.
    fn run_inner(&self, options: &SyncOptions, start: Instant) -> Result<SyncResult> {
        let now = Utc::now();
        let snapshot_name = match &options.name {
            Some(n) => n.clone(),
            None => self.config.timezone.format_datetime(now),
        };

        let backup_dir = format!("diffs/{snapshot_name}");
        let remote_base = &self.config.remote_path;
        let remote_current = format!("{remote_base}/current/");
        let remote_backup_dir = format!("{remote_base}/{backup_dir}");

        // Create journal
        if !options.dry_run {
            self.journal_mgr.create_journal(
                "sync",
                &snapshot_name,
                &backup_dir,
            )?;
        }

        // rclone sync
        if options.dry_run {
            info!(
                "[DRY-RUN] Would run: rclone sync {} {} --backup-dir {}",
                self.config.source_path,
                remote_current,
                remote_backup_dir
            );
        } else {
            let _ = self.rclone.mkdir(&remote_backup_dir);

            self.journal_mgr
                .update_step("rclone_sync", StepStatus::Running)?;

            self.rclone.sync(
                &self.config.source_path,
                &remote_current,
                Some(&remote_backup_dir),
                &self.config.exclude.patterns,
            )?;

            self.journal_mgr
                .update_step("rclone_sync", StepStatus::Done)?;
        }

        // Generate manifest
        let manifest = if options.dry_run {
            info!("[DRY-RUN] Would scan remote current/ to generate manifest");
            Manifest::empty()
        } else {
            self.journal_mgr
                .update_step("generate_manifest", StepStatus::Running)?;

            let lsjson_output = self.rclone.lsjson_recursive_hash(&remote_current)?;
            let manifest = Manifest::from_rclone_lsjson(&lsjson_output, &snapshot_name, &now)?;

            self.journal_mgr
                .update_step("generate_manifest", StepStatus::Done)?;

            manifest
        };

        // Upload manifest
        let manifest_remote_path = format!("{remote_base}/.meta/manifests/{snapshot_name}.json");

        if options.dry_run {
            info!("[DRY-RUN] Would upload manifest to {manifest_remote_path}");
        } else {
            self.journal_mgr
                .update_step("upload_manifest", StepStatus::Running)?;

            let manifest_json = serde_json::to_string_pretty(&manifest)?;
            self.rclone.rcat(&manifest_remote_path, &manifest_json)?;

            let current_manifest_path = format!("{remote_base}/.meta/manifests/current.json");
            self.rclone.rcat(&current_manifest_path, &manifest_json)?;

            self.journal_mgr
                .update_step("upload_manifest", StepStatus::Done)?;
        }

        // Clean up journal
        if !options.dry_run {
            self.journal_mgr.cleanup()?;
            debug!("Journal cleaned up");
        }

        let duration = start.elapsed();
        let files_changed = manifest.files.len();

        let result = SyncResult {
            snapshot_name,
            files_changed,
            bytes_transferred: manifest.total_size(),
            duration,
        };

        info!(
            "Sync complete: {} ({} files, {} bytes, {:.1}s)",
            result.snapshot_name,
            result.files_changed,
            result.bytes_transferred,
            result.duration.as_secs_f64(),
        );

        Ok(result)
    }

    /// Resume a previously interrupted sync operation from its journal.
    fn resume_from_journal(
        &self,
        journal: &crate::journal::Journal,
        options: &SyncOptions,
    ) -> Result<SyncResult> {
        let start = Instant::now();
        info!(
            "Resuming {} operation from journal (started {})",
            journal.operation,
            self.config.timezone.format_message_datetime(journal.started_at)
        );

        if !options.dry_run {
            self.lock_mgr.acquire(true)?;
        }

        let backup_dir = &journal.backup_dir;
        let remote_base = &self.config.remote_path;
        let remote_current = format!("{remote_base}/current/");

        for step in &journal.steps {
            match step.status {
                StepStatus::Done => {
                    debug!("Step '{}' already done, skipping", step.name);
                    continue;
                }
                StepStatus::Running | StepStatus::Pending | StepStatus::Failed(_) => {
                    info!("Resuming step: '{}' (was {:?})", step.name, step.status);
                }
            }

            match step.name.as_str() {
                "acquire_lock" => {
                    if !options.dry_run {
                        self.journal_mgr
                            .update_step("acquire_lock", StepStatus::Done)?;
                    }
                }
                "rclone_sync" => {
                    let remote_backup_dir = format!("{remote_base}/{backup_dir}");
                    if options.dry_run {
                        info!("[DRY-RUN] Would resume rclone sync");
                    } else {
                        self.rclone.sync(
                            &self.config.source_path,
                            &remote_current,
                            Some(&remote_backup_dir),
                            &self.config.exclude.patterns,
                        )?;
                        self.journal_mgr
                            .update_step("rclone_sync", StepStatus::Done)?;
                    }
                }
                "generate_manifest" => {
                    if options.dry_run {
                        info!("[DRY-RUN] Would regenerate manifest");
                    } else {
                        let lsjson_output =
                            self.rclone.lsjson_recursive_hash(&remote_current)?;
                        let now = Utc::now();
                        let snapshot_name = backup_dir
                            .strip_prefix("diffs/")
                            .unwrap_or(backup_dir);
                        let _manifest = Manifest::from_rclone_lsjson(
                            &lsjson_output,
                            snapshot_name,
                            &now,
                        )?;
                        self.journal_mgr
                            .update_step("generate_manifest", StepStatus::Done)?;
                    }
                }
                "upload_manifest" => {
                    if options.dry_run {
                        info!("[DRY-RUN] Would re-upload manifest");
                    } else {
                        let lsjson_output =
                            self.rclone.lsjson_recursive_hash(&remote_current)?;
                        let now = Utc::now();
                        let snapshot_name = backup_dir
                            .strip_prefix("diffs/")
                            .unwrap_or(backup_dir);
                        let manifest = Manifest::from_rclone_lsjson(
                            &lsjson_output,
                            snapshot_name,
                            &now,
                        )?;
                        let manifest_json = serde_json::to_string_pretty(&manifest)?;

                        let manifest_path = format!(
                            "{remote_base}/.meta/manifests/{snapshot_name}.json"
                        );
                        self.rclone.rcat(&manifest_path, &manifest_json)?;

                        let current_manifest_path =
                            format!("{remote_base}/.meta/manifests/current.json");
                        self.rclone.rcat(&current_manifest_path, &manifest_json)?;

                        self.journal_mgr
                            .update_step("upload_manifest", StepStatus::Done)?;
                    }
                }
                "cleanup_journal" => {
                    if !options.dry_run {
                        self.journal_mgr.cleanup()?;
                    }
                }
                "release_lock" => {
                    if !options.dry_run {
                        let _ = self.lock_mgr.release();
                    }
                }
                other => {
                    return Err(IncrError::CorruptJournal(format!(
                        "unexpected step '{other}' in journal"
                    )));
                }
            }
        }

        if !options.dry_run
            && let Err(e) = self.lock_mgr.release()
        {
            warn!("Failed to release lock during resume: {e}");
        }

        let snapshot_name = backup_dir
            .strip_prefix("diffs/")
            .unwrap_or(backup_dir)
            .to_string();

        Ok(SyncResult {
            snapshot_name,
            files_changed: 0,
            bytes_transferred: 0,
            duration: start.elapsed(),
        })
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_options_defaults() {
        let opts = SyncOptions {
            name: None,
            force_lock: false,
            dry_run: true,
        };
        assert!(opts.name.is_none());
        assert!(opts.dry_run);
    }

    #[test]
    fn test_sync_result_fields() {
        let result = SyncResult {
            snapshot_name: "2026-10-29T04-28Z".to_string(),
            files_changed: 42,
            bytes_transferred: 1_048_576,
            duration: Duration::from_secs(30),
        };
        assert_eq!(result.snapshot_name, "2026-10-29T04-28Z");
        assert_eq!(result.files_changed, 42);
        assert_eq!(result.bytes_transferred, 1_048_576);
    }
}
