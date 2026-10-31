//! # Generic Filesystem & Hashing Module
//!
//! Provides directory traversal, chunked file hashing, glob matching, and parallel scanning.
//!
//! ## Usage Example
//! ```rust
//! use common::fs::{hash_file, glob_match, matches_any_exclude, scan_directory};
//! use std::path::Path;
//!
//! // 1. Glob matching
//! assert!(glob_match("*.rs", "main.rs"));
//! assert!(matches_any_exclude("target/debug/build", &["target/**".to_string()]));
//!
//! // 2. Directory scanning with a mapping closure
//! let result = scan_directory(Path::new("."), &["target/**".to_string()], |path, size, md5, mtime| {
//!     format!("{}: {} bytes (md5: {})", path, size, md5)
//! });
//! assert!(result.is_ok());
//! ```

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use log::{debug, warn};
use md5::{Digest, Md5};
use rayon::prelude::*;
use thiserror::Error;
use walkdir::WalkDir;

/// Errors arising from directory scanning operations.
#[derive(Debug, Error)]
pub enum ScanError {
    /// The source path was not found.
    #[error("source directory not found: {0}")]
    NotFound(PathBuf),

    /// The source path is not a directory.
    #[error("source path is not a directory: {0}")]
    NotDirectory(PathBuf),

    /// Standard I/O error.
    #[error("filesystem scan I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Generic summary result of scanning a directory.
#[derive(Debug)]
pub struct GenericScanResult<T> {
    /// Scanned entries mapped by the user-provided closure.
    pub entries: Vec<T>,
    /// Total size in bytes of all scanned files.
    pub total_size: u64,
    /// Total number of files successfully scanned.
    pub file_count: usize,
    /// Duration of the scan.
    pub duration: Duration,
    /// Non-fatal warnings encountered (e.g., skipped symlinks, permission errors).
    pub errors: Vec<String>,
}

/// Scan a local directory tree and process each file in parallel using Rayon.
///
/// # Arguments
/// * `source_path` — Root directory to scan.
/// * `excludes` — Slice of glob patterns to exclude.
/// * `mapper` — Function `(rel_path, size, md5, mtime) -> T` mapping file metadata into `T`.
pub fn scan_directory<T, F>(
    source_path: &Path,
    excludes: &[String],
    mapper: F,
) -> Result<GenericScanResult<T>, ScanError>
where
    T: Send + Sync,
    F: Fn(&str, u64, &str, &str) -> T + Sync + Send,
{
    let start = Instant::now();

    if !source_path.exists() {
        return Err(ScanError::NotFound(source_path.to_path_buf()));
    }
    if !source_path.is_dir() {
        return Err(ScanError::NotDirectory(source_path.to_path_buf()));
    }

    debug!("Scanning directory: {}", source_path.display());

    // Step 1: Collect all file paths
    let mut file_paths: Vec<PathBuf> = Vec::new();
    let mut walk_errors: Vec<String> = Vec::new();

    for entry in WalkDir::new(source_path).follow_links(false) {
        match entry {
            Ok(entry) => {
                if entry.file_type().is_dir() {
                    continue;
                }

                if entry.file_type().is_symlink() {
                    warn!("Skipping symlink: {}", entry.path().display());
                    walk_errors.push(format!("skipped symlink: {}", entry.path().display()));
                    continue;
                }

                let rel_path = entry
                    .path()
                    .strip_prefix(source_path)
                    .unwrap_or(entry.path());
                let rel_str = rel_path.to_string_lossy();

                if matches_any_exclude(&rel_str, excludes) {
                    debug!("Excluding: {}", rel_str);
                    continue;
                }

                file_paths.push(entry.into_path());
            }
            Err(e) => {
                warn!("Error walking directory: {}", e);
                walk_errors.push(format!("walk error: {e}"));
            }
        }
    }

    // Step 2: Hash and process files in parallel
    let results: Vec<std::result::Result<(T, u64), String>> = file_paths
        .par_iter()
        .map(|path| {
            let rel_path = path
                .strip_prefix(source_path)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");

            let metadata = fs::metadata(path)
                .map_err(|e| format!("cannot read metadata for '{}': {e}", path.display()))?;

            let size = metadata.len();

            let mtime = metadata
                .modified()
                .map(|t| {
                    let dt: DateTime<Local> = t.into();
                    dt.to_rfc3339()
                })
                .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());

            let md5 = hash_file(path)
                .map_err(|e| format!("cannot hash '{}': {e}", path.display()))?;

            let mapped = mapper(&rel_path, size, &md5, &mtime);
            Ok((mapped, size))
        })
        .collect();

    // Step 3: Collect mapped results
    let mut entries: Vec<T> = Vec::with_capacity(results.len());
    let mut total_size: u64 = 0;
    let mut errors = walk_errors;

    for res in results {
        match res {
            Ok((entry, size)) => {
                entries.push(entry);
                total_size += size;
            }
            Err(e) => errors.push(e),
        }
    }

    let file_count = entries.len();
    let duration = start.elapsed();

    Ok(GenericScanResult {
        entries,
        total_size,
        file_count,
        duration,
        errors,
    })
}

/// Compute the MD5 hash of a file by streaming in 64 KB chunks.
pub fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Md5::new();
    let mut buffer = [0u8; 64 * 1024];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    let result = hasher.finalize();
    Ok(format!("{:x}", result))
}

/// Check if a relative path matches any glob pattern in the exclude list.
pub fn matches_any_exclude(path: &str, excludes: &[String]) -> bool {
    let path_norm = path.replace('\\', "/");
    excludes.iter().any(|pattern| {
        let pattern_norm = pattern.replace('\\', "/");
        glob_match(&pattern_norm, &path_norm)
            || (!pattern_norm.contains('/')
                && path_norm
                    .rsplit('/')
                    .next()
                    .map(|filename| glob_match(&pattern_norm, filename))
                    .unwrap_or(false))
    })
}

/// Match a simple glob pattern against a path string.
///
/// Supports:
/// - `*` matches anything except `/`
/// - `**` matches anything including `/`
/// - `?` matches a single non-`/` character
pub fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == text || pattern == "**" {
        return true;
    }

    if let Some(prefix) = pattern.strip_suffix("/**") {
        if text == prefix || text.starts_with(&format!("{}/", prefix)) {
            return true;
        }
    }

    if let Some(suffix) = pattern.strip_prefix("**/") {
        if text == suffix || text.ends_with(&format!("/{}", suffix)) {
            return true;
        }
        if let Some(sub_pattern) = suffix.strip_prefix('*') {
            if text.ends_with(sub_pattern) {
                return true;
            }
        }
    }

    if pattern.starts_with("*.") {
        let ext = &pattern[1..];
        if text.ends_with(ext) {
            return true;
        }
    }

    glob_match_recursive(pattern.as_bytes(), text.as_bytes())
}

fn glob_match_recursive(pattern: &[u8], text: &[u8]) -> bool {
    match (pattern.first(), text.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            if pattern.get(1) == Some(&b'*') {
                let rest_pattern = &pattern[2..];
                let rest_pattern = if rest_pattern.first() == Some(&b'/') {
                    &rest_pattern[1..]
                } else {
                    rest_pattern
                };

                for i in 0..=text.len() {
                    if glob_match_recursive(rest_pattern, &text[i..]) {
                        return true;
                    }
                }
                return false;
            }

            if text.first() == Some(&b'/') {
                return glob_match_recursive(&pattern[1..], text);
            }

            glob_match_recursive(&pattern[1..], text)
                || (!text.is_empty()
                    && text[0] != b'/'
                    && glob_match_recursive(pattern, &text[1..]))
        }
        (Some(b'?'), Some(&c)) if c != b'/' => {
            glob_match_recursive(&pattern[1..], &text[1..])
        }
        (Some(&p), Some(&t)) if p == t => {
            glob_match_recursive(&pattern[1..], &text[1..])
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_glob_matching() {
        assert!(glob_match("*.tmp", "foo.tmp"));
        assert!(!glob_match("*.tmp", "foo.txt"));
        assert!(glob_match("node_modules/**", "node_modules/pkg/index.js"));
        assert!(glob_match("**/*.log", "var/log/app.log"));
    }

    #[test]
    fn test_hash_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        fs::write(&file_path, b"hello world").unwrap();

        let hash = hash_file(&file_path).unwrap();
        assert_eq!(hash, "5eb63bbbe01eeed093cb22bb8f5acdc3");
    }

    #[test]
    fn test_scan_directory_basic() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join("file1.txt"), b"123").unwrap();
        fs::write(temp_dir.path().join("file2.log"), b"456").unwrap();

        let res = scan_directory(
            temp_dir.path(),
            &["*.log".to_string()],
            |path, size, _hash, _mtime| (path.to_string(), size),
        )
        .unwrap();

        assert_eq!(res.file_count, 1);
        assert_eq!(res.entries[0].0, "file1.txt");
        assert_eq!(res.total_size, 3);
    }
}
