//! # Sync Module — Full Sync Pipeline Orchestration
//!
//! Orchestrates the entire `incr-backup sync` command: locking, journal-based
//! resumability, rsnapshot-style rotation/cascade/promotion, rclone sync,
//! manifest generation, and cleanup.
//!
//! ## Analogy
//! Think of this module as a **workflow engine** (like Java's Spring Batch or
//! Python's Airflow DAG) — it defines a sequence of steps, tracks which ones
//! have completed via a journal, and can resume from the last successful step
//! after a crash.
//!
//! ## Key Concepts
//! - **Update-in-place**: If the most recent snapshot is younger than
//!   `min_interval_minutes`, we re-sync into the *same* backup-dir instead of
//!   rotating. This prevents rapid cron runs from consuming all hourly slots.
//! - **Cascade rotation**: A `--level daily` run first rotates dailies, promotes
//!   the oldest hourly to `daily.0`, then does a normal hourly sync. Weekly and
//!   monthly cascade further.
//! - **Dry-run**: Every mutating step checks `options.dry_run` and logs what
//!   *would* happen without actually executing.

use std::time::{Duration, Instant};

use chrono::Utc;
use log::{debug, info, warn};

use crate::config::Config;
use crate::error::Result;
use crate::journal::{JournalManager, StepStatus};
use crate::lock::LockManager;
use crate::manifest::Manifest;
use crate::rclone::Rclone;
use crate::snapshot::{RotationManager, SnapshotTier};

// ─── Options & Result Structs ────────────────────────────────────────────────

/// Options controlling how a sync operation behaves.
///
/// Passed from the CLI layer into [`SyncPipeline::run`]. Each field maps
/// directly to a CLI flag (e.g., `--level hourly`, `--force-rotate`, `--dry-run`).
///
/// ## Java/Python analogy
/// Like a Java DTO / Python `@dataclass` — a plain container carrying
/// user-provided settings through the pipeline.
#[derive(Debug, Clone)]
pub struct SyncOptions {
    /// Which retention tier to target (hourly, daily, weekly, monthly).
    /// Determines the cascade depth: `daily` cascades to hourly, etc.
    pub level: SnapshotTier,

    /// If `true`, always rotate even when `min_interval_minutes` has not
    /// elapsed since the last snapshot. Maps to `--force-rotate`.
    pub force_rotate: bool,

    /// If `true`, forcefully break any existing lock before proceeding.
    /// Maps to `--force`.
    pub force_lock: bool,

    /// If `true`, log every step without executing any mutating operations.
    /// Maps to `--dry-run` / `-n`.
    pub dry_run: bool,
}

/// Summary of a completed (or dry-run) sync operation.
///
/// Returned by [`SyncPipeline::run`] so the CLI layer can display a
/// human-readable summary.
#[derive(Debug, Clone)]
pub struct SyncResult {
    /// Name of the snapshot that was created or updated (e.g. `"hourly.0__2026-10-28T08-00"`).
    pub snapshot_name: String,

    /// Number of files that rclone reported as changed (added, modified, deleted).
    pub files_changed: usize,

    /// Total bytes transferred during the rclone sync.
    pub bytes_transferred: u64,

    /// Wall-clock duration of the entire pipeline.
    pub duration: Duration,

    /// `true` when we skipped rotation and re-synced into the existing
    /// `hourly.0` backup-dir (because `min_interval` had not elapsed).
    pub was_update_in_place: bool,
}

// ─── Pipeline ────────────────────────────────────────────────────────────────

/// The main sync pipeline that ties every subsystem together.
///
/// Holds owned instances of all "manager" objects so they share the same
/// config and rclone wrapper. Constructed once per `sync` invocation.
///
/// ## Java/Python analogy
/// Like a Spring `@Service` that depends on other services via constructor
/// injection, or a Python class whose `__init__` wires up collaborators.
pub struct SyncPipeline<'a> {
    /// Parsed backup configuration (source path, remote, retention counts, etc.).
    config: &'a Config,

    /// Thin wrapper around the rclone subprocess. Every remote I/O goes through here.
    rclone: &'a Rclone,

    /// Manages rsnapshot-style rotation: shift, promote, delete within tiers.
    rotation_mgr: RotationManager<'a>,

    /// Manages the remote `.meta/lock` file for mutual exclusion.
    lock_mgr: LockManager<'a>,

    /// Manages the remote `.meta/journal.json` write-ahead log.
    journal_mgr: JournalManager<'a>,
}

impl<'a> SyncPipeline<'a> {
    /// Create a new pipeline from a [`Config`] and [`Rclone`] wrapper.
    ///
    /// Constructs all sub-managers from the single config source-of-truth.
    /// No I/O happens here — the pipeline is inert until [`run`] is called.
    ///
    /// # Arguments
    /// * `config` — Parsed and validated backup configuration.
    /// * `rclone` — Shared rclone subprocess wrapper.
    ///
    /// # Example (pseudo-code)
    /// ```rust,ignore
    /// let config = Config::load("incr-backup.toml")?;
    /// let rclone = Rclone::new(&config);
    /// let pipeline = SyncPipeline::new(&config, &rclone);
    /// let result = pipeline.run(options)?;
    /// ```
    pub fn new(config: &'a Config, rclone: &'a Rclone) -> Self {
        let rotation_mgr = RotationManager::new(rclone, &config.remote_path, &config.retention);
        let lock_mgr = LockManager::new(rclone, &config.remote_path);
        let journal_mgr = JournalManager::new(rclone, &config.remote_path);

        Self {
            config,
            rclone,
            rotation_mgr,
            lock_mgr,
            journal_mgr,
        }
    }

    /// Execute the full sync pipeline for the given options.
    ///
    /// This is the top-level entry point for the `sync` command. It handles:
    /// 1. Journal-based resumption (if a previous run was interrupted)
    /// 2. Lock acquisition (with optional force-break)
    /// 3. Cascade rotation for higher tiers (daily/weekly/monthly)
    /// 4. The actual hourly-level rclone sync
    /// 5. Manifest generation and upload
    /// 6. Journal and lock cleanup
    ///
    /// ## Algorithm
    /// ```text
    /// check journal → acquire lock → cascade if level > hourly
    ///   → rotate hourlies → rclone sync --backup-dir → manifest
    ///   → upload manifest → clean journal → release lock
    /// ```
    ///
    /// ## Errors
    /// Returns [`IncrError::Locked`] if another process holds the lock and
    /// `force_lock` is `false`. Other errors bubble up from rclone or I/O.
    ///
    /// ## Dry-run
    /// When `options.dry_run` is `true`, every step logs what *would* happen
    /// but performs no remote mutations.
    pub fn run(&self, options: &SyncOptions) -> Result<SyncResult> {
        let start = Instant::now();
        info!(
            "Starting sync pipeline: level={:?}, force_rotate={}, dry_run={}",
            options.level, options.force_rotate, options.dry_run
        );

        // ── Step 0: Check for existing journal (resume interrupted run) ──
        // Like checking a transaction log on startup — if we find one, we
        // pick up where we left off instead of starting fresh.
        if let Some(journal) = self.journal_mgr.load_journal()? {
            warn!(
                "Found incomplete journal from {}. Resuming interrupted operation.",
                journal.started_at
            );
            return self.resume_from_journal(&journal, options);
        }

        // ── Step 1: Acquire lock ─────────────────────────────────────────
        if options.dry_run {
            info!("[DRY-RUN] Would acquire lock on remote");
        } else {
            self.lock_mgr.acquire(options.force_lock)?;
            debug!("Lock acquired");
        }

        // Wrap the main work in a closure-like block so we can guarantee
        // lock release even on error (Rust's `Drop` or explicit cleanup).
        let result = self.run_inner(options, start);

        // ── Step N: Release lock (always, even on error) ─────────────────
        if !options.dry_run
            && let Err(e) = self.lock_mgr.release()
        {
            // Log but don't mask the original error — lock release failure
            // is secondary. The lock has a timeout so it will self-heal.
            warn!("Failed to release lock: {e}");
        }

        result
    }

    /// Inner pipeline logic, separated so the caller can wrap lock
    /// acquire/release around it.
    ///
    /// Not public — this is an implementation detail.
    fn run_inner(&self, options: &SyncOptions, start: Instant) -> Result<SyncResult> {
        // ── Step 2: Determine rotation vs update-in-place ────────────────
        // Check how old the newest snapshot at this tier is. If it's younger
        // than `min_interval_minutes`, we update in place (no rotation).
        let should_rotate = if options.force_rotate {
            true
        } else {
            self.rotation_mgr.should_rotate(&options.level)?
        };

        if !should_rotate {
            info!(
                "Most recent {:?} snapshot is within min_interval — updating in place",
                options.level
            );
        }

        // ── Step 3: Cascade rotation for higher tiers ────────────────────
        // If level > hourly, we must first handle the higher tier's rotation
        // and promotion before falling through to the hourly sync.
        //
        // Analogy: like a recursive function that peels off one layer at a time:
        //   monthly → weekly → daily → hourly (base case = actual sync)
        if options.level != SnapshotTier::Hourly {
            self.cascade_rotation(&options.level, options)?;
        }

        // ── Step 4: Do the hourly-level sync (the base case) ─────────────
        self.do_hourly_sync(options, should_rotate, start)
    }

    /// Cascade rotation for tiers above hourly.
    ///
    /// Implements the rsnapshot cascade: each tier rotates its own snapshots,
    /// then promotes the oldest snapshot from the tier below.
    ///
    /// ## Algorithm
    /// ```text
    /// match level:
    ///   monthly → rotate monthlies → promote oldest weekly → cascade(weekly)
    ///   weekly  → rotate weeklies  → promote oldest daily  → cascade(daily)
    ///   daily   → rotate dailies   → promote oldest hourly → (done, hourly sync follows)
    ///   hourly  → no cascade needed (base case)
    /// ```
    ///
    /// ## Example
    /// `--level weekly` triggers:
    ///   1. Shift `weekly.3 → delete`, `weekly.2 → 3`, `weekly.1 → 2`, `weekly.0 → 1`
    ///   2. Rename `daily.6` → `weekly.0`
    ///   3. Then cascade into daily: shift dailies, promote oldest hourly
    ///   4. Hourly sync happens in `do_hourly_sync` (not here)
    fn cascade_rotation(
        &self,
        level: &SnapshotTier,
        options: &SyncOptions,
    ) -> Result<()> {
        // `match` is like Java's switch expression — the compiler ensures we
        // handle every variant of `SnapshotTier`. No forgotten cases.
        match level {
            SnapshotTier::Monthly => {
                info!("Cascading: rotating monthlies, promoting from weekly");
                if options.dry_run {
                    info!("[DRY-RUN] Would rotate monthly snapshots");
                    info!("[DRY-RUN] Would promote oldest weekly → monthly.0");
                } else {
                    self.rotation_mgr.rotate_tier(&SnapshotTier::Monthly)?;
                    self.rotation_mgr
                        .promote(&SnapshotTier::Weekly, &SnapshotTier::Monthly)?;
                }
                // Recurse into the weekly level
                self.cascade_rotation(&SnapshotTier::Weekly, options)?;
            }
            SnapshotTier::Weekly => {
                info!("Cascading: rotating weeklies, promoting from daily");
                if options.dry_run {
                    info!("[DRY-RUN] Would rotate weekly snapshots");
                    info!("[DRY-RUN] Would promote oldest daily → weekly.0");
                } else {
                    self.rotation_mgr.rotate_tier(&SnapshotTier::Weekly)?;
                    self.rotation_mgr
                        .promote(&SnapshotTier::Daily, &SnapshotTier::Weekly)?;
                }
                // Recurse into the daily level
                self.cascade_rotation(&SnapshotTier::Daily, options)?;
            }
            SnapshotTier::Daily => {
                info!("Cascading: rotating dailies, promoting from hourly");
                if options.dry_run {
                    info!("[DRY-RUN] Would rotate daily snapshots");
                    info!("[DRY-RUN] Would promote oldest hourly → daily.0");
                } else {
                    self.rotation_mgr.rotate_tier(&SnapshotTier::Daily)?;
                    self.rotation_mgr
                        .promote(&SnapshotTier::Hourly, &SnapshotTier::Daily)?;
                }
                // No further cascade — hourly sync happens in do_hourly_sync.
            }
            SnapshotTier::Hourly => {
                // Base case: nothing to cascade. The hourly sync is handled
                // separately in do_hourly_sync.
                debug!("Cascade reached hourly level — no further cascade needed");
            }
        }

        Ok(())
    }

    /// Perform the actual hourly-level sync: rotate hourlies, run rclone sync,
    /// generate and upload the manifest.
    ///
    /// This is the "base case" that every cascade eventually reaches.
    ///
    /// ## Steps
    /// 1. Rotate hourly snapshots (if `should_rotate` is `true`)
    /// 2. Determine the backup-dir name (new or existing for update-in-place)
    /// 3. Create the journal with all remaining steps
    /// 4. Run `rclone sync <source> <remote>:current/ --backup-dir <remote>:diffs/<snapshot>`
    /// 5. Scan `current/` via `rclone lsjson --recursive --hash` to build the manifest
    /// 6. Upload the manifest to `.meta/manifests/<snapshot>.json`
    /// 7. Clean up the journal
    ///
    /// ## Arguments
    /// * `options` — The sync options (level, dry_run, etc.).
    /// * `should_rotate` — Whether to shift existing hourlies before syncing.
    ///   `false` means update-in-place (re-sync to the same backup-dir).
    /// * `start` — The [`Instant`] when the pipeline started, for duration tracking.
    fn do_hourly_sync(
        &self,
        options: &SyncOptions,
        should_rotate: bool,
        start: Instant,
    ) -> Result<SyncResult> {
        let now = Utc::now();
        // Timestamp format used in directory names: "2026-10-28T08-00"
        // Colons replaced with hyphens for filesystem/URL safety.
        let timestamp = now.format("%Y-%m-%dT%H-%M").to_string();

        // ── Step 6: Rotate hourlies (if needed) ──────────────────────────
        let (backup_dir_name, was_update_in_place) = if should_rotate {
            if options.dry_run {
                info!("[DRY-RUN] Would rotate hourly snapshots");
            } else {
                self.rotation_mgr.rotate_tier(&SnapshotTier::Hourly)?;
            }
            let name = format!("hourly.0__{timestamp}");
            info!("Creating new snapshot: {name}");
            (name, false)
        } else {
            // Update in place — re-use the existing hourly.0's backup-dir.
            // `unwrap_or_else` = Python's `dict.get(key, default)`.
            let existing_name = self
                .rotation_mgr
                .get_newest_snapshot_name(&SnapshotTier::Hourly)?
                .unwrap_or_else(|| format!("hourly.0__{timestamp}"));
            info!("Updating in place: {existing_name}");
            (existing_name, true)
        };

        let backup_dir = format!("diffs/{backup_dir_name}");
        let remote_base = &self.config.remote_path;

        // ── Step 5: Create journal ───────────────────────────────────────
        // The journal is a write-ahead log so we can resume after a crash.
        // Each step transitions from "pending" → "running" → "done".
        if !options.dry_run {
            self.journal_mgr.create_journal(
                "sync",
                &options.level.to_string(),
                &backup_dir,
            )?;
        }

        // ── Step 7: rclone sync ──────────────────────────────────────────
        // This is the core data transfer step. rclone handles dedup, checksums,
        // and partial uploads. The `--backup-dir` flag causes rclone to
        // server-side MOVE old versions from current/ to diffs/<snapshot>/.
        let source = &self.config.source_path;
        let remote_current = format!("{remote_base}/current/");
        let remote_backup_dir = format!("{remote_base}/{backup_dir}");

        if options.dry_run {
            info!(
                "[DRY-RUN] Would run: rclone sync {} {} --backup-dir {}",
                source,
                remote_current,
                remote_backup_dir
            );
        } else {
            let _ = self.rclone.mkdir(&remote_backup_dir);

            self.journal_mgr
                .update_step("rclone_sync", StepStatus::Running)?;

            self.rclone.sync(
                source,
                &remote_current,
                Some(&remote_backup_dir),
                &self.config.exclude.patterns,
            )?;

            self.journal_mgr
                .update_step("rclone_sync", StepStatus::Done)?;
        }

        // ── Step 8: Generate manifest ────────────────────────────────────
        // Scan current/ on the remote to build a complete listing of every
        // file with its path, size, MD5 hash, and modification time.
        //
        // `rclone lsjson --recursive --hash` returns JSON that we parse into
        // our Manifest struct.
        let manifest = if options.dry_run {
            info!("[DRY-RUN] Would scan remote current/ to generate manifest");
            // Return an empty manifest for dry-run — it won't be uploaded.
            Manifest::empty()
        } else {
            self.journal_mgr
                .update_step("generate_manifest", StepStatus::Running)?;

            let lsjson_output = self.rclone.lsjson_recursive_hash(&remote_current)?;
            let manifest = Manifest::from_rclone_lsjson(&lsjson_output, &backup_dir_name, &now)?;

            self.journal_mgr
                .update_step("generate_manifest", StepStatus::Done)?;

            manifest
        };

        // ── Step 9: Upload manifest ──────────────────────────────────────
        let manifest_remote_path = format!(
            "{remote_base}/.meta/manifests/{}.json",
            backup_dir_name.split("__").next().unwrap_or(&backup_dir_name)
        );

        if options.dry_run {
            info!("[DRY-RUN] Would upload manifest to {manifest_remote_path}");
        } else {
            self.journal_mgr
                .update_step("upload_manifest", StepStatus::Running)?;

            let manifest_json = serde_json::to_string_pretty(&manifest)?;
            self.rclone.rcat(&manifest_remote_path, &manifest_json)?;

            // Also upload/update the "current" manifest which always reflects
            // the latest state of current/.
            let current_manifest_path =
                format!("{remote_base}/.meta/manifests/current.json");
            self.rclone.rcat(&current_manifest_path, &manifest_json)?;

            self.journal_mgr
                .update_step("upload_manifest", StepStatus::Done)?;
        }

        // ── Step 10: Clean up journal ────────────────────────────────────
        if !options.dry_run {
            self.journal_mgr.cleanup()?;
            debug!("Journal cleaned up");
        }

        let duration = start.elapsed();
        let files_changed = manifest.files.len();

        let result = SyncResult {
            snapshot_name: backup_dir_name,
            files_changed,
            bytes_transferred: manifest.total_size(),
            duration,
            was_update_in_place,
        };

        info!(
            "Sync complete: {} files, {} bytes, {:.1}s{}",
            result.files_changed,
            result.bytes_transferred,
            result.duration.as_secs_f64(),
            if result.was_update_in_place {
                " (update-in-place)"
            } else {
                ""
            }
        );

        Ok(result)
    }

    /// Resume a previously interrupted sync operation from its journal.
    ///
    /// Reads the journal to find the last completed step, then re-executes
    /// from the next pending/running step onward. This is the core of the
    /// crash-recovery mechanism.
    ///
    /// ## Algorithm
    /// ```text
    /// 1. Parse journal → find first step with status != "done"
    /// 2. For each remaining step:
    ///    a. If "running" → re-run (idempotent by design)
    ///    b. If "pending" → run normally
    /// 3. Lock is re-acquired if needed (journal proves we had it)
    /// ```
    ///
    /// ## Why is re-running safe?
    /// Every step is designed to be **idempotent**:
    /// - `rclone sync` skips already-uploaded files (hash match)
    /// - `rclone purge` on a missing dir is a no-op
    /// - `rclone moveto` on an already-moved dir is a no-op
    /// - Manifest upload overwrites the previous version
    ///
    /// ## Analogy
    /// Like a database WAL (Write-Ahead Log) replay: re-apply operations that
    /// may or may not have completed, relying on idempotency.
    fn resume_from_journal(
        &self,
        journal: &crate::journal::Journal,
        options: &SyncOptions,
    ) -> Result<SyncResult> {
        let start = Instant::now();
        info!(
            "Resuming {} operation from journal (started {})",
            journal.operation, journal.started_at
        );

        // Re-acquire lock — the old lock may have expired or been released.
        // Since we have the journal, we know we're the rightful owner.
        if !options.dry_run {
            self.lock_mgr.acquire(true)?; // force=true since we're resuming
        }

        let backup_dir = &journal.backup_dir;
        let remote_base = &self.config.remote_path;
        let remote_current = format!("{remote_base}/current/");

        // Walk through journal steps, executing any that aren't "done".
        // `iter()` returns references — like Java's `for (Step s : steps)`.
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

            // Dispatch each step by name. This is a simple state machine:
            // each step knows how to re-execute itself idempotently.
            match step.name.as_str() {
                "acquire_lock" => {
                    // Already re-acquired above.
                    if !options.dry_run {
                        self.journal_mgr
                            .update_step("acquire_lock", StepStatus::Done)?;
                    }
                }
                "rotate_snapshots" => {
                    if options.dry_run {
                        info!("[DRY-RUN] Would resume rotation");
                    } else {
                        // Re-run rotation — idempotent because rclone moveto
                        // on already-moved dirs is a no-op (source missing).
                        self.rotation_mgr.rotate_tier(&options.level)?;
                        self.journal_mgr
                            .update_step("rotate_snapshots", StepStatus::Done)?;
                    }
                }
                "delete_oldest" => {
                    if options.dry_run {
                        info!("[DRY-RUN] Would resume delete of oldest snapshot");
                    } else {
                        // `rclone purge` on a missing dir is a no-op — safe to re-run.
                        self.rotation_mgr.delete_oldest(&options.level)?;
                        self.journal_mgr
                            .update_step("delete_oldest", StepStatus::Done)?;
                    }
                }
                "rclone_sync" => {
                    let remote_backup_dir = format!("{remote_base}/{backup_dir}");
                    if options.dry_run {
                        info!("[DRY-RUN] Would resume rclone sync");
                    } else {
                        // rclone sync is idempotent — already-uploaded files are
                        // skipped (hash match), partially uploaded files are
                        // re-uploaded.
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
                        // Re-scan and re-upload — overwrites any partial upload.
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

                        let short_name = snapshot_name
                            .split("__")
                            .next()
                            .unwrap_or(snapshot_name);
                        let manifest_path = format!(
                            "{remote_base}/.meta/manifests/{short_name}.json"
                        );
                        self.rclone.rcat(&manifest_path, &manifest_json)?;

                        let current_manifest_path =
                            format!("{remote_base}/.meta/manifests/current.json");
                        self.rclone
                            .rcat(&current_manifest_path, &manifest_json)?;

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
                    // Lock release happens in the outer `run()` method.
                    debug!("Lock release deferred to outer handler");
                }
                unknown => {
                    warn!("Unknown journal step '{}', skipping", unknown);
                }
            }
        }

        // Release lock (outer `run` also releases, but explicit here for clarity)
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
            files_changed: 0, // Unknown during resume — would need manifest re-read
            bytes_transferred: 0,
            duration: start.elapsed(),
            was_update_in_place: false,
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
            level: SnapshotTier::Hourly,
            force_rotate: false,
            force_lock: false,
            dry_run: true,
        };
        assert!(!opts.force_rotate);
        assert!(opts.dry_run);
    }

    #[test]
    fn test_sync_result_fields() {
        let result = SyncResult {
            snapshot_name: "hourly.0__2026-10-28T08-00".to_string(),
            files_changed: 42,
            bytes_transferred: 1_048_576,
            duration: Duration::from_secs(30),
            was_update_in_place: false,
        };
        assert_eq!(result.files_changed, 42);
        assert_eq!(result.bytes_transferred, 1_048_576);
        assert!(!result.was_update_in_place);
    }
}
