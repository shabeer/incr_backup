//! # Error Module
//!
//! Defines all error types used throughout the incr-backup tool.
//!
//! ## Analogy
//! Like a Java exception hierarchy — each variant is a specific error type.
//! Unlike Java, Rust errors are returned as values (`Result<T, E>`) not thrown,
//! so the compiler FORCES you to handle them.

use std::path::PathBuf;
use thiserror::Error;

/// Top-level error type for all incr-backup operations.
///
/// Each variant corresponds to a specific failure mode. The `#[error(...)]`
/// attribute auto-generates human-readable messages (like Java's `getMessage()`).
///
/// Uses `thiserror` crate — similar to Java's `@Override toString()` but automatic.
#[derive(Debug, Error)]
pub enum IncrError {
    // ── Configuration Errors ────────────────────────────────────────────

    /// Config file not found or unreadable.
    #[error("config error: {0}")]
    Config(String),

    /// Config file contains invalid TOML or missing required fields.
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    // ── rclone Errors ───────────────────────────────────────────────────

    /// rclone binary not found in PATH or at specified location.
    #[error("rclone not found at '{path}': {source}")]
    RcloneNotFound {
        path: String,
        source: std::io::Error,
    },

    /// rclone command exited with a non-zero status.
    ///
    /// Contains the full command string for debugging — like Java's
    /// `ProcessBuilder` failure with captured stderr.
    #[error("rclone command failed: {cmd} (exit {code})\nstderr: {stderr}")]
    RcloneFailed {
        cmd: String,
        code: i32,
        stderr: String,
    },

    /// rclone command timed out (not currently used, reserved for future).
    #[error("rclone command timed out after {seconds}s: {cmd}")]
    RcloneTimeout { cmd: String, seconds: u64 },

    // ── Lock Errors ─────────────────────────────────────────────────────

    /// Another backup process holds the lock.
    ///
    /// Like Java's `FileLockInterruptionException` — the lock file exists
    /// and is not stale.
    #[error("backup locked by '{holder}' since {since} (use --force to override)")]
    Locked { holder: String, since: String },

    /// Lock file exists but is corrupted / unparseable.
    #[error("corrupt lock file: {0}")]
    CorruptLock(String),

    // ── Journal Errors ──────────────────────────────────────────────────

    /// A previous operation was interrupted and left a journal.
    ///
    /// The tool should resume from where it left off.
    #[error("incomplete operation found (started {started_at}): {operation} — resuming")]
    IncompleteJournal {
        operation: String,
        started_at: String,
    },

    /// Journal file is corrupted.
    #[error("corrupt journal file: {0}")]
    CorruptJournal(String),

    // ── Manifest Errors ─────────────────────────────────────────────────

    /// Manifest file is missing when expected.
    #[error("manifest not found: {0}")]
    ManifestNotFound(String),

    /// Manifest JSON is malformed or missing required fields.
    #[error("invalid manifest '{path}': {reason}")]
    InvalidManifest { path: String, reason: String },

    // ── Init Errors ─────────────────────────────────────────────────────

    /// Remote already has incr-backup metadata (`.meta/` exists).
    #[error("remote '{remote}' already initialized (has .meta/). Use a different path or delete existing backup.")]
    AlreadyInitialized { remote: String },

    /// Remote contains files but is missing required backup structure and --adopt was not passed.
    #[error("remote '{remote}' contains existing files but is not an initialized backup namespace (missing required directories).\nHint: To adopt existing files into 'current/', re-run with '--adopt'.")]
    RemoteNotEmptyNeedsAdopt { remote: String },

    /// Remote already has a 'current/' directory without an active in-progress adopt journal.
    #[error("remote '{remote}' already contains 'current/' without an in-progress adopt journal.\nHint: If this is unmanaged data, please manually rename or delete 'current/' before retrying.")]
    AdoptCurrentAlreadyExists { remote: String },

    /// Remote files lack precomputed MD5 checksums and --download-to-hash was not passed.
    #[error("remote '{remote}' has {missing_count} files missing precomputed MD5 hashes.\nTo stream/download files to compute hashes, pass '--download-to-hash'.\nAlternatively, clear the remote and run standard 'init' and 'sync' from local source.")]
    MissingPrecomputedHashes {
        remote: String,
        missing_count: usize,
    },

    /// Source directory does not exist or is not readable.
    #[error("source directory not found: {0}")]
    SourceNotFound(PathBuf),

    /// Source path is not a directory.
    #[error("source path is not a directory: {0}")]
    SourceNotDirectory(PathBuf),

    // ── Snapshot Errors ─────────────────────────────────────────────────

    /// Referenced snapshot does not exist.
    #[error("snapshot not found: '{0}'. Use 'incr-backup list' to see available snapshots.")]
    SnapshotNotFound(String),

    /// Invalid snapshot name format.
    #[error("invalid snapshot name: '{name}'. Expected format: hourly.0, daily.2, weekly.1, etc.")]
    InvalidSnapshotName { name: String },

    // ── Compaction Errors ───────────────────────────────────────────────

    /// Timezone offset in CLI parameter does not match configured backup timezone.
    #[error("Timezone offset in {param} ({got}) does not match configured backup timezone ({expected}).")]
    TimezoneMismatch {
        param: String,
        got: String,
        expected: String,
    },

    /// No snapshots found within the specified date range.
    #[error("No snapshots found in range [{from}, {to}]")]
    NoSnapshotsInRange { from: String, to: String },

    /// General compaction error.
    #[error("compaction error: {0}")]
    CompactionError(String),

    // ── Remove Errors ───────────────────────────────────────────────────

    /// Invalid target path for remove operation.
    #[error("invalid remove target: {0}")]
    InvalidTarget(String),

    // ── Restore Errors ──────────────────────────────────────────────────

    /// Restore destination already contains files and --overwrite not specified.
    #[error("restore destination '{path}' is not empty. Use --overwrite to proceed.")]
    RestoreDestNotEmpty { path: PathBuf },

    /// Restore destination cannot be created.
    #[error("cannot create restore destination '{path}': {source}")]
    RestoreDestCreation {
        path: PathBuf,
        source: std::io::Error,
    },

    // ── I/O Errors ──────────────────────────────────────────────────────

    /// Generic I/O error (file read/write, etc.).
    ///
    /// `#[from]` auto-implements `From<std::io::Error>` — like Java's
    /// implicit exception wrapping in a catch block.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// TOML parsing error.
    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
}

impl From<common::time::TimeError> for IncrError {
    fn from(err: common::time::TimeError) -> Self {
        IncrError::InvalidConfig(err.to_string())
    }
}

impl From<common::lock::LockError> for IncrError {
    fn from(err: common::lock::LockError) -> Self {
        match err {
            common::lock::LockError::Locked { holder, since } => {
                IncrError::Locked { holder, since }
            }
            common::lock::LockError::CorruptLock(s) => IncrError::CorruptLock(s),
            common::lock::LockError::Backend(s) => IncrError::Config(s),
        }
    }
}

impl From<common::journal::JournalError> for IncrError {
    fn from(err: common::journal::JournalError) -> Self {
        match err {
            common::journal::JournalError::IncompleteJournal {
                operation,
                started_at,
            } => IncrError::IncompleteJournal {
                operation,
                started_at,
            },
            common::journal::JournalError::CorruptJournal(s) => IncrError::CorruptJournal(s),
            common::journal::JournalError::Backend(s) => IncrError::Config(s),
            common::journal::JournalError::StepNotFound(s) => {
                IncrError::CorruptJournal(format!("step '{s}' not found in journal"))
            }
            common::journal::JournalError::NoActiveJournal => {
                IncrError::CorruptJournal("no active journal to update".to_string())
            }
        }
    }
}

impl From<common::config::ConfigError> for IncrError {
    fn from(err: common::config::ConfigError) -> Self {
        match err {
            common::config::ConfigError::FileNotFound(path) => {
                IncrError::Config(format!("config file not found: {}", path.display()))
            }
            common::config::ConfigError::ReadError { path, reason } => {
                IncrError::Config(format!("cannot read '{}': {}", path.display(), reason))
            }
            common::config::ConfigError::InvalidContent(s) => IncrError::Config(s),
            common::config::ConfigError::InvalidField { field, reason } => {
                IncrError::InvalidConfig(format!("invalid field '{field}': {reason}"))
            }
            common::config::ConfigError::Validation(s) => IncrError::InvalidConfig(s),
        }
    }
}

impl From<common::subprocess::SubprocessError> for IncrError {
    fn from(err: common::subprocess::SubprocessError) -> Self {
        match err {
            common::subprocess::SubprocessError::NotFound { binary, source } => {
                IncrError::RcloneNotFound {
                    path: binary,
                    source,
                }
            }
            common::subprocess::SubprocessError::Failed { cmd, code, stderr } => {
                IncrError::RcloneFailed { cmd, code, stderr }
            }
            common::subprocess::SubprocessError::Io(e) => IncrError::Io(e),
        }
    }
}

impl From<common::fs::ScanError> for IncrError {
    fn from(err: common::fs::ScanError) -> Self {
        match err {
            common::fs::ScanError::NotFound(p) => IncrError::SourceNotFound(p),
            common::fs::ScanError::NotDirectory(p) => IncrError::SourceNotDirectory(p),
            common::fs::ScanError::Io(e) => IncrError::Io(e),
        }
    }
}

/// Result type alias used throughout the crate.
///
/// Like Java's generic `Result<T>` pattern — every function that can fail
/// returns `Result<T>` which is shorthand for `Result<T, IncrError>`.
///
/// Usage:
/// ```rust,ignore
/// fn load_config(path: &Path) -> Result<Config> { ... }
/// // Equivalent to: Result<Config, IncrError>
/// ```
pub type Result<T> = std::result::Result<T, IncrError>;

