//! # Manifest Module
//!
//! Handles reading, writing, and comparing snapshot manifests.
//! Think of a manifest as a "table of contents" for a snapshot — it lists
//! every file with its path, size, MD5 hash, and modification time.
//!
//! ## Analogy
//! Like a Python dict mapping file paths to their metadata, serialized as JSON.
//! ```python
//! manifest = {
//!     "documents/report.pdf": {"size": 1048576, "md5": "abc123...", "mtime": "..."},
//!     "photos/cat.jpg":       {"size": 524288,  "md5": "def456...", "mtime": "..."},
//! }
//! ```

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{IncrError, Result};

// ─── Data Structures ────────────────────────────────────────────────────────

/// A single file's metadata in a manifest.
///
/// Represents one row in the manifest's "table of contents."
/// Think of it like a Java POJO / Python dataclass with four fields.
///
/// ## Fields
/// - `path` — relative path from source root (e.g., `"documents/report.pdf"`)
/// - `size` — file size in bytes
/// - `md5` — hex-encoded MD5 hash of file contents
/// - `mtime` — last modification time as ISO 8601 string with timezone
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Relative path from the source root (forward slashes).
    pub path: String,

    /// File size in bytes.
    pub size: u64,

    /// Hex-encoded MD5 hash (32 lowercase hex chars).
    pub md5: String,

    /// Last modification time in ISO 8601 format (e.g., "2026-10-28T08:30:00+05:30").
    pub mtime: String,
}

/// A snapshot manifest: the complete listing of files at a point in time.
///
/// Serialized as JSON and stored at `.meta/manifests/<snapshot>.json` on the
/// remote. Contains every file's path, size, hash, and mtime.
///
/// ## Key Properties
/// - `snapshot_name` identifies which snapshot this belongs to (e.g., `"hourly.0"`)
/// - `created_at` records when the manifest was generated
/// - `files` is the full file listing
/// - `total_size` and `file_count` are precomputed summaries
///
/// ## Analogy
/// Like a database table with one row per file, serialized as a JSON array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Which snapshot this manifest describes (e.g., "hourly.0", "current").
    pub snapshot_name: String,

    /// When this manifest was generated (UTC).
    pub created_at: DateTime<Utc>,

    /// Complete listing of every file in the snapshot.
    pub files: Vec<FileEntry>,

    /// Precomputed total size of all files in bytes.
    /// Stored for quick access without iterating `files`.
    pub total_size: u64,

    /// Precomputed file count.
    pub file_count: usize,
}

impl Manifest {
    /// Create a new manifest from a list of file entries.
    ///
    /// Automatically computes `total_size` and `file_count` from the entries.
    ///
    /// # Arguments
    /// * `snapshot_name` — Name like `"hourly.0"` or `"current"`
    /// * `files` — Complete file listing
    /// * `created_at` — When the snapshot was taken
    pub fn new(snapshot_name: &str, files: Vec<FileEntry>, created_at: &DateTime<Utc>) -> Self {
        // `iter().map().sum()` is like Python's `sum(f.size for f in files)`
        // or Java's `files.stream().mapToLong(f -> f.size).sum()`
        let total_size: u64 = files.iter().map(|f| f.size).sum();
        let file_count = files.len();

        Self {
            snapshot_name: snapshot_name.to_string(),
            created_at: *created_at,
            files,
            total_size,
            file_count,
        }
    }

    /// Create an empty manifest (used for dry-run mode).
    pub fn empty() -> Self {
        Self {
            snapshot_name: String::new(),
            created_at: Utc::now(),
            files: Vec::new(),
            total_size: 0,
            file_count: 0,
        }
    }

    /// Get the total size of all files. Convenience accessor.
    pub fn total_size(&self) -> u64 {
        self.total_size
    }

    /// Check if the manifest contains a file at the given path.
    ///
    /// Like Python's `path in manifest_dict` or Java's `map.containsKey(path)`.
    pub fn has_file(&self, path: &str) -> bool {
        self.files.iter().any(|f| f.path == path)
    }

    /// Get a file entry by path.
    ///
    /// Like Python's `manifest_dict.get(path)` or Java's `map.get(path)`.
    /// Returns `None` if the file is not in this manifest.
    pub fn get_file(&self, path: &str) -> Option<&FileEntry> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Serialize this manifest to pretty-printed JSON.
    ///
    /// Used when uploading to `.meta/manifests/` via `rclone rcat`.
    pub fn to_json(&self) -> Result<String> {
        let json = serde_json::to_string_pretty(self)?;
        Ok(json)
    }

    /// Deserialize a manifest from a JSON string.
    ///
    /// Used when reading manifests downloaded from the remote.
    pub fn from_json(data: &str) -> Result<Self> {
        let manifest: Manifest = serde_json::from_str(data).map_err(|e| {
            IncrError::InvalidManifest {
                path: "<inline>".to_string(),
                reason: format!("JSON parse error: {e}"),
            }
        })?;
        Ok(manifest)
    }

    /// Parse rclone's `lsjson` output into a manifest.
    ///
    /// rclone lsjson returns JSON like:
    /// ```json
    /// [{"Path":"documents/report.pdf","Name":"report.pdf","Size":1048576,
    ///   "ModTime":"2026-10-27T22:00:00.000000000Z","IsDir":false,
    ///   "Hashes":{"md5":"a1b2c3d4..."}}]
    /// ```
    ///
    /// We extract only files (not directories) and map to our `FileEntry` format.
    pub fn from_rclone_lsjson(
        json_output: &str,
        snapshot_name: &str,
        created_at: &DateTime<Utc>,
    ) -> Result<Self> {
        // Parse the rclone JSON output into our intermediate struct
        let entries: Vec<RcloneLsjsonEntry> =
            serde_json::from_str(json_output).map_err(|e| IncrError::InvalidManifest {
                path: snapshot_name.to_string(),
                reason: format!("failed to parse rclone lsjson output: {e}"),
            })?;

        // Filter to files only (skip directories) and convert to FileEntry
        // `iter().filter().map().collect()` is like Python's list comprehension:
        //   [FileEntry(...) for e in entries if not e.is_dir]
        let files: Vec<FileEntry> = entries
            .into_iter()
            .filter(|e| !e.is_dir)
            .map(|e| FileEntry {
                path: e.path,
                size: e.size,
                md5: e
                    .hashes
                    .as_ref()
                    .and_then(|h| h.md5.clone())
                    .unwrap_or_default(),
                mtime: e.mod_time,
            })
            .collect();

        Ok(Self::new(snapshot_name, files, created_at))
    }
}

/// Intermediate struct for deserializing rclone's lsjson output.
///
/// rclone uses PascalCase field names, so we use `#[serde(rename_all)]`.
/// Not public — this is an implementation detail of `from_rclone_lsjson`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RcloneLsjsonEntry {
    path: String,
    #[allow(dead_code)]
    name: Option<String>,
    #[serde(default)]
    size: u64,
    mod_time: String,
    #[serde(default)]
    is_dir: bool,
    hashes: Option<RcloneHashes>,
}

/// Hash values from rclone's lsjson output.
#[derive(Debug, Deserialize)]
struct RcloneHashes {
    md5: Option<String>,
}

// ─── Diff Types ─────────────────────────────────────────────────────────────

/// Result of comparing two manifests.
///
/// Like a "git diff --stat" between two snapshots — shows which files were
/// added, deleted, or modified.
///
/// ## Analogy
/// Like Python's `DeepDiff` or Java's diff utility — compares two collections
/// and categorizes the differences.
#[derive(Debug, Clone, Default)]
pub struct DiffResult {
    /// Files present in manifest A but not in manifest B.
    pub added: Vec<FileEntry>,

    /// Files present in manifest B but not in manifest A.
    pub deleted: Vec<FileEntry>,

    /// Files present in both but with different MD5 hashes.
    pub modified: Vec<(FileEntry, FileEntry)>,

    /// Count of files identical in both manifests.
    pub unchanged: usize,
}

impl DiffResult {
    /// Total number of changes (added + deleted + modified).
    pub fn total_changes(&self) -> usize {
        self.added.len() + self.deleted.len() + self.modified.len()
    }

    /// Whether the two manifests are identical.
    pub fn is_empty(&self) -> bool {
        self.total_changes() == 0
    }

    /// Total bytes of newly added files.
    pub fn added_bytes(&self) -> u64 {
        self.added.iter().map(|f| f.size).sum()
    }

    /// Total bytes of deleted files.
    pub fn deleted_bytes(&self) -> u64 {
        self.deleted.iter().map(|f| f.size).sum()
    }

    /// Serialize the diff to JSON.
    pub fn to_json(&self) -> Result<String> {
        // We create a serializable representation since DiffResult itself
        // contains tuples which aren't as clean in JSON.
        let json_repr = serde_json::json!({
            "added": self.added,
            "deleted": self.deleted,
            "modified": self.modified.iter().map(|(old, new)| {
                serde_json::json!({
                    "path": old.path,
                    "old_size": old.size,
                    "new_size": new.size,
                    "old_md5": old.md5,
                    "new_md5": new.md5,
                })
            }).collect::<Vec<_>>(),
            "unchanged": self.unchanged,
            "summary": {
                "added_count": self.added.len(),
                "deleted_count": self.deleted.len(),
                "modified_count": self.modified.len(),
                "unchanged_count": self.unchanged,
                "added_bytes": self.added_bytes(),
                "deleted_bytes": self.deleted_bytes(),
            }
        });

        Ok(serde_json::to_string_pretty(&json_repr)?)
    }
}

/// Compare two manifests, like a "git diff --stat" between snapshots.
///
/// ## Algorithm (similar to Python dict comparison)
/// ```python
/// map_a = {f.path: f for f in manifest_a.files}
/// map_b = {f.path: f for f in manifest_b.files}
/// added    = [f for p, f in map_a.items() if p not in map_b]
/// deleted  = [f for p, f in map_b.items() if p not in map_a]
/// modified = [(map_a[p], map_b[p]) for p in map_a if p in map_b and map_a[p].md5 != map_b[p].md5]
/// ```
///
/// ## Arguments
/// * `a` — The "newer" manifest (e.g., hourly.0)
/// * `b` — The "older" manifest (e.g., daily.0)
///
/// ## Returns
/// A `DiffResult` with categorized changes.
pub fn diff_manifests(a: &Manifest, b: &Manifest) -> DiffResult {
    // Build HashMaps for O(1) lookup — like Python's dict comprehension
    // `{f.path: f for f in manifest.files}`
    let map_a: HashMap<&str, &FileEntry> = a.files.iter().map(|f| (f.path.as_str(), f)).collect();
    let map_b: HashMap<&str, &FileEntry> = b.files.iter().map(|f| (f.path.as_str(), f)).collect();

    let mut result = DiffResult::default();

    // Check files in A against B
    for (path, entry_a) in &map_a {
        match map_b.get(path) {
            None => {
                // In A but not B → added
                result.added.push((*entry_a).clone());
            }
            Some(entry_b) if entry_a.md5 != entry_b.md5 => {
                // In both but different content → modified
                result
                    .modified
                    .push(((*entry_a).clone(), (*entry_b).clone()));
            }
            _ => {
                // Same content → unchanged
                result.unchanged += 1;
            }
        }
    }

    // Check files in B but not in A → deleted
    for (path, entry_b) in &map_b {
        if !map_a.contains_key(path) {
            result.deleted.push((*entry_b).clone());
        }
    }

    result
}

/// Format a file size in human-readable form.
///
/// Examples: "1.5 MB", "840.0 KB", "3.2 GB", "128 B"
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;

    let b = bytes as f64;
    if b >= TB {
        format!("{:.1} TB", b / TB)
    } else if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample_entry(path: &str, size: u64, md5: &str) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            size,
            md5: md5.to_string(),
            mtime: "2026-10-28T08:00:00Z".to_string(),
        }
    }

    #[test]
    fn test_manifest_new_computes_totals() {
        let files = vec![
            sample_entry("a.txt", 100, "aaa"),
            sample_entry("b.txt", 200, "bbb"),
            sample_entry("c.txt", 300, "ccc"),
        ];
        let now = Utc::now();
        let m = Manifest::new("hourly.0", files, &now);
        assert_eq!(m.total_size, 600);
        assert_eq!(m.file_count, 3);
        assert_eq!(m.snapshot_name, "hourly.0");
    }

    #[test]
    fn test_manifest_has_file() {
        let files = vec![sample_entry("docs/report.pdf", 1000, "abc")];
        let m = Manifest::new("test", files, &Utc::now());
        assert!(m.has_file("docs/report.pdf"));
        assert!(!m.has_file("nonexistent.txt"));
    }

    #[test]
    fn test_manifest_get_file() {
        let files = vec![sample_entry("docs/report.pdf", 1000, "abc")];
        let m = Manifest::new("test", files, &Utc::now());
        let entry = m.get_file("docs/report.pdf").unwrap();
        assert_eq!(entry.size, 1000);
        assert!(m.get_file("nope.txt").is_none());
    }

    #[test]
    fn test_manifest_json_roundtrip() {
        let files = vec![
            sample_entry("a.txt", 100, "aaa"),
            sample_entry("b.txt", 200, "bbb"),
        ];
        let now = Utc::now();
        let m = Manifest::new("hourly.0", files, &now);

        let json = m.to_json().unwrap();
        let restored = Manifest::from_json(&json).unwrap();

        assert_eq!(restored.snapshot_name, "hourly.0");
        assert_eq!(restored.file_count, 2);
        assert_eq!(restored.total_size, 300);
        assert_eq!(restored.files[0].path, "a.txt");
    }

    #[test]
    fn test_from_rclone_lsjson() {
        let json = r#"[
            {"Path":"documents/report.pdf","Name":"report.pdf","Size":1048576,
             "ModTime":"2026-10-27T22:00:00.000Z","IsDir":false,
             "Hashes":{"md5":"a1b2c3d4e5f6"}},
            {"Path":"photos/","Name":"photos","Size":0,
             "ModTime":"2026-10-27T22:00:00.000Z","IsDir":true},
            {"Path":"photos/cat.jpg","Name":"cat.jpg","Size":524288,
             "ModTime":"2026-10-27T21:00:00.000Z","IsDir":false,
             "Hashes":{"md5":"deadbeef1234"}}
        ]"#;

        let now = Utc::now();
        let m = Manifest::from_rclone_lsjson(json, "hourly.0", &now).unwrap();

        // Should only include files, not directories
        assert_eq!(m.file_count, 2);
        assert_eq!(m.files[0].path, "documents/report.pdf");
        assert_eq!(m.files[0].md5, "a1b2c3d4e5f6");
        assert_eq!(m.files[1].path, "photos/cat.jpg");
        assert_eq!(m.total_size, 1048576 + 524288);
    }

    #[test]
    fn test_diff_identical_manifests() {
        let files = vec![
            sample_entry("a.txt", 100, "aaa"),
            sample_entry("b.txt", 200, "bbb"),
        ];
        let now = Utc::now();
        let m1 = Manifest::new("a", files.clone(), &now);
        let m2 = Manifest::new("b", files, &now);

        let diff = diff_manifests(&m1, &m2);
        assert!(diff.is_empty());
        assert_eq!(diff.unchanged, 2);
    }

    #[test]
    fn test_diff_added_files() {
        let now = Utc::now();
        let m1 = Manifest::new(
            "new",
            vec![
                sample_entry("a.txt", 100, "aaa"),
                sample_entry("b.txt", 200, "bbb"),
                sample_entry("c.txt", 300, "ccc"), // new file
            ],
            &now,
        );
        let m2 = Manifest::new(
            "old",
            vec![
                sample_entry("a.txt", 100, "aaa"),
                sample_entry("b.txt", 200, "bbb"),
            ],
            &now,
        );

        let diff = diff_manifests(&m1, &m2);
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.added[0].path, "c.txt");
        assert_eq!(diff.deleted.len(), 0);
        assert_eq!(diff.modified.len(), 0);
        assert_eq!(diff.unchanged, 2);
    }

    #[test]
    fn test_diff_deleted_files() {
        let now = Utc::now();
        let m1 = Manifest::new("new", vec![sample_entry("a.txt", 100, "aaa")], &now);
        let m2 = Manifest::new(
            "old",
            vec![
                sample_entry("a.txt", 100, "aaa"),
                sample_entry("b.txt", 200, "bbb"), // deleted
            ],
            &now,
        );

        let diff = diff_manifests(&m1, &m2);
        assert_eq!(diff.added.len(), 0);
        assert_eq!(diff.deleted.len(), 1);
        assert_eq!(diff.deleted[0].path, "b.txt");
    }

    #[test]
    fn test_diff_modified_files() {
        let now = Utc::now();
        let m1 = Manifest::new(
            "new",
            vec![sample_entry("a.txt", 150, "new_hash")],
            &now,
        );
        let m2 = Manifest::new(
            "old",
            vec![sample_entry("a.txt", 100, "old_hash")],
            &now,
        );

        let diff = diff_manifests(&m1, &m2);
        assert_eq!(diff.modified.len(), 1);
        assert_eq!(diff.modified[0].0.md5, "new_hash"); // new version
        assert_eq!(diff.modified[0].1.md5, "old_hash"); // old version
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1_048_576), "1.0 MB");
        assert_eq!(format_size(1_073_741_824), "1.0 GB");
        assert_eq!(format_size(1_099_511_627_776), "1.0 TB");
    }

    #[test]
    fn test_diff_result_to_json() {
        let now = Utc::now();
        let m1 = Manifest::new(
            "new",
            vec![
                sample_entry("new.txt", 100, "aaa"),
                sample_entry("modified.txt", 200, "new_md5"),
            ],
            &now,
        );
        let m2 = Manifest::new(
            "old",
            vec![
                sample_entry("deleted.txt", 50, "ddd"),
                sample_entry("modified.txt", 180, "old_md5"),
            ],
            &now,
        );

        let diff = diff_manifests(&m1, &m2);
        let json = diff.to_json().unwrap();
        assert!(json.contains("\"added\""));
        assert!(json.contains("\"deleted\""));
        assert!(json.contains("\"modified\""));
    }
}
