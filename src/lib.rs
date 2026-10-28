//! # incr-backup — Library Root
//!
//! Re-exports all modules that compose the incr-backup crate.
//!
//! ## Analogy
//! Think of this as Java's top-level package declaration or Python's `__init__.py` —
//! it makes all sub-modules accessible to both the binary (`main.rs`) and external
//! consumers (tests, other crates).
//!
//! ## Architecture
//! ```text
//!   main.rs (CLI)
//!     └── lib.rs (this file)
//!           ├── config.rs     — TOML config + env vars + CLI overrides
//!           ├── manifest.rs   — Snapshot manifest CRUD + diffing
//!           ├── scanner.rs    — Local filesystem scan + parallel MD5 hashing
//!           ├── rclone.rs     — rclone subprocess wrapper
//!           ├── snapshot.rs   — Rotation, promotion, deletion logic
//!           ├── sync.rs       — Full sync pipeline orchestration
//!           ├── restore.rs    — Restore with location resolution
//!           ├── diff.rs       — Diff command (manifest comparison)
//!           ├── verify.rs     — Backup integrity verification (12 checks)
//!           ├── journal.rs    — Write-ahead journal for crash recovery
//!           ├── lock.rs       — Remote lock management
//!           ├── remove.rs     — Purge files/directories from all snapshots
//!           └── error.rs      — Error types (thiserror)
//! ```

pub mod config;
pub mod manifest;
pub mod scanner;
pub mod rclone;
pub mod snapshot;
pub mod sync;
pub mod restore;
pub mod diff;
pub mod verify;
pub mod journal;
pub mod lock;
pub mod remove;
pub mod error;
