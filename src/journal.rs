//! # Journal Module — Write-Ahead Journal for Crash Recovery
//!
//! Tracks in-progress operations so they can be resumed after crashes.
//! The journal is stored at `.meta/journal.json` on the remote.
//!
//! ## Analogy
//! Like a database WAL (Write-Ahead Log) or Java's transaction journal:
//! before doing work, record what you're about to do. After a crash,
//! replay the log to complete or roll back.
//!
//! ## How It Works
//! 1. Before a sync starts, a journal is created listing all steps (pending).
//! 2. Each step transitions: Pending → Running → Done.
//! 3. On re-run after a crash, we find the first non-Done step and resume.
//! 4. After all steps complete, the journal is deleted.

use chrono::{DateTime, Utc};
use log::{debug, info};
use serde::{Deserialize, Serialize};

use crate::error::{IncrError, Result};
use crate::rclone::Rclone;

// ─── Data Structures ────────────────────────────────────────────────────────

/// Status of a single journal step.
///
/// Transitions: Pending → Running → Done (or Failed).
/// Like a Java enum for a state machine, or Python's Enum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    /// Step has not started yet.
    Pending,
    /// Step is currently executing (may have crashed mid-way).
    Running,
    /// Step completed successfully.
    Done,
    /// Step failed with an error message.
    Failed(String),
}

/// A single step in the journal.
///
/// Like a row in a task table: name, current status, and optional details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalStep {
    /// Step name (e.g., "acquire_lock", "rclone_sync", "generate_manifest").
    pub name: String,
    /// Current status of this step.
    pub status: StepStatus,
    /// Optional details (e.g., list of renames for rotate_snapshots).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// The complete journal: tracks an in-progress backup operation.
///
/// Stored as `.meta/journal.json` on the remote. If this file exists when
/// the tool starts, it means a previous run was interrupted and we should
/// resume from where it left off.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Journal {
    /// What operation is in progress (e.g., "sync", "init").
    pub operation: String,
    /// Which level this sync targets (e.g., "hourly", "daily").
    pub level: String,
    /// When this operation started.
    pub started_at: DateTime<Utc>,
    /// The backup-dir being used (e.g., "diffs/hourly.0__2026-10-28T08-00").
    pub backup_dir: String,
    /// Ordered list of steps with their current status.
    pub steps: Vec<JournalStep>,
}

// ─── JournalManager ─────────────────────────────────────────────────────────

/// Manages the remote journal file for crash-recovery.
///
/// Reads, creates, updates, and deletes the journal on the remote via rclone.
///
/// ## Usage
/// ```rust,ignore
/// let jm = JournalManager::new(&rclone, "remote:backup/myhost");
/// jm.create_journal("sync", "hourly", "diffs/hourly.0__...")?;
/// jm.update_step("rclone_sync", StepStatus::Running)?;
/// // ... do work ...
/// jm.update_step("rclone_sync", StepStatus::Done)?;
/// jm.cleanup()?; // delete journal when all done
/// ```
pub struct JournalManager<'a> {
    rclone: &'a Rclone,
    remote_base: String,
    /// In-memory copy of the journal. Updated on each step change and
    /// flushed to the remote.
    journal: std::cell::RefCell<Option<Journal>>,
}

impl<'a> JournalManager<'a> {
    /// Create a new JournalManager.
    ///
    /// # Arguments
    /// * `rclone` — Reference to the rclone subprocess wrapper
    /// * `remote_base` — Remote base path, e.g. `"gdrive:backup/myhost"`
    pub fn new(rclone: &'a Rclone, remote_base: impl Into<String>) -> Self {
        Self {
            rclone,
            remote_base: remote_base.into(),
            journal: std::cell::RefCell::new(None),
        }
    }

    /// Full remote path to the journal file.
    fn journal_path(&self) -> String {
        format!("{}/.meta/journal.json", self.remote_base)
    }

    /// Create a new journal and upload it to the remote.
    ///
    /// Sets up the standard sync steps: acquire_lock, rotate_snapshots,
    /// delete_oldest, rclone_sync, generate_manifest, upload_manifest,
    /// cleanup_journal, release_lock.
    pub fn create_journal(
        &self,
        operation: &str,
        level: &str,
        backup_dir: &str,
    ) -> Result<()> {
        let journal = Journal {
            operation: operation.to_string(),
            level: level.to_string(),
            started_at: Utc::now(),
            backup_dir: backup_dir.to_string(),
            steps: create_sync_steps(),
        };

        // Upload to remote
        let json = serde_json::to_string_pretty(&journal)?;
        self.rclone.rcat(&self.journal_path(), &json)?;

        // Keep in-memory copy
        *self.journal.borrow_mut() = Some(journal);

        info!("Journal created for {} at level {}", operation, level);
        Ok(())
    }

    /// Load an existing journal from the remote.
    ///
    /// Returns `Some(Journal)` if a journal file exists, `None` if not.
    /// Used on startup to detect and resume interrupted operations.
    pub fn load_journal(&self) -> Result<Option<Journal>> {
        match self.rclone.cat(&self.journal_path()) {
            Ok(contents) => {
                let journal: Journal = serde_json::from_str(&contents).map_err(|e| {
                    IncrError::CorruptJournal(format!(
                        "failed to parse journal: {e}. Contents: {contents}"
                    ))
                })?;
                *self.journal.borrow_mut() = Some(journal.clone());
                Ok(Some(journal))
            }
            Err(_) => {
                // No journal file → no interrupted operation
                Ok(None)
            }
        }
    }

    /// Update a step's status and flush the journal to the remote.
    ///
    /// Finds the step by name, updates its status, and re-uploads the journal.
    /// This is the core "write-ahead" mechanism: status is persisted to the
    /// remote BEFORE the step executes (Running) and AFTER it completes (Done).
    pub fn update_step(&self, step_name: &str, new_status: StepStatus) -> Result<()> {
        let mut journal_ref = self.journal.borrow_mut();
        let journal = journal_ref.as_mut().ok_or_else(|| {
            IncrError::CorruptJournal("no active journal to update".to_string())
        })?;

        // Find and update the step
        let step = journal.steps.iter_mut().find(|s| s.name == step_name);
        match step {
            Some(s) => {
                debug!("Journal: {} → {:?}", step_name, new_status);
                s.status = new_status;
            }
            None => {
                return Err(IncrError::CorruptJournal(format!(
                    "step '{}' not found in journal",
                    step_name
                )));
            }
        }

        // Flush to remote
        let json = serde_json::to_string_pretty(journal)?;
        self.rclone.rcat(&self.journal_path(), &json)?;

        Ok(())
    }

    /// Delete the journal file from the remote.
    ///
    /// Called when all steps are Done. After this, a new run will start fresh
    /// instead of resuming.
    pub fn cleanup(&self) -> Result<()> {
        info!("Cleaning up journal");
        match self.rclone.delete_file(&self.journal_path()) {
            Ok(_) => {}
            Err(e) => {
                // Log but don't fail — missing journal is fine
                debug!("Journal cleanup: {e} (may already be deleted)");
            }
        }
        *self.journal.borrow_mut() = None;
        Ok(())
    }

    /// Find the first step that needs to be executed (Running or Pending).
    ///
    /// Used during resume to determine where to pick up.
    pub fn find_resume_point(journal: &Journal) -> Option<&str> {
        for step in &journal.steps {
            match step.status {
                StepStatus::Running | StepStatus::Pending => {
                    return Some(&step.name);
                }
                StepStatus::Done => continue,
                StepStatus::Failed(_) => {
                    return Some(&step.name);
                }
            }
        }
        None // All done
    }
}

/// Create the standard set of sync steps for a new journal.
///
/// These are the steps that every sync operation goes through, in order.
fn create_sync_steps() -> Vec<JournalStep> {
    let step_names = [
        "acquire_lock",
        "rotate_snapshots",
        "delete_oldest",
        "rclone_sync",
        "generate_manifest",
        "upload_manifest",
        "cleanup_journal",
        "release_lock",
    ];

    step_names
        .iter()
        .map(|name| JournalStep {
            name: name.to_string(),
            status: StepStatus::Pending,
            details: None,
        })
        .collect()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_journal_serialization() {
        let journal = Journal {
            operation: "sync".to_string(),
            level: "hourly".to_string(),
            started_at: Utc::now(),
            backup_dir: "diffs/hourly.0__2026-10-28T08-00".to_string(),
            steps: create_sync_steps(),
        };

        let json = serde_json::to_string_pretty(&journal).unwrap();
        assert!(json.contains("\"operation\": \"sync\""));
        assert!(json.contains("acquire_lock"));
        assert!(json.contains("rclone_sync"));

        // Roundtrip
        let restored: Journal = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.operation, "sync");
        assert_eq!(restored.steps.len(), 8);
        assert_eq!(restored.steps[0].name, "acquire_lock");
    }

    #[test]
    fn test_step_status_serialization() {
        let step = JournalStep {
            name: "test".to_string(),
            status: StepStatus::Running,
            details: None,
        };
        let json = serde_json::to_string(&step).unwrap();
        let restored: JournalStep = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.status, StepStatus::Running);
    }

    #[test]
    fn test_create_sync_steps() {
        let steps = create_sync_steps();
        assert_eq!(steps.len(), 8);
        assert_eq!(steps[0].name, "acquire_lock");
        assert_eq!(steps[7].name, "release_lock");
        // All should start as Pending
        for step in &steps {
            assert_eq!(step.status, StepStatus::Pending);
        }
    }

    #[test]
    fn test_find_resume_point_all_done() {
        let journal = Journal {
            operation: "sync".to_string(),
            level: "hourly".to_string(),
            started_at: Utc::now(),
            backup_dir: "test".to_string(),
            steps: vec![
                JournalStep {
                    name: "step1".to_string(),
                    status: StepStatus::Done,
                    details: None,
                },
                JournalStep {
                    name: "step2".to_string(),
                    status: StepStatus::Done,
                    details: None,
                },
            ],
        };

        assert!(JournalManager::find_resume_point(&journal).is_none());
    }

    #[test]
    fn test_find_resume_point_partial() {
        let journal = Journal {
            operation: "sync".to_string(),
            level: "hourly".to_string(),
            started_at: Utc::now(),
            backup_dir: "test".to_string(),
            steps: vec![
                JournalStep {
                    name: "step1".to_string(),
                    status: StepStatus::Done,
                    details: None,
                },
                JournalStep {
                    name: "step2".to_string(),
                    status: StepStatus::Running,
                    details: None,
                },
                JournalStep {
                    name: "step3".to_string(),
                    status: StepStatus::Pending,
                    details: None,
                },
            ],
        };

        assert_eq!(
            JournalManager::find_resume_point(&journal),
            Some("step2")
        );
    }
}

