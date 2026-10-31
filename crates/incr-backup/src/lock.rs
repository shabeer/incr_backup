//! # Lock Module — Remote Lock Management (Derived Layer)
//!
//! Implements cloud remote advisory locking for `incr-backup` by adapting the
//! generic [`crate::core::lock`] engine with an [`RcloneLockBackend`].
//!
//! Stores lock files at `.meta/lock` on the configured remote storage.

use log::warn;

pub use common::lock::{
    get_hostname, is_stale, stale_minutes, GenericLockManager, InMemoryLockBackend, LockBackend,
    LockError, LockInfo,
};
use crate::config::SnapshotTimezone;
use crate::error::{IncrError, Result};
use crate::rclone::Rclone;

// ── Rclone Backend ──────────────────────────────────────────────────────────

/// Rclone storage backend for remote lock files.
pub struct RcloneLockBackend<'a> {
    rclone: &'a Rclone,
    lock_path: String,
}

impl<'a> RcloneLockBackend<'a> {
    /// Create a new backend targeting `{remote_base}/.meta/lock`.
    pub fn new(rclone: &'a Rclone, remote_base: &str) -> Self {
        Self {
            rclone,
            lock_path: format!("{}/.meta/lock", remote_base),
        }
    }
}

impl<'a> LockBackend for RcloneLockBackend<'a> {
    fn read_lock(&self) -> std::result::Result<Option<String>, LockError> {
        match self.rclone.cat(&self.lock_path) {
            Ok(contents) => Ok(Some(contents)),
            Err(_) => Ok(None),
        }
    }

    fn write_lock(&self, content: &str) -> std::result::Result<(), LockError> {
        self.rclone
            .rcat(&self.lock_path, content)
            .map(|_| ())
            .map_err(|e| LockError::Backend(e.to_string()))
    }

    fn delete_lock(&self) -> std::result::Result<(), LockError> {
        match self.rclone.delete_file(&self.lock_path) {
            Ok(_) => Ok(()),
            Err(e) => {
                warn!("Failed to release lock (will become stale): {e}");
                Ok(())
            }
        }
    }

    fn lock_identifier(&self) -> &str {
        &self.lock_path
    }
}

// ── Remote Lock Manager ─────────────────────────────────────────────────────

/// Remote lock manager for incr-backup operations.
pub struct LockManager<'a> {
    inner: GenericLockManager<RcloneLockBackend<'a>>,
}

impl<'a> LockManager<'a> {
    /// Create a new `LockManager`.
    pub fn new(
        rclone: &'a Rclone,
        remote_base: impl Into<String>,
        timezone: SnapshotTimezone,
    ) -> Self {
        let backend = RcloneLockBackend::new(rclone, &remote_base.into());
        Self {
            inner: GenericLockManager::new(backend, timezone),
        }
    }

    /// Try to acquire the remote lock.
    pub fn acquire(&self, force: bool) -> Result<()> {
        self.inner.acquire(force).map_err(IncrError::from)
    }

    /// Release the lock by deleting `.meta/lock`.
    pub fn release(&self) -> Result<()> {
        self.inner.release().map_err(IncrError::from)
    }

    /// Check the current lock status without acquiring or releasing.
    pub fn check(&self) -> Result<Option<LockInfo>> {
        self.inner.check().map_err(IncrError::from)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

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
    fn test_is_stale_exactly_at_boundary() {
        let lock = LockInfo {
            holder: "test".to_string(),
            pid: 1,
            started_at: Utc::now() - Duration::minutes(60),
            timeout_minutes: 60,
        };
        assert!(!is_stale(&lock));
    }

    #[test]
    fn test_get_hostname_returns_something() {
        let hostname = get_hostname();
        assert!(!hostname.is_empty());
    }

    #[test]
    fn test_lock_info_current() {
        let lock = LockInfo::current(30);
        assert!(!lock.holder.is_empty());
        assert!(lock.pid > 0);
        assert_eq!(lock.timeout_minutes, 30);
        let elapsed = Utc::now()
            .signed_duration_since(lock.started_at)
            .num_seconds();
        assert!(elapsed < 5, "Lock started_at should be within last 5 seconds");
    }
}
