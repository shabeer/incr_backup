//! # Scanner Module — Local File Scanner with Parallel MD5 Hashing (Derived Layer)
//!
//! Scans a local directory tree and computes MD5 hashes in parallel using
//! [`common::fs`], mapping each item into backup [`FileEntry`] records.

use std::path::Path;
use std::time::Duration;

use crate::error::{IncrError, Result};
use crate::manifest::FileEntry;
pub use common::fs::{hash_file as common_hash_file, matches_any_exclude};

/// Match a simple glob pattern against text string.
pub fn matches_glob(text: &str, pattern: &str) -> bool {
    common::fs::glob_match(pattern, text)
}

/// Result of scanning a local directory.
#[derive(Debug)]
pub struct ScanResult {
    /// Complete file listing with path, size, MD5, and mtime.
    pub files: Vec<FileEntry>,
    /// Total size of all scanned files in bytes.
    pub total_size: u64,
    /// Number of files successfully scanned.
    pub file_count: usize,
    /// How long the scan took (wall clock).
    pub scan_duration: Duration,
    /// Non-fatal errors encountered during scanning.
    pub errors: Vec<String>,
}

/// Scan a local directory and compute MD5 hashes in parallel.
pub fn scan_directory(source_path: &Path, excludes: &[String]) -> Result<ScanResult> {
    let res = common::fs::scan_directory(source_path, excludes, |rel_path, size, md5, mtime| {
        FileEntry {
            path: rel_path.to_string(),
            size,
            md5: md5.to_string(),
            mtime: mtime.to_string(),
        }
    })
    .map_err(IncrError::from)?;

    let mut files = res.entries;
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(ScanResult {
        files,
        total_size: res.total_size,
        file_count: res.file_count,
        scan_duration: res.duration,
        errors: res.errors,
    })
}

/// Compute the MD5 hash of a file, returning the hex-encoded digest.
pub fn hash_file(path: &Path) -> Result<String> {
    common_hash_file(path).map_err(IncrError::Io)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_hash_file_known_value() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, b"hello\n").unwrap();

        let hash = hash_file(&path).unwrap();
        assert_eq!(hash, "b1946ac92492d2347c6235b4d2611184");
    }

    #[test]
    fn test_hash_empty_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.txt");
        fs::write(&path, b"").unwrap();

        let hash = hash_file(&path).unwrap();
        assert_eq!(hash, "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn test_scan_directory_basic() {
        let dir = TempDir::new().unwrap();

        fs::write(dir.path().join("a.txt"), "hello").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/b.txt"), "world").unwrap();

        let result = scan_directory(dir.path(), &[]).unwrap();

        assert_eq!(result.file_count, 2);
        assert_eq!(result.total_size, 10);
        assert!(result.errors.is_empty());

        let paths: Vec<&str> = result.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["a.txt", "sub/b.txt"]);
    }

    #[test]
    fn test_scan_directory_with_excludes() {
        let dir = TempDir::new().unwrap();

        fs::write(dir.path().join("keep.txt"), "keep").unwrap();
        fs::write(dir.path().join("skip.tmp"), "skip").unwrap();
        fs::write(dir.path().join(".DS_Store"), "ds").unwrap();

        let excludes = vec!["*.tmp".to_string(), ".DS_Store".to_string()];
        let result = scan_directory(dir.path(), &excludes).unwrap();

        assert_eq!(result.file_count, 1);
        assert_eq!(result.files[0].path, "keep.txt");
    }

    #[test]
    fn test_scan_nonexistent_directory() {
        let result = scan_directory(Path::new("/nonexistent/path"), &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_glob_matching() {
        assert!(matches_glob("file.tmp", "*.tmp"));
        assert!(matches_glob(".DS_Store", ".DS_Store"));
        assert!(matches_glob("hello", "h?llo"));
        assert!(!matches_glob("file.txt", "*.tmp"));
        assert!(matches_glob("anything", "*"));
        assert!(!matches_glob("file.tmp", "*.txt"));
    }

    #[test]
    fn test_matches_any_exclude() {
        let excludes = vec!["*.tmp".to_string(), ".DS_Store".to_string()];

        assert!(matches_any_exclude("file.tmp", &excludes));
        assert!(matches_any_exclude("dir/file.tmp", &excludes));
        assert!(matches_any_exclude(".DS_Store", &excludes));
        assert!(matches_any_exclude("subdir/.DS_Store", &excludes));
        assert!(!matches_any_exclude("file.txt", &excludes));
    }
}
