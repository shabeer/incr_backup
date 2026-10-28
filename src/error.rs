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

