//! # Lock Module — Remote Lock Management
//!
//! Prevents concurrent backup operations by maintaining a lock file at
//! `.meta/lock` on the rclone remote. Only one backup process can hold
//! the lock at a time.
//!
//! ## Analogy
//! Like Java's `FileLock` or Python's `fcntl.flock()`, but for a **remote**
//! cloud file. Since we can't use OS-level file locking on Google Drive,
//! we implement advisory locking via a JSON lock file.
//!
//! ## How It Works
//! 1. Before starting a backup, try to read `.meta/lock` from the remote.
//! 2. If no lock exists → write one with our hostname, PID, and timestamp.
//! 3. If a lock exists and is fresh → refuse (another process is running).
//! 4. If a lock exists but is stale (older than `timeout_minutes`) → warn and overwrite.
//! 5. On completion (or error), delete the lock file.
//!
//! ## Lock JSON Format
//! ```json
//! {"holder":"laptop.local","pid":12345,"started_at":"2026-10-28T08:00:00Z","timeout_minutes":60}
//! ```

use chrono::{DateTime, Utc};
use log::{info, warn};
use serde::{Deserialize, Serialize};

use crate::error::{IncrError, Result};
use crate::rclone::Rclone;

// ── Data Structures ─────────────────────────────────────────────────────────

/// Information stored in the remote lock file.
///
/// Serialized as JSON and uploaded to `.meta/lock` on the remote.
/// Contains enough info to identify who holds the lock and whether it's stale.
///
/// Like a Java `LockInfo` POJO or Python `@dataclass` — a plain data holder
/// with automatic JSON serialization via serde `#[derive(Serialize, Deserialize)]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    /// Hostname of the machine that acquired the lock.
    /// Used to identify which machine is running a backup.
    pub holder: String,

    /// Process ID of the backup process that holds the lock.
    /// Combined with `holder`, uniquely identifies the running process.
    pub pid: u32,

    /// When the lock was acquired (UTC).
    /// Used to compute staleness: if `Utc::now() - started_at > timeout_minutes`,
    /// the lock is considered stale (the process probably crashed).
    pub started_at: DateTime<Utc>,

    /// How many minutes before this lock is considered stale.
    /// Default is 60 minutes. If a backup takes longer, increase this.
    #[serde(default = "default_timeout_minutes")]
    pub timeout_minutes: u32,
}

/// Default timeout for lock staleness: 60 minutes.
///
/// Used by serde's `#[serde(default = "...")]` when the field is missing
/// in older lock files. Like Python's `dict.get("timeout_minutes", 60)`.
fn default_timeout_minutes() -> u32 {
    60
}

impl LockInfo {
    /// Create a new `LockInfo` for the current machine and process.
    ///
    /// Captures the current hostname, PID, and UTC timestamp.
    /// Like `new LockInfo(hostname, pid, Instant.now())` in Java.
    pub fn current(timeout_minutes: u32) -> Self {
        Self {
            holder: get_hostname(),
            pid: std::process::id(),
            started_at: Utc::now(),
            timeout_minutes,
        }
    }
}

// ── LockManager ─────────────────────────────────────────────────────────────

/// Manages the remote lock file to prevent concurrent backup operations.
///
/// Wraps an `Rclone` instance and provides acquire/release/check semantics
/// for the `.meta/lock` file on the remote.
///
/// ## Usage
/// ```rust,ignore
/// let lock_mgr = LockManager::new(&rclone, "remote:backup/myhost");
/// lock_mgr.acquire(false)?;   // Fails if another process holds the lock
/// // ... do backup work ...
/// lock_mgr.release()?;        // Always release when done
/// ```
///
/// ## Analogy
/// Like a Java `ReentrantLock` or Python's `threading.Lock()`, but distributed
/// across machines via a cloud file. The "lock" is just a JSON file whose
/// existence means "someone is running a backup."
pub struct LockManager<'a> {
    /// Reference to the rclone wrapper for remote operations.
    /// Uses a borrow (`&'a`) — we don't own the Rclone instance.
    /// Like Java's dependency injection: `LockManager(Rclone rclone)`.
    rclone: &'a Rclone,

    /// Base path on the remote, e.g. "remote:backup/myhost".
    /// The lock file lives at `{remote_base}/.meta/lock`.
    remote_base: String,
}

impl<'a> LockManager<'a> {
    /// Create a new `LockManager`.
    ///
    /// # Arguments
    /// * `rclone` — Reference to the rclone subprocess wrapper
    /// * `remote_base` — Remote base path, e.g. `"gdrive:backup/myhost"`
    pub fn new(rclone: &'a Rclone, remote_base: impl Into<String>) -> Self {
        Self {
            rclone,
            remote_base: remote_base.into(),
        }
    }

    /// Try to acquire the remote lock.
    ///
    /// # Algorithm
    /// 1. Read `.meta/lock` via `rclone.cat()`.
    /// 2. If no lock file exists → create one with current host/PID/timestamp.
    /// 3. If lock exists and is NOT stale → return `IncrError::Locked`.
    /// 4. If lock exists and IS stale → warn and overwrite.
    /// 5. If `force=true` → overwrite regardless of staleness.
    ///
    /// # Arguments
    /// * `force` — If true, break any existing lock unconditionally (like `--force` CLI flag).
    ///
    /// # Errors
    /// * `IncrError::Locked` — Another non-stale backup is running.
    /// * `IncrError::CorruptLock` — Lock file exists but can't be parsed.
    /// * Other I/O or rclone errors.
    ///
    /// # Analogy
    /// Like Java's `tryLock()` — returns immediately with success or failure,
    /// never blocks waiting for the lock.
    pub fn acquire(&self, force: bool) -> Result<()> {
        let lock_path = self.lock_path();

        // Step 1: Try to read the existing lock file.
        // `rclone.cat()` returns the file contents or an error if it doesn't exist.
        match self.rclone.cat(&lock_path) {
            Ok(contents) => {
                // Lock file exists — parse it.
                // `serde_json::from_str` is like Python's `json.loads()` or Java's `objectMapper.readValue()`.
                let existing: LockInfo = serde_json::from_str(&contents).map_err(|e| {
                    IncrError::CorruptLock(format!(
                        "failed to parse lock file: {e}. Contents: {contents}"
                    ))
                })?;

                if force {
                    // --force flag: break lock unconditionally.
                    warn!(
                        "Force-breaking lock held by '{}' (PID {}) since {}",
                        existing.holder, existing.pid, existing.started_at
                    );
                    self.write_lock()?;
                    Ok(())
                } else if is_stale(&existing) {
                    // Lock is stale — the holder probably crashed.
                    warn!(
                        "Breaking stale lock held by '{}' (PID {}) since {} ({} minutes old, timeout is {} minutes)",
                        existing.holder,
                        existing.pid,
                        existing.started_at,
                        stale_minutes(&existing),
                        existing.timeout_minutes
                    );
                    self.write_lock()?;
                    Ok(())
                } else {
                    // Lock is fresh — another backup is actually running.
                    Err(IncrError::Locked {
                        holder: existing.holder,
                        since: existing.started_at.to_rfc3339(),
                    })
                }
            }
            Err(_) => {
                // No lock file (or read error) → create one.
                // We treat any read error as "no lock" because the file might not exist.
                // This is safe: the worst case is two processes both think there's no lock
                // and both write one — but cloud storage isn't ACID anyway, so advisory
                // locking is best-effort.
                info!("No existing lock found, acquiring lock");
                self.write_lock()?;
                Ok(())
            }
        }
    }

    /// Release the lock by deleting the `.meta/lock` file from the remote.
    ///
    /// Should be called when the backup operation completes (success or error).
    /// Idempotent — safe to call even if the lock was already released or never acquired.
    ///
    /// # Analogy
    /// Like Java's `lock.unlock()` in a `finally` block, or Python's `lock.release()`.
    pub fn release(&self) -> Result<()> {
        let lock_path = self.lock_path();
        info!("Releasing lock at {}", lock_path);

        // `rclone.delete_file()` removes a single file from the remote.
        // If the file doesn't exist, rclone may return an error — we ignore it
        // because releasing a non-existent lock is a no-op.
        match self.rclone.delete_file(&lock_path) {
            Ok(_) => Ok(()),
            Err(e) => {
                // Log but don't fail — if we can't delete the lock, it'll
                // eventually become stale and get broken on the next run.
                warn!("Failed to release lock (will become stale): {e}");
                Ok(())
            }
        }
    }

    /// Check the current lock status without acquiring or releasing.
    ///
    /// Returns `Some(LockInfo)` if a lock file exists on the remote,
    /// `None` if no lock file is found.
    ///
    /// # Analogy
    /// Like checking `File.exists()` + reading its contents. Non-mutating.
    pub fn check(&self) -> Result<Option<LockInfo>> {
        let lock_path = self.lock_path();

        match self.rclone.cat(&lock_path) {
            Ok(contents) => {
                let lock_info: LockInfo =
                    serde_json::from_str(&contents).map_err(|e| {
                        IncrError::CorruptLock(format!(
                            "failed to parse lock file: {e}. Contents: {contents}"
                        ))
                    })?;
                Ok(Some(lock_info))
            }
            Err(_) => {
                // File doesn't exist or can't be read → no lock.
                Ok(None)
            }
        }
    }

    // ── Private Helpers ─────────────────────────────────────────────────

    /// Full remote path to the lock file.
    fn lock_path(&self) -> String {
        format!("{}/.meta/lock", self.remote_base)
    }

    /// Write a new lock file with current host/PID/timestamp.
    ///
    /// Uses `rclone.rcat()` which reads from stdin and writes to a remote file.
    /// This creates or overwrites the lock file atomically (as atomic as cloud
    /// storage allows).
    fn write_lock(&self) -> Result<()> {
        let lock_info = LockInfo::current(60);
        // `serde_json::to_string_pretty` is like Python's `json.dumps(obj, indent=2)`
        // or Java's `objectMapper.writerWithDefaultPrettyPrinter().writeValueAsString(obj)`.
        let json = serde_json::to_string_pretty(&lock_info)?;
        let lock_path = self.lock_path();

        info!(
            "Writing lock: holder={}, pid={}, started_at={}",
            lock_info.holder, lock_info.pid, lock_info.started_at
        );

        self.rclone.rcat(&lock_path, &json)?;
        Ok(())
    }
}

// ── Free Functions ──────────────────────────────────────────────────────────

/// Check whether a lock is stale (expired).
///
/// A lock is stale if it was acquired more than `timeout_minutes` ago.
/// Stale locks indicate the holder process probably crashed without releasing.
///
/// # Analogy
/// Like checking if a session token has expired:
/// `is_expired = (now - issued_at) > max_age`.
pub fn is_stale(lock_info: &LockInfo) -> bool {
    let elapsed = Utc::now()
        .signed_duration_since(lock_info.started_at)
        .num_minutes();
    // `num_minutes()` returns i64. We compare against timeout (u32 → i64).
    // Negative elapsed means the lock's timestamp is in the future (clock skew) — treat as not stale.
    elapsed > lock_info.timeout_minutes as i64
}

/// How many minutes old the lock is. Used for log messages.
fn stale_minutes(lock_info: &LockInfo) -> i64 {
    Utc::now()
        .signed_duration_since(lock_info.started_at)
        .num_minutes()
}

/// Get the hostname of the current machine.
///
/// Tries the `HOSTNAME` environment variable first, then falls back to
/// `gethostname()` system call, and finally to `"unknown"`.
///
/// # Analogy
/// Like Python's `socket.gethostname()` or Java's `InetAddress.getLocalHost().getHostName()`.
fn get_hostname() -> String {
    // Try environment variable first (works on most systems).
    if let Ok(hostname) = std::env::var("HOSTNAME") {
        return hostname;
    }

    // Fall back to the `hostname` command (portable across macOS/Linux).
    // `std::process::Command` is like Python's `subprocess.run()` or
    // Java's `Runtime.getRuntime().exec()`.
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
        // Simulates reading an old-format lock file without the timeout field.
        // `#[serde(default)]` should fill in 60.
        let json = r#"{"holder":"test","pid":1,"started_at":"2026-10-28T08:00:00Z"}"#;
        let lock: LockInfo = serde_json::from_str(json).unwrap();
        assert_eq!(lock.timeout_minutes, 60); // default value
    }

    #[test]
    fn test_is_stale_fresh_lock() {
        let lock = LockInfo {
            holder: "test".to_string(),
            pid: 1,
            started_at: Utc::now(), // just created
            timeout_minutes: 60,
        };
        assert!(!is_stale(&lock));
    }

    #[test]
    fn test_is_stale_old_lock() {
        let lock = LockInfo {
            holder: "test".to_string(),
            pid: 1,
            // Created 2 hours ago — well past the 60-minute timeout.
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
            // Exactly at the boundary — should not be stale (> not >=).
            started_at: Utc::now() - Duration::minutes(60),
            timeout_minutes: 60,
        };
        // At exactly 60 minutes, num_minutes() returns 60 which is NOT > 60,
        // so the lock is not stale. This is intentional: we give a small grace period.
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
        // started_at should be very recent
        let elapsed = Utc::now()
            .signed_duration_since(lock.started_at)
            .num_seconds();
        assert!(elapsed < 5, "Lock started_at should be within last 5 seconds");
    }
}
