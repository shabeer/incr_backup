//! # Diff Module — Snapshot Comparison
//!
//! Compare two snapshots and show what changed (added, deleted, modified).
//! Pure manifest comparison — no network I/O after manifests are cached.

use log::info;

use crate::config::{Config, SnapshotTimezone};
use crate::error::Result;
use crate::manifest::{self, DiffResult, Manifest, format_size};
use crate::rclone::Rclone;

/// Options for the diff command.
#[derive(Debug, Clone)]
pub struct DiffOptions {
    /// First snapshot name (e.g., "hourly.0").
    pub snapshot_a: String,
    /// Second snapshot name (e.g., "daily.0").
    pub snapshot_b: String,
    /// Output as JSON instead of human-readable.
    pub json_output: bool,
    /// Show only added files.
    pub only_added: bool,
    /// Show only deleted files.
    pub only_deleted: bool,
    /// Show only modified files.
    pub only_modified: bool,
    /// Show summary counts/sizes only.
    pub stat_only: bool,
}

/// Run the diff command: compare two snapshots.
///
/// Loads manifests for both snapshots, computes the diff, and formats output.
pub fn run_diff(config: &Config, rclone: &Rclone, options: &DiffOptions) -> Result<String> {
    let remote_base = &config.remote_path;

    // Load manifest A
    let manifest_a = load_manifest(rclone, remote_base, &options.snapshot_a)?;
    // Load manifest B
    let manifest_b = load_manifest(rclone, remote_base, &options.snapshot_b)?;

    info!(
        "Comparing {} ({} files) ↔ {} ({} files)",
        options.snapshot_a,
        manifest_a.file_count,
        options.snapshot_b,
        manifest_b.file_count
    );

    // Compute diff using manifest module's diff_manifests: target (snapshot_b) vs base (snapshot_a)
    let diff = manifest::diff_manifests(&manifest_b, &manifest_a);

    // Format output
    if options.json_output {
        diff.to_json()
    } else {
        Ok(format_diff(&diff, options, &manifest_a, &manifest_b, config.timezone))
    }
}

/// Load a manifest from the remote by snapshot name.
fn load_manifest(rclone: &Rclone, remote_base: &str, snapshot: &str) -> Result<Manifest> {
    let manifest_path = format!("{}/.meta/manifests/{}.json", remote_base, snapshot);
    let json = rclone.cat(&manifest_path)?;
    Manifest::from_json(&json)
}

/// Format the diff result as human-readable text.
fn format_diff(
    diff: &DiffResult,
    options: &DiffOptions,
    manifest_a: &Manifest,
    manifest_b: &Manifest,
    timezone: SnapshotTimezone,
) -> String {
    let mut output = String::new();

    // Header
    output.push_str(&format!(
        "Comparing: {} ({}) ↔ {} ({})\n\n",
        options.snapshot_a,
        timezone.format_display_datetime(manifest_a.created_at),
        options.snapshot_b,
        timezone.format_display_datetime(manifest_b.created_at),
    ));

    if options.stat_only {
        // Summary only
        output.push_str(&format!(
            "Summary: {} added, {} deleted, {} modified, {} unchanged\n",
            diff.added.len(),
            diff.deleted.len(),
            diff.modified.len(),
            diff.unchanged
        ));
        output.push_str(&format!(
            "         New data: {} | Removed: {}\n",
            format_size(diff.added_bytes()),
            format_size(diff.deleted_bytes()),
        ));
        return output;
    }

    // Added files
    if !options.only_deleted && !options.only_modified && !diff.added.is_empty() {
        output.push_str(&format!("Added (in {}, not in {}):\n", options.snapshot_b, options.snapshot_a));
        for f in &diff.added {
            output.push_str(&format!("  + {:<50} {}\n", f.path, format_size(f.size)));
        }
        output.push('\n');
    }

    // Deleted files
    if !options.only_added && !options.only_modified && !diff.deleted.is_empty() {
        output.push_str(&format!("Deleted (in {}, not in {}):\n", options.snapshot_a, options.snapshot_b));
        for f in &diff.deleted {
            output.push_str(&format!("  - {:<50} {}\n", f.path, format_size(f.size)));
        }
        output.push('\n');
    }

    // Modified files
    if !options.only_added && !options.only_deleted && !diff.modified.is_empty() {
        output.push_str("Modified (same path, different content):\n");
        for (new_ver, old_ver) in &diff.modified {
            output.push_str(&format!(
                "  ~ {:<50} {} → {}   md5: {} → {}\n",
                new_ver.path,
                format_size(old_ver.size),
                format_size(new_ver.size),
                &old_ver.md5[..8.min(old_ver.md5.len())],
                &new_ver.md5[..8.min(new_ver.md5.len())],
            ));
        }
        output.push('\n');
    }

    // Summary
    output.push_str(&format!(
        "Summary: {} added, {} deleted, {} modified\n",
        diff.added.len(),
        diff.deleted.len(),
        diff.modified.len(),
    ));
    output.push_str(&format!(
        "         New data: {} | Removed: {}\n",
        format_size(diff.added_bytes()),
        format_size(diff.deleted_bytes()),
    ));

    output
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_diff_empty() {
        let diff = DiffResult::default();
        let options = DiffOptions {
            snapshot_a: "hourly.0".to_string(),
            snapshot_b: "hourly.1".to_string(),
            json_output: false,
            only_added: false,
            only_deleted: false,
            only_modified: false,
            stat_only: true,
        };
        let ma = Manifest::empty();
        let mb = Manifest::empty();

        let output = format_diff(&diff, &options, &ma, &mb, SnapshotTimezone::Utc);
        assert!(output.contains("0 added"));
        assert!(output.contains("0 deleted"));
        assert!(output.contains("0 modified"));
        assert!(output.contains("Comparing: hourly.0 ("));
        assert!(output.contains("Z) ↔ hourly.1 ("));

        let local_output = format_diff(&diff, &options, &ma, &mb, SnapshotTimezone::Local);
        assert!(local_output.contains('+') || local_output.contains('-'));
    }
}

