//! # Generic Lock Module
//!
//! Provides backend-agnostic distributed advisory locking.
//! Prevents concurrent operations by maintaining a lock file in a storage backend
//! (e.g. cloud remote, local filesystem, or in-memory map).
//!
//! ## Usage Example
//! ```rust
//! use common::lock::{GenericLockManager, InMemoryLockBackend};
//! use common::time::Timezone;
//!
//! // 1. Create a backend (e.g. in-memory for testing, or a remote backend)
//! let backend = InMemoryLockBackend::new("my-task-lock");
//!
//! // 2. Create the generic lock manager
//! let lock_mgr = GenericLockManager::new(backend, Timezone::Utc);
//!
//! // 3. Acquire lock (returns Err if already held and fresh)
//! lock_mgr.acquire(false).expect("should acquire fresh lock");
//!
//! // 4. Check status
//! assert!(lock_mgr.check().unwrap().is_some());
//!
//! // 5. Release lock when done
//! lock_mgr.release().expect("should release lock");
//! assert!(lock_mgr.check().unwrap().is_none());
//! ```

use chrono::{DateTime, Utc};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use thiserror::Error;

use crate::time::Timezone;

// ── Error Types ─────────────────────────────────────────────────────────────

/// Errors originating from lock operations.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum LockError {
    /// Another process currently holds a non-stale lock.
    #[error("backup locked by '{holder}' since {since} (use --force to override)")]
    Locked {
        /// Identifier of the process or machine holding the lock.
        holder: String,
        /// Formatted timestamp since when the lock has been held.
        since: String,
    },

    /// Lock file exists but is corrupted / unparseable.
    #[error("corrupt lock file: {0}")]
    CorruptLock(String),

    /// Lock backend I/O or storage operation failed.
    #[error("lock backend error: {0}")]
    Backend(String),
}

// ── Data Structures ─────────────────────────────────────────────────────────

/// Information stored in the lock file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockInfo {
    /// Hostname of the machine that acquired the lock.
    pub holder: String,

    /// Process ID of the process that holds the lock.
    pub pid: u32,

    /// When the lock was acquired (UTC).
    pub started_at: DateTime<Utc>,

    /// How many minutes before this lock is considered stale.
    #[serde(default = "default_timeout_minutes")]
    pub timeout_minutes: u32,
}

/// Default timeout for lock staleness: 60 minutes.
fn default_timeout_minutes() -> u32 {
    60
}

impl LockInfo {
    /// Create a new `LockInfo` for the current machine and process.
    pub fn current(timeout_minutes: u32) -> Self {
        Self {
            holder: get_hostname(),
            pid: std::process::id(),
            started_at: Utc::now(),
            timeout_minutes,
        }
    }
}

// ── Backend Trait ───────────────────────────────────────────────────────────

/// Storage abstraction for reading, writing, and deleting advisory lock payloads.
pub trait LockBackend {
    /// Read raw content of the lock file if present. Returns `Ok(None)` if no lock exists.
    fn read_lock(&self) -> Result<Option<String>, LockError>;

    /// Atomically write or overwrite the lock payload.
    fn write_lock(&self, content: &str) -> Result<(), LockError>;

    /// Delete the lock file. Idempotent — should succeed if the lock is already absent.
    fn delete_lock(&self) -> Result<(), LockError>;

    /// Human-readable identifier or path of the lock target (for logs and errors).
    fn lock_identifier(&self) -> &str;
}

// ── Generic Lock Manager ───────────────────────────────────────────────────

/// Generic advisory lock manager coordinating lock acquisition, expiration checks, and releases.
pub struct GenericLockManager<B: LockBackend> {
    backend: B,
    timezone: Timezone,
}

impl<B: LockBackend> GenericLockManager<B> {
    /// Create a new lock manager for the given backend and timezone.
    pub fn new(backend: B, timezone: Timezone) -> Self {
        Self { backend, timezone }
    }

    /// Access the underlying backend.
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Try to acquire the lock.
    ///
    /// # Algorithm
    /// 1. Attempt to read existing lock via backend.
    /// 2. If no lock exists -> write a new lock.
    /// 3. If lock exists and is fresh and `force=false` -> return `LockError::Locked`.
    /// 4. If lock exists and is stale or `force=true` -> warn and overwrite.
    pub fn acquire(&self, force: bool) -> Result<(), LockError> {
        let lock_id = self.backend.lock_identifier();

        match self.backend.read_lock()? {
            Some(contents) => {
                let existing: LockInfo = serde_json::from_str(&contents).map_err(|e| {
                    LockError::CorruptLock(format!(
                        "failed to parse lock file: {e}. Contents: {contents}"
                    ))
                })?;

                if force {
                    warn!(
                        "Force-breaking lock held by '{}' (PID {}) since {}",
                        existing.holder,
                        existing.pid,
                        self.timezone.format_message_datetime(existing.started_at)
                    );
                    self.write_lock()?;
                    Ok(())
                } else if is_stale(&existing) {
                    warn!(
                        "Breaking stale lock held by '{}' (PID {}) since {} ({} minutes old, timeout is {} minutes)",
                        existing.holder,
                        existing.pid,
                        self.timezone.format_message_datetime(existing.started_at),
                        stale_minutes(&existing),
                        existing.timeout_minutes
                    );
                    self.write_lock()?;
                    Ok(())
                } else {
                    Err(LockError::Locked {
                        holder: existing.holder,
                        since: self.timezone.format_rfc3339(existing.started_at),
                    })
                }
            }
            None => {
                info!("No existing lock found at {}, acquiring lock", lock_id);
                self.write_lock()?;
                Ok(())
            }
        }
    }

    /// Release the lock by deleting it via the backend.
    pub fn release(&self) -> Result<(), LockError> {
        info!("Releasing lock at {}", self.backend.lock_identifier());
        self.backend.delete_lock()
    }

    /// Check lock status without mutating it.
    pub fn check(&self) -> Result<Option<LockInfo>, LockError> {
        match self.backend.read_lock()? {
            Some(contents) => {
                let info: LockInfo = serde_json::from_str(&contents).map_err(|e| {
                    LockError::CorruptLock(format!(
                        "failed to parse lock file: {e}. Contents: {contents}"
                    ))
                })?;
                Ok(Some(info))
            }
            None => Ok(None),
        }
    }

    fn write_lock(&self) -> Result<(), LockError> {
        let lock_info = LockInfo::current(60);
        let json = serde_json::to_string_pretty(&lock_info)
            .map_err(|e| LockError::Backend(format!("serialization error: {e}")))?;

        info!(
            "Writing lock: holder={}, pid={}, started_at={}",
            lock_info.holder,
            lock_info.pid,
            self.timezone.format_message_datetime(lock_info.started_at)
        );

        self.backend.write_lock(&json)
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Check whether a lock is stale based on its `timeout_minutes`.
pub fn is_stale(lock_info: &LockInfo) -> bool {
    let elapsed = Utc::now()
        .signed_duration_since(lock_info.started_at)
        .num_minutes();
    elapsed > lock_info.timeout_minutes as i64
}

/// How many minutes old the lock is.
pub fn stale_minutes(lock_info: &LockInfo) -> i64 {
    Utc::now()
        .signed_duration_since(lock_info.started_at)
        .num_minutes()
}

/// Get the hostname of the current machine.
pub fn get_hostname() -> String {
    if let Ok(hostname) = std::env::var("HOSTNAME") {
        return hostname;
    }

    if let Ok(output) = std::process::Command::new("hostname").output()
        && output.status.success()
    {
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }

    "unknown".to_string()
}

// ── In-Memory Backend (for Tests / Reusable Mock) ────────────────────────────

/// Thread-safe in-memory lock backend.
#[derive(Debug, Clone, Default)]
pub struct InMemoryLockBackend {
    name: String,
    data: Arc<Mutex<Option<String>>>,
}

impl InMemoryLockBackend {
    /// Create a new in-memory lock target with the given identifier.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            data: Arc::new(Mutex::new(None)),
        }
    }
}

impl LockBackend for InMemoryLockBackend {
    fn read_lock(&self) -> Result<Option<String>, LockError> {
        let guard = self.data.lock().map_err(|e| LockError::Backend(e.to_string()))?;
        Ok(guard.clone())
    }

    fn write_lock(&self, content: &str) -> Result<(), LockError> {
        let mut guard = self.data.lock().map_err(|e| LockError::Backend(e.to_string()))?;
        *guard = Some(content.to_string());
        Ok(())
    }

    fn delete_lock(&self) -> Result<(), LockError> {
        let mut guard = self.data.lock().map_err(|e| LockError::Backend(e.to_string()))?;
        *guard = None;
        Ok(())
    }

    fn lock_identifier(&self) -> &str {
        &self.name
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_lock_info_serialization() {
        let lock = LockInfo {
            holder: "laptop.local".to_string(),
            pid: 12345,
            started_at: Utc::now(),
            timeout_minutes: 60,
        };
        let json = serde_json::to_string(&lock).unwrap();
        let parsed: LockInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.holder, "laptop.local");
        assert_eq!(parsed.pid, 12345);
        assert_eq!(parsed.timeout_minutes, 60);
    }

    #[test]
    fn test_lock_info_deserialization_without_timeout() {
        let json = r#"{"holder":"test","pid":1,"started_at":"2026-10-28T08:00:00Z"}"#;
        let lock: LockInfo = serde_json::from_str(json).unwrap();
        assert_eq!(lock.timeout_minutes, 60);
    }

    #[test]
    fn test_is_stale_fresh_lock() {
        let lock = LockInfo {
            holder: "test".to_string(),
            pid: 1,
            started_at: Utc::now(),
            timeout_minutes: 60,
        };
        assert!(!is_stale(&lock));
    }

    #[test]
    fn test_is_stale_old_lock() {
        let lock = LockInfo {
            holder: "test".to_string(),
            pid: 1,
            started_at: Utc::now() - Duration::hours(2),
            timeout_minutes: 60,
        };
        assert!(is_stale(&lock));
    }

    #[test]
    fn test_in_memory_lock_manager_lifecycle() {
        let backend = InMemoryLockBackend::new("test-lock");
        let mgr = GenericLockManager::new(backend, Timezone::Utc);

        assert!(mgr.check().unwrap().is_none());
        assert!(mgr.acquire(false).is_ok());
        assert!(mgr.check().unwrap().is_some());

        // Concurrently acquiring without force should fail
        let err = mgr.acquire(false).unwrap_err();
        match err {
            LockError::Locked { .. } => {}
            _ => panic!("Expected LockError::Locked, got {:?}", err),
        }

        // Force acquire should succeed
        assert!(mgr.acquire(true).is_ok());

        // Release
        assert!(mgr.release().is_ok());
        assert!(mgr.check().unwrap().is_none());
    }
}
