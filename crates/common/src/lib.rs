//! # Common — Reusable Foundation Library
//!
//! Project-agnostic foundational building blocks:
//! - [`time`]: Timezone management, datetime formatting, and global clock/timezone settings.
//! - [`lock`]: Advisory lock management (trait, staleness check, in-memory & file backends).
//! - [`journal`]: Write-Ahead Logging (WAL) state machine and crash-recovery journal manager.
//! - [`config`]: Layered configuration loader (file, environment variables, CLI overrides).
//! - [`subprocess`]: Configurable subprocess executor with extra flags, dry-run, and piped stdin.
//! - [`fs`]: Parallel filesystem scanning, chunked file hashing, and glob matching.
//! - [`format`]: Human-readable byte formatting.
//! - [`diff`]: Generic set/collection difference engine.

pub mod time;
pub mod lock;
pub mod journal;
pub mod config;
pub mod subprocess;
pub mod fs;
pub mod format;
pub mod diff;

