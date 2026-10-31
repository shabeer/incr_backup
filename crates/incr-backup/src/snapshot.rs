//! # Snapshot Module — Discrete Snapshot Management
//!
//! Under Dynamic Snapshot Consolidation, every backup run creates an independent,
//! immutable, timestamped snapshot. There are no fixed retention slots or tier rotations.
//! Compaction and pruning are explicit operations performed by the consolidation engine.

use chrono::Utc;

use crate::config::SnapshotTimezone;

/// Generate a formatted snapshot name for a new backup run.
///
/// Uses the configured timezone to produce standard snapshot directory names,
/// e.g. `2026-10-29T04-28Z` or `2026-10-29T09-58+0530`.
pub fn format_snapshot_name(tz: SnapshotTimezone) -> String {
    tz.now_formatted()
}

/// Parse a snapshot timestamp string into a DateTime<Utc>.
///
/// Handles:
/// - Local with %z offset: "2026-10-29T09-49+0530"
/// - UTC with 'Z' suffix: "2026-10-29T04-28Z"
/// - Standard RFC3339: "2026-10-29T04:28:00Z"
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

    // 4. Try date-only (e.g., "2026-10-28")
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
    fn test_format_snapshot_name() {
        let name_utc = format_snapshot_name(SnapshotTimezone::Utc);
        assert!(name_utc.ends_with('Z'));
        assert!(!name_utc.contains(':'));

        let name_local = format_snapshot_name(SnapshotTimezone::Local);
        assert!(name_local.contains('+') || name_local.contains('-'));
        assert!(!name_local.contains(':'));
    }

    #[test]
    fn test_parse_snapshot_timestamp() {
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
}
