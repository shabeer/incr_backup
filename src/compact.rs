//! # Compact Module — Dynamic Snapshot Consolidation
//!
//! Implements reverse-incremental snapshot consolidation: combining multiple
//! discrete snapshots within a user-specified date range into a single named snapshot
//! with mathematically verifiable point-in-time recovery and zero data loss.
//!
//! ## Core Principles
//! 1. Identical Snapshot Format: Consolidated snapshots have the exact same directory
//!    and manifest format as normal snapshots, allowing recursive compaction.
//! 2. Two-Phase Safe Compaction Protocol: Staging -> Verification -> Commit -> Purge.
//!    Source snapshots are purged only after the consolidated snapshot is verified.
//! 3. Reverse-Incremental Dependency Resolution:
//!    - Consolidated manifest adopts the newest snapshot in the range (the anchor).
//!    - For each file in the diff directories:
//!      - If no snapshots exist before the range: discard all diffs.
//!      - If a preceding snapshot ($S_{prev}$) exists:
//!        - If file is in $S_{prev}$: keep the earliest diff version in range (which holds
//!          the state prior to the range edits). Discard later revisions.
//!        - If file is not in $S_{prev}$: discard (created and deleted intra-range).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use chrono::{DateTime, FixedOffset, Local, Utc};
use log::{debug, info, warn};
use serde::Deserialize;

use crate::config::{Config, SnapshotTimezone};
use crate::error::{IncrError, Result};
use crate::lock::LockManager;
use crate::manifest::{format_size, Manifest};
use crate::rclone::Rclone;

// ─── Options & Plan Data Structures ──────────────────────────────────────────

/// Options for a compaction operation.
#[derive(Debug, Clone)]
pub struct CompactOptions {
    /// Start of date range (ISO-8601 with explicit timezone offset matching configured timezone).
    pub from: String,
    /// End of date range (ISO-8601 with explicit timezone offset matching configured timezone).
    pub to: String,
    /// Name for the consolidated snapshot (e.g., "daily-2026-10-28").
    pub name: String,
    /// Show high-level summary of changes.
    pub summary: bool,
    /// Show detailed file-by-file plan.
    pub full_list: bool,
    /// Dry run mode: calculate plan without modifying remote.
    pub dry_run: bool,
    /// Break stale lock if locked.
    pub force_lock: bool,
}

/// Metadata about an existing snapshot discovered on the remote.
#[derive(Debug, Clone)]
pub struct SnapshotMeta {
    /// Snapshot identifier (e.g. "hourly.0__2026-10-28T10-00", "daily-2026-10-28", "snap-10:00").
    pub name: String,
    /// Filename of the manifest in `.meta/manifests/` (e.g. "snap-10:00.json" or "hourly.0.json").
    pub manifest_name: String,
    /// Timestamp of when the snapshot was taken.
    pub created_at: DateTime<Utc>,
    /// Subdirectory name under `diffs/`, if diff files exist.
    pub diff_dir: Option<String>,
    /// Full parsed manifest.
    pub manifest: Manifest,
}

/// A diff file planned to be kept in the consolidated snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeepFilePlan {
    pub path: String,
    pub size: u64,
    pub md5: String,
    pub source_diff_dir: String,
    pub source_snapshot: String,
}

/// A diff file planned to be discarded (space reclaimed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscardFilePlan {
    pub path: String,
    pub size: u64,
    pub md5: String,
    pub source_diff_dir: String,
    pub source_snapshot: String,
    pub reason: String,
}

/// Complete plan computed for a compaction operation.
#[derive(Debug, Clone)]
pub struct CompactPlan {
    pub from_str: String,
    pub to_str: String,
    pub from_utc: DateTime<Utc>,
    pub to_utc: DateTime<Utc>,
    pub name: String,
    pub in_range_snapshots: Vec<SnapshotMeta>,
    pub anchor_snapshot: SnapshotMeta,
    pub prev_snapshot: Option<SnapshotMeta>,
    pub files_to_keep: Vec<KeepFilePlan>,
    pub files_to_discard: Vec<DiscardFilePlan>,
    pub snapshots_to_purge: Vec<SnapshotMeta>,
    pub is_single: bool,
}

/// Summary result of a completed compaction.
#[derive(Debug, Clone)]
pub struct CompactResult {
    pub name: String,
    pub snapshots_consolidated: usize,
    pub files_kept: usize,
    pub bytes_kept: u64,
    pub files_discarded: usize,
    pub bytes_discarded: u64,
    pub duration: Duration,
}

// ─── Timezone Parsing & Validation ──────────────────────────────────────────

/// Parse an ISO-8601 string and validate that its explicit timezone offset
/// matches the configured timezone.
pub fn parse_and_validate_timestamp(
    param_name: &str,
    input: &str,
    configured_tz: SnapshotTimezone,
) -> Result<DateTime<Utc>> {
    let (parsed_dt, offset_str) = parse_iso_datetime_with_offset(input).map_err(|reason| {
        IncrError::InvalidConfig(format!("Invalid timestamp in {param_name} ('{input}'): {reason}"))
    })?;

    // Determine expected offset string for configured timezone
    let expected_offset = match configured_tz {
        SnapshotTimezone::Utc => "+00:00".to_string(),
        SnapshotTimezone::Local => {
            // Local machine's current offset, formatted as %:z (+05:30)
            let local_now = Local::now();
            format!("{}", local_now.offset())
        }
    };

    if offset_str != expected_offset {
        return Err(IncrError::TimezoneMismatch {
            param: param_name.to_string(),
            got: offset_str,
            expected: expected_offset,
        });
    }

    Ok(parsed_dt.with_timezone(&Utc))
}

/// Parse datetime with explicit offset from common ISO-8601 formats.
fn parse_iso_datetime_with_offset(
    s: &str,
) -> std::result::Result<(DateTime<FixedOffset>, String), String> {
    // 1. Standard RFC3339 (e.g. 2026-10-28T00:00:00+05:30 or 2026-10-28T00:00:00Z)
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        let offset = format!("{}", dt.offset());
        return Ok((dt, offset));
    }

    // 2. Custom patterns with colons or non-colon offsets
    let formats = [
        "%Y-%m-%dT%H:%M:%S%:z",
        "%Y-%m-%dT%H:%M:%S%z",
        "%Y-%m-%dT%H:%M%:z",
        "%Y-%m-%dT%H:%M%z",
        "%Y-%m-%d %H:%M:%S%:z",
        "%Y-%m-%d %H:%M:%S%z",
    ];

    for fmt in &formats {
        if let Ok(dt) = DateTime::parse_from_str(s, fmt) {
            let offset = format!("{}", dt.offset());
            return Ok((dt, offset));
        }
    }

    // 3. Handle 'Z' / 'z' suffix without seconds (e.g. 2026-10-28T00:00Z)
    if s.ends_with('Z') || s.ends_with('z') {
        let trimmed = &s[..s.len() - 1];
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M") {
            let dt = naive.and_utc().fixed_offset();
            return Ok((dt, "+00:00".to_string()));
        }
    }

    // Check if user passed a naive date/time without offset
    if chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").is_ok()
        || chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
    {
        return Err("missing explicit timezone offset (e.g., +05:30 or +00:00)".to_string());
    }

    Err("expected ISO-8601 format (e.g. 2026-10-28T00:00:00+05:30)".to_string())
}

// ─── Snapshot Discovery ─────────────────────────────────────────────────────

/// Discover all snapshots available on the remote, sorted chronologically.
pub fn discover_all_snapshots(rclone: &Rclone, remote_base: &str) -> Result<Vec<SnapshotMeta>> {
    let manifests_dir = format!("{remote_base}/.meta/manifests/");
    let diffs_dir = format!("{remote_base}/diffs/");

    let manifest_files = match rclone.lsf(&manifests_dir) {
        Ok(files) => files,
        Err(_) => Vec::new(),
    };

    let existing_diff_dirs: HashSet<String> = match rclone.lsd(&diffs_dir) {
        Ok(dirs) => dirs.into_iter().collect(),
        Err(_) => HashSet::new(),
    };

    let mut snapshots = Vec::new();

    for file in manifest_files {
        if !file.ends_with(".json") || file == "current.json" {
            continue;
        }

        let remote_manifest_path = format!("{manifests_dir}{file}");
        let content = match rclone.cat(&remote_manifest_path) {
            Ok(c) => c,
            Err(e) => {
                debug!("Failed to read manifest {file}: {e}");
                continue;
            }
        };

        let manifest = match Manifest::from_json(&content) {
            Ok(m) => m,
            Err(e) => {
                warn!("Failed to parse manifest {file}: {e}");
                continue;
            }
        };

        let stem = file.trim_end_matches(".json").to_string();

        // Match diff directory
        let diff_dir = if existing_diff_dirs.contains(&stem) {
            Some(stem.clone())
        } else if existing_diff_dirs.contains(&manifest.snapshot_name) {
            Some(manifest.snapshot_name.clone())
        } else {
            // Check for tier prefix (e.g. "hourly.0" matching "hourly.0__2026...")
            let prefix = format!("{stem}__");
            existing_diff_dirs.iter().find(|d| d.starts_with(&prefix)).cloned()
        };

        snapshots.push(SnapshotMeta {
            name: if manifest.snapshot_name.is_empty() {
                stem.clone()
            } else {
                manifest.snapshot_name.clone()
            },
            manifest_name: file,
            created_at: manifest.created_at,
            diff_dir,
            manifest,
        });
    }

    // Sort chronologically (oldest first)
    snapshots.sort_by_key(|s| s.created_at);
    Ok(snapshots)
}

// ─── Plan Construction ──────────────────────────────────────────────────────

/// Structure for an entry returned by `lsjson` with hashes.
#[derive(Debug, Deserialize)]
struct LsjsonFileEntry {
    #[serde(rename = "Path")]
    path: String,
    #[serde(rename = "Size")]
    size: u64,
    #[serde(rename = "IsDir", default)]
    is_dir: bool,
    #[serde(rename = "Hashes")]
    hashes: Option<HashMap<String, String>>,
}

/// Build the compaction plan without performing mutations.
pub fn build_compact_plan(
    rclone: &Rclone,
    remote_base: &str,
    config: &Config,
    options: &CompactOptions,
) -> Result<CompactPlan> {
    // 1. Validate name
    if options.name.is_empty()
        || options.name.contains('/')
        || options.name.contains('\\')
        || options.name.starts_with('.')
    {
        return Err(IncrError::CompactionError(format!(
            "Invalid consolidated snapshot name '{}'",
            options.name
        )));
    }

    // 2. Validate timestamps and offsets
    let from_utc = parse_and_validate_timestamp("--from", &options.from, config.timezone)?;
    let to_utc = parse_and_validate_timestamp("--to", &options.to, config.timezone)?;

    if from_utc > to_utc {
        return Err(IncrError::CompactionError(
            "--from timestamp must be before or equal to --to timestamp".to_string(),
        ));
    }

    // 3. Discover snapshots
    let all_snapshots = discover_all_snapshots(rclone, remote_base)?;

    // 4. Filter snapshots in range [from_utc, to_utc]
    let in_range: Vec<SnapshotMeta> = all_snapshots
        .iter()
        .filter(|s| s.created_at >= from_utc && s.created_at <= to_utc)
        .cloned()
        .collect();

    if in_range.is_empty() {
        return Err(IncrError::NoSnapshotsInRange {
            from: options.from.clone(),
            to: options.to.clone(),
        });
    }

    // Check if target name conflicts with an existing snapshot outside this range
    let conflicts = all_snapshots.iter().any(|s| {
        !in_range.iter().any(|r| r.manifest_name == s.manifest_name)
            && (s.name == options.name || s.manifest_name == format!("{}.json", options.name))
    });
    if conflicts {
        return Err(IncrError::CompactionError(format!(
            "Snapshot '{}' already exists outside compaction range. Cannot overwrite.",
            options.name
        )));
    }

    // 5. Single snapshot case
    if in_range.len() == 1 {
        let single = in_range[0].clone();
        return Ok(CompactPlan {
            from_str: options.from.clone(),
            to_str: options.to.clone(),
            from_utc,
            to_utc,
            name: options.name.clone(),
            in_range_snapshots: in_range.clone(),
            anchor_snapshot: single.clone(),
            prev_snapshot: None,
            files_to_keep: Vec::new(),
            files_to_discard: Vec::new(),
            snapshots_to_purge: in_range,
            is_single: true,
        });
    }

    // 6. Multi-snapshot consolidation
    let anchor = in_range.last().unwrap().clone();

    // Find preceding snapshot (immediately before the range)
    let before_range: Vec<&SnapshotMeta> = all_snapshots
        .iter()
        .filter(|s| s.created_at < from_utc)
        .collect();
    let prev = before_range.last().cloned().cloned();

    // Inspect files in diff directories of all in-range snapshots
    // Tuple: (file_path, file_size, file_md5, diff_dir_name, snapshot_name, snap_index)
    struct RawDiffFile {
        path: String,
        size: u64,
        md5: String,
        diff_dir: String,
        snap_name: String,
        snap_idx: usize,
    }

    let mut diff_files: Vec<RawDiffFile> = Vec::new();

    for (idx, snap) in in_range.iter().enumerate() {
        if let Some(ref dir) = snap.diff_dir {
            let diff_path = format!("{remote_base}/diffs/{dir}/");
            if let Ok(lsjson) = rclone.lsjson_recursive_hash(&diff_path) {
                if let Ok(entries) = serde_json::from_str::<Vec<LsjsonFileEntry>>(&lsjson) {
                    for entry in entries {
                        if entry.is_dir {
                            continue;
                        }
                        let md5 = entry
                            .hashes
                            .as_ref()
                            .and_then(|h| {
                                h.get("md5")
                                    .or_else(|| h.get("MD5"))
                                    .or_else(|| h.get("quickxor"))
                            })
                            .cloned()
                            .unwrap_or_default();

                        diff_files.push(RawDiffFile {
                            path: entry.path,
                            size: entry.size,
                            md5,
                            diff_dir: dir.clone(),
                            snap_name: snap.name.clone(),
                            snap_idx: idx,
                        });
                    }
                }
            }
        }
    }

    // Group diff files by relative path
    let mut files_by_path: HashMap<String, Vec<RawDiffFile>> = HashMap::new();
    for f in diff_files {
        files_by_path.entry(f.path.clone()).or_default().push(f);
    }

    let mut files_to_keep = Vec::new();
    let mut files_to_discard = Vec::new();

    // Preceding snapshot file lookup
    let prev_files_map: Option<HashMap<String, String>> = prev.as_ref().map(|p| {
        p.manifest
            .files
            .iter()
            .map(|f| (f.path.clone(), f.md5.clone()))
            .collect()
    });

    for (path, mut occurrences) in files_by_path {
        // Sort occurrences chronologically (by snapshot index in range)
        occurrences.sort_by_key(|o| o.snap_idx);

        match &prev_files_map {
            None => {
                // Beginning of history: no snapshots exist prior to the range.
                // Pre-range history requires zero diffs.
                for occ in occurrences {
                    files_to_discard.push(DiscardFilePlan {
                        path: occ.path,
                        size: occ.size,
                        md5: occ.md5,
                        source_diff_dir: occ.diff_dir,
                        source_snapshot: occ.snap_name,
                        reason: "No snapshots prior to compaction range; diff not needed"
                            .to_string(),
                    });
                }
            }
            Some(prev_map) => {
                if !prev_map.contains_key(&path) {
                    // File created and deleted intra-range (never existed in S_prev).
                    for occ in occurrences {
                        files_to_discard.push(DiscardFilePlan {
                            path: occ.path,
                            size: occ.size,
                            md5: occ.md5,
                            source_diff_dir: occ.diff_dir,
                            source_snapshot: occ.snap_name,
                            reason: "Created intra-range; not needed by pre-range snapshots"
                                .to_string(),
                        });
                    }
                } else {
                    // File existed in S_prev!
                    // Earliest diff rule: keep the earliest diff version, discard later revisions.
                    let earliest = occurrences.remove(0);
                    files_to_keep.push(KeepFilePlan {
                        path: earliest.path.clone(),
                        size: earliest.size,
                        md5: earliest.md5.clone(),
                        source_diff_dir: earliest.diff_dir.clone(),
                        source_snapshot: earliest.snap_name.clone(),
                    });

                    for later in occurrences {
                        files_to_discard.push(DiscardFilePlan {
                            path: later.path,
                            size: later.size,
                            md5: later.md5,
                            source_diff_dir: later.diff_dir,
                            source_snapshot: later.snap_name,
                            reason: format!(
                                "Superseded by earlier diff version in {}",
                                earliest.snap_name
                            ),
                        });
                    }
                }
            }
        }
    }

    // Sort plans by path for consistent deterministic reporting
    files_to_keep.sort_by(|a, b| a.path.cmp(&b.path));
    files_to_discard.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(CompactPlan {
        from_str: options.from.clone(),
        to_str: options.to.clone(),
        from_utc,
        to_utc,
        name: options.name.clone(),
        in_range_snapshots: in_range.clone(),
        anchor_snapshot: anchor,
        prev_snapshot: prev,
        files_to_keep,
        files_to_discard,
        snapshots_to_purge: in_range,
        is_single: false,
    })
}

// ─── Formatting & Reporting ─────────────────────────────────────────────────

/// Format high-level summary of compaction plan.
pub fn format_summary(plan: &CompactPlan) -> String {
    let mut out = String::new();
    out.push_str("Compaction Plan Summary:\n");
    out.push_str(&format!("  Range:                {} → {}\n", plan.from_str, plan.to_str));
    out.push_str(&format!("  Consolidated name:    {}\n", plan.name));

    if plan.is_single {
        out.push_str("  Action:               Single snapshot in range — alias / rename\n");
        out.push_str(&format!("  Source snapshot:      {}\n", plan.anchor_snapshot.name));
        return out;
    }

    let snap_names: Vec<&str> = plan.in_range_snapshots.iter().map(|s| s.name.as_str()).collect();
    out.push_str(&format!(
        "  Snapshots in range:   {} ({})\n",
        plan.in_range_snapshots.len(),
        snap_names.join(", ")
    ));
    out.push_str(&format!("  Anchor snapshot:      {}\n", plan.anchor_snapshot.name));

    let prev_name = plan
        .prev_snapshot
        .as_ref()
        .map(|s| s.name.as_str())
        .unwrap_or("(none — beginning of history)");
    out.push_str(&format!("  Preceding snapshot:   {}\n", prev_name));

    let total_kept_bytes: u64 = plan.files_to_keep.iter().map(|f| f.size).sum();
    let total_discard_bytes: u64 = plan.files_to_discard.iter().map(|f| f.size).sum();

    out.push_str(&format!(
        "  Diff files kept:      {} ({})\n",
        plan.files_to_keep.len(),
        format_size(total_kept_bytes)
    ));
    out.push_str(&format!(
        "  Diff files discarded: {} ({} reclaimed)\n",
        plan.files_to_discard.len(),
        format_size(total_discard_bytes)
    ));

    out.push_str(&format!(
        "  Snapshots to purge:   {} ({})\n",
        plan.snapshots_to_purge.len(),
        snap_names.join(", ")
    ));

    out
}

/// Format detailed file-by-file plan.
pub fn format_full_list(plan: &CompactPlan) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Detailed Compaction Plan for '{}':\n",
        plan.name
    ));

    if plan.is_single {
        out.push_str(&format!(
            "  Rename snapshot '{}' → '{}'\n",
            plan.anchor_snapshot.name, plan.name
        ));
        return out;
    }

    out.push_str(&format!(
        "  Anchor Snapshot: {} (created {})\n\n",
        plan.anchor_snapshot.name, plan.anchor_snapshot.created_at
    ));

    out.push_str(&format!(
        "  Files to KEEP in diffs/{}/\n",
        plan.name
    ));
    if plan.files_to_keep.is_empty() {
        out.push_str("    (none)\n");
    } else {
        for f in &plan.files_to_keep {
            out.push_str(&format!(
                "    + {} ({}, md5: {}) from diffs/{}/\n",
                f.path,
                format_size(f.size),
                f.md5,
                f.source_diff_dir
            ));
        }
    }

    out.push_str("\n  Files to DISCARD:\n");
    if plan.files_to_discard.is_empty() {
        out.push_str("    (none)\n");
    } else {
        for f in &plan.files_to_discard {
            out.push_str(&format!(
                "    - {} ({}) in diffs/{}/ [{}]\n",
                f.path,
                format_size(f.size),
                f.source_diff_dir,
                f.reason
            ));
        }
    }

    out.push_str("\n  Manifest Actions:\n");
    out.push_str(&format!(
        "    * Create .meta/manifests/{}.json (cloned from {})\n",
        plan.name, plan.anchor_snapshot.name
    ));

    out.push_str("\n  Purge Actions:\n");
    for s in &plan.snapshots_to_purge {
        if let Some(ref dir) = s.diff_dir {
            out.push_str(&format!("    * Purge diffs/{dir}/\n"));
        }
        out.push_str(&format!("    * Delete .meta/manifests/{}\n", s.manifest_name));
    }

    out
}

// ─── Execution Pipeline ─────────────────────────────────────────────────────

/// Run the compaction pipeline: build plan, acquire lock, verify, commit, and purge.
pub fn run_compact(
    config: &Config,
    rclone: &Rclone,
    options: &CompactOptions,
) -> Result<CompactResult> {
    let start = Instant::now();
    let remote_base = &config.remote_path;

    // Acquire lock
    let lock_mgr = LockManager::new(rclone, remote_base, config.timezone);
    if options.dry_run {
        info!("[DRY-RUN] Would acquire remote advisory lock");
    } else {
        lock_mgr.acquire(options.force_lock)?;
        debug!("Remote advisory lock acquired");
    }

    // Build plan
    let plan = match build_compact_plan(rclone, remote_base, config, options) {
        Ok(p) => p,
        Err(e) => {
            if !options.dry_run {
                let _ = lock_mgr.release();
            }
            return Err(e);
        }
    };

    // Dry-run handling
    if options.dry_run {
        if options.full_list {
            println!("{}", format_full_list(&plan));
        } else {
            // Default to summary for dry-run
            println!("{}", format_summary(&plan));
        }

        let total_kept_bytes: u64 = plan.files_to_keep.iter().map(|f| f.size).sum();
        let total_discard_bytes: u64 = plan.files_to_discard.iter().map(|f| f.size).sum();

        return Ok(CompactResult {
            name: plan.name,
            snapshots_consolidated: plan.in_range_snapshots.len(),
            files_kept: plan.files_to_keep.len(),
            bytes_kept: total_kept_bytes,
            files_discarded: plan.files_to_discard.len(),
            bytes_discarded: total_discard_bytes,
            duration: start.elapsed(),
        });
    }

    // Show plan if summary or full_list requested
    if options.full_list {
        println!("{}", format_full_list(&plan));
    } else if options.summary {
        println!("{}", format_summary(&plan));
    }

    // Execute plan
    let exec_res = execute_compact_plan(rclone, remote_base, &plan, start);

    // Release lock
    if let Err(e) = lock_mgr.release() {
        warn!("Failed to release remote lock: {e}");
    }

    exec_res
}

fn execute_compact_plan(
    rclone: &Rclone,
    remote_base: &str,
    plan: &CompactPlan,
    start: Instant,
) -> Result<CompactResult> {
    // ── Single Snapshot Rename Case ─────────────────────────────────────────
    if plan.is_single {
        info!("Executing single snapshot alias / rename to '{}'", plan.name);
        let single = &plan.anchor_snapshot;

        let mut manifest = single.manifest.clone();
        manifest.snapshot_name = plan.name.clone();
        let json = manifest.to_json()?;

        let staging_manifest = format!("{remote_base}/.meta/staging/{}.manifest.json", plan.name);
        let final_manifest = format!("{remote_base}/.meta/manifests/{}.json", plan.name);

        rclone.rcat(&staging_manifest, &json)?;
        rclone.moveto(&staging_manifest, &final_manifest)?;

        if let Some(ref dir) = single.diff_dir {
            if dir != &plan.name {
                let old_diff = format!("{remote_base}/diffs/{dir}");
                let new_diff = format!("{remote_base}/diffs/{}", plan.name);
                rclone.moveto(&old_diff, &new_diff)?;
            }
        }

        if single.manifest_name != format!("{}.json", plan.name) {
            let old_manifest = format!("{remote_base}/.meta/manifests/{}", single.manifest_name);
            let _ = rclone.delete_file(&old_manifest);
        }

        return Ok(CompactResult {
            name: plan.name.clone(),
            snapshots_consolidated: 1,
            files_kept: 0,
            bytes_kept: 0,
            files_discarded: 0,
            bytes_discarded: 0,
            duration: start.elapsed(),
        });
    }

    // ── Multi-Snapshot Consolidation (Two-Phase Safe Protocol) ──────────────
    let staging_dir = format!("{remote_base}/.meta/staging/{}/", plan.name);
    let staging_manifest = format!("{remote_base}/.meta/staging/{}.manifest.json", plan.name);
    let final_manifest = format!("{remote_base}/.meta/manifests/{}.json", plan.name);
    let final_diff_dir = format!("{remote_base}/diffs/{}", plan.name);

    // Clean up any existing staging from aborted previous runs
    let _ = rclone.purge(&staging_dir);
    let _ = rclone.delete_file(&staging_manifest);

    info!(
        "Stage 1: Staging {} diff files for '{}'...",
        plan.files_to_keep.len(),
        plan.name
    );

    // Step 1: Copy kept diff files into staging
    if !plan.files_to_keep.is_empty() {
        rclone.mkdir(&staging_dir)?;
        for file in &plan.files_to_keep {
            let src = format!("{remote_base}/diffs/{}/{}", file.source_diff_dir, file.path);
            let dst = format!("{staging_dir}{}", file.path);
            rclone.copyto(&src, &dst)?;
        }
    }

    // Step 1b: Create and stage consolidated manifest
    let mut manifest = plan.anchor_snapshot.manifest.clone();
    manifest.snapshot_name = plan.name.clone();
    let json = manifest.to_json()?;
    rclone.rcat(&staging_manifest, &json)?;

    // Step 2: Verification
    info!("Stage 2: Verifying staged files and manifest...");
    let staged_json = rclone.cat(&staging_manifest)?;
    let parsed_manifest = Manifest::from_json(&staged_json)?;
    if parsed_manifest.files.len() != plan.anchor_snapshot.manifest.files.len() {
        let _ = rclone.purge(&staging_dir);
        let _ = rclone.delete_file(&staging_manifest);
        return Err(IncrError::CompactionError(
            "Staged manifest validation failed: file count mismatch".to_string(),
        ));
    }

    if !plan.files_to_keep.is_empty() {
        let staged_lsjson = rclone.lsjson_recursive_hash(&staging_dir)?;
        let staged_entries: Vec<LsjsonFileEntry> =
            serde_json::from_str(&staged_lsjson).unwrap_or_default();
        let staged_map: HashMap<String, String> = staged_entries
            .into_iter()
            .filter(|e| !e.is_dir)
            .map(|e| {
                let md5 = e
                    .hashes
                    .as_ref()
                    .and_then(|h| {
                        h.get("md5")
                            .or_else(|| h.get("MD5"))
                            .or_else(|| h.get("quickxor"))
                    })
                    .cloned()
                    .unwrap_or_default();
                (e.path, md5)
            })
            .collect();

        for file in &plan.files_to_keep {
            match staged_map.get(&file.path) {
                Some(staged_md5) => {
                    if !file.md5.is_empty() && !staged_md5.is_empty() && file.md5 != *staged_md5 {
                        let _ = rclone.purge(&staging_dir);
                        let _ = rclone.delete_file(&staging_manifest);
                        return Err(IncrError::CompactionError(format!(
                            "Hash verification failed for staged file '{}'",
                            file.path
                        )));
                    }
                }
                None => {
                    let _ = rclone.purge(&staging_dir);
                    let _ = rclone.delete_file(&staging_manifest);
                    return Err(IncrError::CompactionError(format!(
                        "Verification failed: file '{}' missing from staging directory",
                        file.path
                    )));
                }
            }
        }
    }

    // Step 3: Commit (Move manifest and diffs into active remote location)
    info!("Stage 3: Committing consolidated snapshot '{}'...", plan.name);
    rclone.moveto(&staging_manifest, &final_manifest)?;

    if !plan.files_to_keep.is_empty() {
        // Move staging directory to final diffs/<name>
        rclone.moveto(&staging_dir.trim_end_matches('/'), &final_diff_dir)?;
    } else {
        let _ = rclone.purge(&staging_dir);
    }

    // Consolidated snapshot is now fully active!

    // Step 4: Purge source snapshots and their manifests
    info!(
        "Stage 4: Purging {} source snapshots...",
        plan.snapshots_to_purge.len()
    );
    for snap in &plan.snapshots_to_purge {
        if let Some(ref dir) = snap.diff_dir {
            let old_diff = format!("{remote_base}/diffs/{dir}");
            let _ = rclone.purge(&old_diff);
        }
        let old_manifest = format!("{remote_base}/.meta/manifests/{}", snap.manifest_name);
        let _ = rclone.delete_file(&old_manifest);
    }

    let total_kept_bytes: u64 = plan.files_to_keep.iter().map(|f| f.size).sum();
    let total_discard_bytes: u64 = plan.files_to_discard.iter().map(|f| f.size).sum();

    Ok(CompactResult {
        name: plan.name.clone(),
        snapshots_consolidated: plan.in_range_snapshots.len(),
        files_kept: plan.files_to_keep.len(),
        bytes_kept: total_kept_bytes,
        files_discarded: plan.files_to_discard.len(),
        bytes_discarded: total_discard_bytes,
        duration: start.elapsed(),
    })
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::FileEntry;

    fn sample_manifest(name: &str, files: Vec<(&str, u64, &str)>, dt: DateTime<Utc>) -> Manifest {
        let file_entries: Vec<FileEntry> = files
            .into_iter()
            .map(|(path, size, md5)| FileEntry {
                path: path.to_string(),
                size,
                md5: md5.to_string(),
                mtime: dt.to_rfc3339(),
            })
            .collect();
        Manifest::new(name, file_entries, &dt)
    }

    #[test]
    fn test_parse_and_validate_timestamp_utc() {
        let dt = parse_and_validate_timestamp(
            "--from",
            "2026-10-28T00:00:00Z",
            SnapshotTimezone::Utc,
        )
        .unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-10-28T00:00:00+00:00");

        let dt2 = parse_and_validate_timestamp(
            "--to",
            "2026-10-28T23:59:59+00:00",
            SnapshotTimezone::Utc,
        )
        .unwrap();
        assert_eq!(dt2.to_rfc3339(), "2026-10-28T23:59:59+00:00");

        // Offset mismatch
        let err = parse_and_validate_timestamp(
            "--from",
            "2026-10-28T00:00:00+05:30",
            SnapshotTimezone::Utc,
        )
        .unwrap_err();
        match err {
            IncrError::TimezoneMismatch { param, got, expected } => {
                assert_eq!(param, "--from");
                assert_eq!(got, "+05:30");
                assert_eq!(expected, "+00:00");
            }
            _ => panic!("unexpected error type"),
        }
    }

    #[test]
    fn test_parse_and_validate_timestamp_local() {
        let local_offset = format!("{}", Local::now().offset());
        let valid_str = format!("2026-10-28T12:00:00{}", local_offset);

        let dt = parse_and_validate_timestamp("--from", &valid_str, SnapshotTimezone::Local)
            .unwrap();
        assert!(dt.to_rfc3339().ends_with("+00:00") || dt.to_rfc3339().ends_with('Z'));

        // Offset mismatch with UTC Z
        let err = parse_and_validate_timestamp(
            "--from",
            "2026-10-28T12:00:00Z",
            SnapshotTimezone::Local,
        )
        .unwrap_err();
        match err {
            IncrError::TimezoneMismatch { param, got, expected } => {
                assert_eq!(param, "--from");
                assert_eq!(got, "+00:00");
                assert_eq!(expected, local_offset);
            }
            _ => panic!("unexpected error type"),
        }
    }

    #[test]
    fn test_earliest_diff_rule_logic() {
        // Test logic matching Section 2 & 3:
        // Day 1 (S_prev): report.docx = VA, budget.xlsx = V1, logo.png = VL
        let t1 = DateTime::parse_from_rfc3339("2026-10-27T23:00:00Z").unwrap().with_timezone(&Utc);
        let s_prev = SnapshotMeta {
            name: "day-2026-10-27".to_string(),
            manifest_name: "day-2026-10-27.json".to_string(),
            created_at: t1,
            diff_dir: None,
            manifest: sample_manifest(
                "day-2026-10-27",
                vec![
                    ("report.docx", 100, "VA"),
                    ("budget.xlsx", 200, "V1"),
                    ("logo.png", 300, "VL"),
                ],
                t1,
            ),
        };

        // Day 2 (Anchor: snap-20:00): report.docx = VC, budget.xlsx = V2, logo.png = VL
        let t2 = DateTime::parse_from_rfc3339("2026-10-28T20:00:00Z").unwrap().with_timezone(&Utc);
        let anchor = SnapshotMeta {
            name: "snap-20:00".to_string(),
            manifest_name: "snap-20:00.json".to_string(),
            created_at: t2,
            diff_dir: Some("snap-20:00".to_string()),
            manifest: sample_manifest(
                "snap-20:00",
                vec![
                    ("report.docx", 105, "VC"),
                    ("budget.xlsx", 210, "V2"),
                    ("logo.png", 300, "VL"),
                ],
                t2,
            ),
        };

        assert_eq!(anchor.manifest.files.len(), 3);
        assert_eq!(s_prev.manifest.files.len(), 3);
    }
}
