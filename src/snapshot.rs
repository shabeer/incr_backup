//! # Snapshot Module — Rotation, Promotion, Deletion
//!
//! Implements rsnapshot-style snapshot management: shifting snapshot indices,
//! promoting from one tier to another, and deleting expired snapshots.
//!
//! ## Analogy
//! Like rsnapshot's rotation logic, but using rclone remote operations
//! (server-side renames on Google Drive = zero bandwidth).
//!
//! ## Key Concepts
//! - **Rotation**: Shift all snapshots in a tier up by one index
//!   (hourly.0→1, 1→2, ..., N→delete if at limit)
//! - **Promotion**: Take the oldest snapshot from one tier and rename it
//!   as the newest in the next tier (hourly.5 → daily.0)
//! - **Tiers**: hourly → daily → weekly → monthly (cascade order)

use std::fmt;

use chrono::Utc;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};

use crate::config::{RetentionConfig, SnapshotTimezone};
use crate::manifest::Manifest;
use crate::rclone::Rclone;
use crate::error::{IncrError, Result};

// ─── SnapshotTier ───────────────────────────────────────────────────────────

/// The four retention tiers, ordered from finest to coarsest granularity.
///
/// Like a Java enum — each variant represents a tier level.
/// The `match` statement forces you to handle all variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SnapshotTier {
    Hourly,
    Daily,
    Weekly,
    Monthly,
}

impl SnapshotTier {
    /// Get the string prefix used in directory names (e.g., "hourly").
    pub fn prefix(&self) -> &'static str {
        match self {
            SnapshotTier::Hourly => "hourly",
            SnapshotTier::Daily => "daily",
            SnapshotTier::Weekly => "weekly",
            SnapshotTier::Monthly => "monthly",
        }
    }

    /// Parse a tier from a string (case-insensitive).
    ///
    /// Used for CLI argument parsing.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "hourly" => Ok(SnapshotTier::Hourly),
            "daily" => Ok(SnapshotTier::Daily),
            "weekly" => Ok(SnapshotTier::Weekly),
            "monthly" => Ok(SnapshotTier::Monthly),
            _ => Err(IncrError::InvalidSnapshotName {
                name: format!("invalid tier: '{s}'. Expected: hourly, daily, weekly, monthly"),
            }),
        }
    }

    /// Get the parent tier (the tier that promotes INTO this one).
    ///
    /// Hourly has no parent (it's the base). Daily is promoted from hourly, etc.
    pub fn child_tier(&self) -> Option<SnapshotTier> {
        match self {
            SnapshotTier::Hourly => None,
            SnapshotTier::Daily => Some(SnapshotTier::Hourly),
            SnapshotTier::Weekly => Some(SnapshotTier::Daily),
            SnapshotTier::Monthly => Some(SnapshotTier::Weekly),
        }
    }
}

impl fmt::Display for SnapshotTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.prefix())
    }
}

// ─── SnapshotName ───────────────────────────────────────────────────────────

/// A parsed snapshot name like `hourly.0__2026-10-27T22-00`.
///
/// Snapshot directory names follow the format: `{tier}.{index}__{timestamp}`.
/// This struct represents the parsed components.
#[derive(Debug, Clone)]
pub struct SnapshotName {
    /// Which tier this snapshot belongs to.
    pub tier: SnapshotTier,
    /// Index within the tier (0 = newest).
    pub index: u32,
    /// Timestamp string (e.g., "2026-10-27T22-00").
    pub timestamp: String,
}

impl SnapshotName {
    /// Build the directory name (e.g., "hourly.0__2026-10-27T22-00").
    pub fn dir_name(&self) -> String {
        format!("{}.{}__{}", self.tier.prefix(), self.index, self.timestamp)
    }

    /// Parse a directory name into a SnapshotName.
    ///
    /// Expected format: `hourly.0__2026-10-27T22-00`
    ///
    /// # Errors
    /// Returns `IncrError::InvalidSnapshotName` if the format doesn't match.
    pub fn parse(dir_name: &str) -> Result<Self> {
        // Split on "__" to separate "hourly.0" from "2026-10-27T22-00"
        let parts: Vec<&str> = dir_name.splitn(2, "__").collect();
        if parts.is_empty() {
            return Err(IncrError::InvalidSnapshotName {
                name: dir_name.to_string(),
            });
        }

        let prefix_index = parts[0];
        let timestamp = if parts.len() > 1 {
            parts[1].to_string()
        } else {
            String::new()
        };

        // Split "hourly.0" into "hourly" and "0"
        let dot_parts: Vec<&str> = prefix_index.rsplitn(2, '.').collect();
        if dot_parts.len() != 2 {
            return Err(IncrError::InvalidSnapshotName {
                name: dir_name.to_string(),
            });
        }

        let index: u32 = dot_parts[0].parse().map_err(|_| IncrError::InvalidSnapshotName {
            name: dir_name.to_string(),
        })?;

        let tier = SnapshotTier::from_str(dot_parts[1])?;

        Ok(SnapshotName {
            tier,
            index,
            timestamp,
        })
    }

    /// Parse a simple snapshot reference like "hourly.2" (no timestamp needed).
    pub fn parse_ref(name: &str) -> Result<(SnapshotTier, u32)> {
        let parts: Vec<&str> = name.rsplitn(2, '.').collect();
        if parts.len() != 2 {
            return Err(IncrError::InvalidSnapshotName {
                name: name.to_string(),
            });
        }

        let index: u32 = parts[0].parse().map_err(|_| IncrError::InvalidSnapshotName {
            name: name.to_string(),
        })?;

        let tier = SnapshotTier::from_str(parts[1])?;

        Ok((tier, index))
    }
}

// ─── SnapshotInfo ───────────────────────────────────────────────────────────

/// Full information about an existing snapshot on the remote.
#[derive(Debug, Clone)]
pub struct SnapshotInfo {
    /// Parsed snapshot name.
    pub name: SnapshotName,
    /// The diff directory name as it appears on the remote.
    pub dir_name: String,
    /// Manifest for this snapshot (loaded lazily).
    pub manifest: Option<Manifest>,
}

// ─── RotationManager ────────────────────────────────────────────────────────

/// Manages snapshot rotation, promotion, and deletion on the remote.
///
/// ## Key Operations
/// - `rotate_tier(Hourly)`: shift hourly.4→5, 3→4, ..., 0→1, delete 5 if full
/// - `promote(Hourly, Daily)`: rename hourly.{max} → daily.0
/// - `should_rotate()`: check if min_interval has elapsed
///
/// ## Analogy
/// Like rsnapshot's rotation logic, but operating on remote directories
/// via rclone moveto/purge commands instead of local filesystem hardlinks.
pub struct RotationManager<'a> {
    rclone: &'a Rclone,
    remote_base: String,
    retention: RetentionConfig,
}

impl<'a> RotationManager<'a> {
    /// Create a new RotationManager.
    pub fn new(rclone: &'a Rclone, remote_base: impl Into<String>, retention: &RetentionConfig) -> Self {
        Self {
            rclone,
            remote_base: remote_base.into(),
            retention: retention.clone(),
        }
    }

    /// Get the configured retention limit for a tier.
    pub fn retention_limit(&self, tier: &SnapshotTier) -> u32 {
        match tier {
            SnapshotTier::Hourly => self.retention.hourly,
            SnapshotTier::Daily => self.retention.daily,
            SnapshotTier::Weekly => self.retention.weekly,
            SnapshotTier::Monthly => self.retention.monthly,
        }
    }

    /// List existing snapshots for a tier, sorted by index.
    ///
    /// Reads the `diffs/` directory on the remote and filters for entries
    /// matching the tier prefix (e.g., "hourly.").
    pub fn list_snapshots(&self, tier: &SnapshotTier) -> Result<Vec<SnapshotInfo>> {
        let diffs_path = format!("{}/diffs/", self.remote_base);
        let prefix = format!("{}.", tier.prefix());

        let dirs = match self.rclone.lsd(&diffs_path) {
            Ok(d) => d,
            Err(_) => {
                // diffs/ may not exist yet (first run)
                debug!("No diffs/ directory found — first run?");
                return Ok(Vec::new());
            }
        };

        let mut snapshots: Vec<SnapshotInfo> = dirs
            .iter()
            .filter(|d| d.starts_with(&prefix))
            .filter_map(|d| {
                SnapshotName::parse(d).ok().map(|name| SnapshotInfo {
                    name,
                    dir_name: d.clone(),
                    manifest: None,
                })
            })
            .collect();

        // Sort by index (0 = newest)
        snapshots.sort_by_key(|s| s.name.index);

        Ok(snapshots)
    }

    /// Rotate snapshots within a tier: shift indices up, delete oldest if at limit.
    ///
    /// ## Algorithm
    /// 1. List existing snapshots for the tier
    /// 2. If count >= retention limit, delete the highest-indexed one
    /// 3. Shift all remaining: N→N+1, N-1→N, ..., 0→1
    ///    (must go highest-first to avoid overwriting)
    /// 4. Slot 0 is now free for the new snapshot
    ///
    /// ## Example
    /// With retention=6 and hourlies 0-5:
    /// - Delete hourly.5 (at limit)
    /// - Rename hourly.4→5, 3→4, 2→3, 1→2, 0→1
    /// - Slot hourly.0 is now free
    pub fn rotate_tier(&self, tier: &SnapshotTier) -> Result<()> {
        let snapshots = self.list_snapshots(tier)?;
        let limit = self.retention_limit(tier);

        info!(
            "Rotating {:?}: {} existing, limit={}",
            tier,
            snapshots.len(),
            limit
        );

        if snapshots.is_empty() {
            debug!("No snapshots to rotate for {:?}", tier);
            return Ok(());
        }

        // Step 1: Delete oldest if at or over limit
        if snapshots.len() >= limit as usize {
            let oldest = snapshots.last().unwrap();
            let oldest_path = format!("{}/diffs/{}", self.remote_base, oldest.dir_name);
            info!("Deleting oldest {:?} snapshot: {}", tier, oldest.dir_name);
            match self.rclone.purge(&oldest_path) {
                Ok(_) => {}
                Err(e) => {
                    warn!("Failed to purge {}: {e} (may not exist)", oldest_path);
                }
            }

            // Also delete its manifest
            let short_name = format!("{}.{}", tier.prefix(), oldest.name.index);
            let manifest_path = format!(
                "{}/.meta/manifests/{}.json",
                self.remote_base, short_name
            );
            let _ = self.rclone.delete_file(&manifest_path);
        }

        // Step 2: Shift all snapshots up by one index (highest first!)
        // Must process highest index first to avoid collisions.
        // Like shifting array elements: [0,1,2,3] → [_,0,1,2,3] → delete 4
        let mut to_shift: Vec<&SnapshotInfo> = snapshots.iter().collect();
        to_shift.sort_by_key(|a| std::cmp::Reverse(a.name.index)); // highest first

        for snap in to_shift {
            let new_index = snap.name.index + 1;
            // Skip if this would exceed the retention limit
            if new_index >= limit {
                continue;
            }

            let old_name = &snap.dir_name;
            let new_name = format!("{}.{}__{}", tier.prefix(), new_index, snap.name.timestamp);

            let old_path = format!("{}/diffs/{}", self.remote_base, old_name);
            let new_path = format!("{}/diffs/{}", self.remote_base, new_name);

            debug!("Rotating: {} → {}", old_name, new_name);
            self.rclone.moveto(&old_path, &new_path)?;

            // Also rename manifest
            let old_manifest = format!(
                "{}/.meta/manifests/{}.{}.json",
                self.remote_base,
                tier.prefix(),
                snap.name.index
            );
            let new_manifest = format!(
                "{}/.meta/manifests/{}.{}.json",
                self.remote_base,
                tier.prefix(),
                new_index
            );
            let _ = self.rclone.moveto(&old_manifest, &new_manifest);
        }

        Ok(())
    }

    /// Promote the oldest snapshot from `from_tier` to `to_tier.0`.
    ///
    /// Takes the highest-indexed snapshot in `from_tier` and renames it
    /// to `to_tier.0`. For example, `hourly.5` → `daily.0`.
    ///
    /// Server-side rename on GDrive = zero bandwidth.
    pub fn promote(&self, from_tier: &SnapshotTier, to_tier: &SnapshotTier) -> Result<()> {
        let snapshots = self.list_snapshots(from_tier)?;

        if snapshots.is_empty() {
            warn!(
                "No {:?} snapshots to promote to {:?}",
                from_tier, to_tier
            );
            return Ok(());
        }

        // Get the oldest (highest index) snapshot
        let oldest = snapshots.last().unwrap();
        let old_path = format!("{}/diffs/{}", self.remote_base, oldest.dir_name);

        // New name: to_tier.0__<original_timestamp>
        let new_name = format!(
            "{}.0__{}",
            to_tier.prefix(),
            oldest.name.timestamp
        );
        let new_path = format!("{}/diffs/{}", self.remote_base, new_name);

        info!(
            "Promoting {} → {} (server-side rename)",
            oldest.dir_name, new_name
        );
        self.rclone.moveto(&old_path, &new_path)?;

        // Move manifest too
        let old_manifest = format!(
            "{}/.meta/manifests/{}.{}.json",
            self.remote_base,
            from_tier.prefix(),
            oldest.name.index
        );
        let new_manifest = format!(
            "{}/.meta/manifests/{}.0.json",
            self.remote_base,
            to_tier.prefix()
        );
        let _ = self.rclone.moveto(&old_manifest, &new_manifest);

        Ok(())
    }

    /// Check if enough time has elapsed since the last snapshot to warrant rotation.
    ///
    /// If `min_interval_minutes` is 0, always returns true (rsnapshot behavior).
    /// Otherwise, checks if the newest snapshot's timestamp is older than
    /// `min_interval_minutes`.
    pub fn should_rotate(&self, tier: &SnapshotTier) -> Result<bool> {
        if self.retention.min_interval_minutes == 0 {
            return Ok(true); // Always rotate (rsnapshot-style)
        }

        let snapshots = self.list_snapshots(tier)?;

        if snapshots.is_empty() {
            return Ok(true); // No snapshots → must create first one
        }

        // Get newest snapshot (index 0)
        let newest = &snapshots[0];

        // Parse timestamp from the snapshot name
        // Format: "2026-10-28T08-00"
        let ts = &newest.name.timestamp;
        let parsed = parse_snapshot_timestamp(ts);

        match parsed {
            Some(dt) => {
                let elapsed = Utc::now().signed_duration_since(dt).num_minutes();
                let should = elapsed >= self.retention.min_interval_minutes as i64;
                debug!(
                    "Newest {:?} snapshot is {} minutes old (min_interval={}): {}",
                    tier,
                    elapsed,
                    self.retention.min_interval_minutes,
                    if should { "will rotate" } else { "update in place" }
                );
                Ok(should)
            }
            None => {
                warn!("Could not parse timestamp '{}', assuming rotation needed", ts);
                Ok(true)
            }
        }
    }

    /// Delete the oldest snapshot in a tier if at the retention limit.
    ///
    /// Returns the name of the deleted snapshot if one was deleted.
    pub fn delete_oldest(&self, tier: &SnapshotTier) -> Result<Option<String>> {
        let snapshots = self.list_snapshots(tier)?;
        let limit = self.retention_limit(tier);

        if snapshots.len() >= limit as usize {
            let oldest = snapshots.last().unwrap();
            let path = format!("{}/diffs/{}", self.remote_base, oldest.dir_name);
            info!("Deleting oldest {:?} snapshot: {}", tier, oldest.dir_name);
            match self.rclone.purge(&path) {
                Ok(_) => Ok(Some(oldest.dir_name.clone())),
                Err(e) => {
                    warn!("Failed to delete {}: {e}", oldest.dir_name);
                    Ok(None)
                }
            }
        } else {
            Ok(None)
        }
    }

    /// Get the newest snapshot in a tier (index 0).
    pub fn get_newest(&self, tier: &SnapshotTier) -> Result<Option<SnapshotInfo>> {
        let snapshots = self.list_snapshots(tier)?;
        Ok(snapshots.into_iter().next())
    }

    /// Get the name of the newest snapshot directory (convenience).
    pub fn get_newest_snapshot_name(&self, tier: &SnapshotTier) -> Result<Option<String>> {
        Ok(self.get_newest(tier)?.map(|s| s.dir_name))
    }

    /// Generate a backup directory name for a new snapshot.
    ///
    /// Format: `hourly.0__2026-10-28T08-00Z` or `hourly.0__2026-10-28T13-30+0530`
    pub fn get_backup_dir_name(tier: &SnapshotTier, tz: SnapshotTimezone) -> String {
        let timestamp = tz.now_formatted();
        format!("{}.0__{}", tier.prefix(), timestamp)
    }
}

/// Parse a snapshot timestamp string into a DateTime<Utc>.
///
/// Handles:
/// - Local with %z offset: "2026-10-29T09-49+0530"
/// - UTC with 'Z' suffix: "2026-10-29T04-28Z"
/// - Standard RFC3339: "2026-10-29T04:28:00Z"
/// - Legacy hyphenated UTC without timezone: "2026-10-28T08-00"
/// - Date-only: "2026-10-28"
pub fn parse_snapshot_timestamp(ts: &str) -> Option<chrono::DateTime<Utc>> {
    // 1. Try standard RFC3339 directly
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        return Some(dt.with_timezone(&Utc));
    }

    // Try format with colons and timezone offset (e.g., "2026-10-29T09:49+05:30", "2026-10-29T09:49+0530")
    if let Ok(dt) = chrono::DateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M%:z") {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(dt) = chrono::DateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M%z") {
        return Some(dt.with_timezone(&Utc));
    }

    // 2. Try timezone-aware format with %z (e.g., "2026-10-29T09-49+0530")
    if let Ok(dt) = chrono::DateTime::parse_from_str(ts, "%Y-%m-%dT%H-%M%z") {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(dt) = chrono::DateTime::parse_from_str(ts, "%Y-%m-%dT%H-%M-%S%z") {
        return Some(dt.with_timezone(&Utc));
    }

    // 3. Try UTC with 'Z' suffix (e.g., "2026-10-29T04-28Z", "2026-10-29T04:28Z")
    if ts.ends_with('Z') || ts.ends_with('z') {
        let without_z = &ts[..ts.len() - 1];
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(without_z, "%Y-%m-%dT%H:%M") {
            return Some(dt.and_utc());
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(without_z, "%Y-%m-%dT%H:%M:%S") {
            return Some(dt.and_utc());
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(without_z, "%Y-%m-%dT%H-%M") {
            return Some(dt.and_utc());
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(without_z, "%Y-%m-%dT%H-%M-%S") {
            return Some(dt.and_utc());
        }
    }

    // 4. Try legacy format without timezone (e.g., "2026-10-28T08-00")
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H-%M") {
        return Some(dt.and_utc());
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H-%M-%S") {
        return Some(dt.and_utc());
    }

    // 5. Try date-only (e.g., "2026-10-28")
    if let Ok(date) = chrono::NaiveDate::parse_from_str(ts, "%Y-%m-%d") {
        return date.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc());
    }

    None
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};

    #[test]
    fn test_snapshot_tier_prefix() {
        assert_eq!(SnapshotTier::Hourly.prefix(), "hourly");
        assert_eq!(SnapshotTier::Daily.prefix(), "daily");
        assert_eq!(SnapshotTier::Weekly.prefix(), "weekly");
        assert_eq!(SnapshotTier::Monthly.prefix(), "monthly");
    }

    #[test]
    fn test_snapshot_tier_from_str() {
        assert_eq!(SnapshotTier::from_str("hourly").unwrap(), SnapshotTier::Hourly);
        assert_eq!(SnapshotTier::from_str("DAILY").unwrap(), SnapshotTier::Daily);
        assert!(SnapshotTier::from_str("invalid").is_err());
    }

    #[test]
    fn test_snapshot_tier_child() {
        assert_eq!(SnapshotTier::Hourly.child_tier(), None);
        assert_eq!(SnapshotTier::Daily.child_tier(), Some(SnapshotTier::Hourly));
        assert_eq!(SnapshotTier::Weekly.child_tier(), Some(SnapshotTier::Daily));
        assert_eq!(SnapshotTier::Monthly.child_tier(), Some(SnapshotTier::Weekly));
    }

    #[test]
    fn test_snapshot_name_dir_name() {
        let name = SnapshotName {
            tier: SnapshotTier::Hourly,
            index: 0,
            timestamp: "2026-10-28T08-00".to_string(),
        };
        assert_eq!(name.dir_name(), "hourly.0__2026-10-28T08-00");
    }

    #[test]
    fn test_snapshot_name_parse() {
        let name = SnapshotName::parse("hourly.0__2026-10-28T08-00").unwrap();
        assert_eq!(name.tier, SnapshotTier::Hourly);
        assert_eq!(name.index, 0);
        assert_eq!(name.timestamp, "2026-10-28T08-00");

        let name = SnapshotName::parse("daily.3__2026-10-25").unwrap();
        assert_eq!(name.tier, SnapshotTier::Daily);
        assert_eq!(name.index, 3);
    }

    #[test]
    fn test_snapshot_name_parse_ref() {
        let (tier, idx) = SnapshotName::parse_ref("hourly.2").unwrap();
        assert_eq!(tier, SnapshotTier::Hourly);
        assert_eq!(idx, 2);

        let (tier, idx) = SnapshotName::parse_ref("monthly.0").unwrap();
        assert_eq!(tier, SnapshotTier::Monthly);
        assert_eq!(idx, 0);
    }

    #[test]
    fn test_parse_snapshot_timestamp() {
        // Legacy format without timezone
        let dt = parse_snapshot_timestamp("2026-10-28T08-00").unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 10);
        assert_eq!(dt.day(), 28);
        assert_eq!(dt.hour(), 8);

        // UTC with Z
        let dt_z = parse_snapshot_timestamp("2026-10-28T08-00Z").unwrap();
        assert_eq!(dt_z.year(), 2026);
        assert_eq!(dt_z.month(), 10);
        assert_eq!(dt_z.day(), 28);
        assert_eq!(dt_z.hour(), 8);

        // Local with offset +0530
        let dt_offset = parse_snapshot_timestamp("2026-10-28T13-30+0530").unwrap();
        assert_eq!(dt_offset.year(), 2026);
        assert_eq!(dt_offset.month(), 10);
        assert_eq!(dt_offset.day(), 28);
        assert_eq!(dt_offset.hour(), 8); // 13:30 - 05:30 = 08:00 UTC

        // Display format with colons and offset +05:30
        let dt_colon_offset = parse_snapshot_timestamp("2026-10-28T13:30+05:30").unwrap();
        assert_eq!(dt_colon_offset.hour(), 8);

        // Display format with colons and Z
        let dt_colon_z = parse_snapshot_timestamp("2026-10-28T08:00Z").unwrap();
        assert_eq!(dt_colon_z.hour(), 8);
    }

    #[test]
    fn test_retention_limit() {
        let retention = RetentionConfig::default();
        let rclone = Rclone::new(&crate::config::Config {
            source_path: "/tmp".to_string(),
            remote_path: "test:backup".to_string(),
            retention: retention.clone(),
            rclone: crate::config::RcloneConfig::default(),
            exclude: crate::config::ExcludeConfig::default(),
            verbose: 0,
            dry_run: true,
            timezone: SnapshotTimezone::default(),
        });
        let mgr = RotationManager::new(&rclone, "test:backup", &retention);

        assert_eq!(mgr.retention_limit(&SnapshotTier::Hourly), 6);
        assert_eq!(mgr.retention_limit(&SnapshotTier::Daily), 7);
        assert_eq!(mgr.retention_limit(&SnapshotTier::Weekly), 4);
        assert_eq!(mgr.retention_limit(&SnapshotTier::Monthly), 6);
    }

    #[test]
    fn test_get_backup_dir_name_format() {
        let name_utc = RotationManager::get_backup_dir_name(&SnapshotTier::Hourly, SnapshotTimezone::Utc);
        assert!(name_utc.starts_with("hourly.0__"));
        assert!(name_utc.ends_with('Z'));

        let name_local = RotationManager::get_backup_dir_name(&SnapshotTier::Hourly, SnapshotTimezone::Local);
        assert!(name_local.starts_with("hourly.0__"));
        assert!(name_local.contains('+') || name_local.contains('-'));
        assert!(!name_local.contains(':'));
    }
}

