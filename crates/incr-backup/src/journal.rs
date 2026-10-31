//! # Journal Module — Write-Ahead Journal for Crash Recovery (Derived Layer)
//!
//! Tracks in-progress backup operations using the generic [`crate::core::journal`] WAL engine,
//! persisted to `.meta/journal.json` on the rclone remote.

use chrono::{DateTime, Utc};
use log::debug;
use serde::{Deserialize, Serialize};

pub use common::journal::{
    GenericJournalManager, InMemoryJournalBackend, JournalBackend, JournalError, JournalRecord,
    JournalStep, SimpleJournal, StepStatus,
};
use crate::error::{IncrError, Result};
use crate::rclone::Rclone;

// ── Data Structures ─────────────────────────────────────────────────────────

/// The complete journal for an in-progress backup sync operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    /// What operation is in progress (e.g., "sync", "init").
    pub operation: String,
    /// Which snapshot this sync targets (e.g., "2026-10-29T10-30Z").
    pub snapshot_name: String,
    /// When this operation started.
    pub started_at: DateTime<Utc>,
    /// The backup-dir being used (e.g., "diffs/2026-10-29T10-30Z").
    pub backup_dir: String,
    /// Ordered list of steps with their current status.
    pub steps: Vec<JournalStep>,
}

impl JournalRecord for Journal {
    fn steps(&self) -> &[JournalStep] {
        &self.steps
    }

    fn steps_mut(&mut self) -> &mut [JournalStep] {
        &mut self.steps
    }

    fn operation(&self) -> &str {
        &self.operation
    }

    fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }
}

// ── Rclone Backend ──────────────────────────────────────────────────────────

/// Rclone storage backend for remote journal files.
pub struct RcloneJournalBackend<'a> {
    rclone: &'a Rclone,
    journal_path: String,
}

impl<'a> RcloneJournalBackend<'a> {
    /// Create a new backend targeting `{remote_base}/.meta/journal.json`.
    pub fn new(rclone: &'a Rclone, remote_base: &str) -> Self {
        Self {
            rclone,
            journal_path: format!("{}/.meta/journal.json", remote_base),
        }
    }
}

impl<'a> JournalBackend for RcloneJournalBackend<'a> {
    fn read_journal(&self) -> std::result::Result<Option<String>, JournalError> {
        match self.rclone.cat(&self.journal_path) {
            Ok(contents) => Ok(Some(contents)),
            Err(_) => Ok(None),
        }
    }

    fn write_journal(&self, content: &str) -> std::result::Result<(), JournalError> {
        self.rclone
            .rcat(&self.journal_path, content)
            .map(|_| ())
            .map_err(|e| JournalError::Backend(e.to_string()))
    }

    fn delete_journal(&self) -> std::result::Result<(), JournalError> {
        match self.rclone.delete_file(&self.journal_path) {
            Ok(_) => Ok(()),
            Err(e) => {
                debug!("Journal cleanup: {e} (may already be deleted)");
                Ok(())
            }
        }
    }

    fn journal_identifier(&self) -> &str {
        &self.journal_path
    }
}

// ── JournalManager ─────────────────────────────────────────────────────────

/// Manages the remote journal file for backup crash-recovery.
pub struct JournalManager<'a> {
    inner: GenericJournalManager<RcloneJournalBackend<'a>, Journal>,
}

impl<'a> JournalManager<'a> {
    /// Create a new JournalManager.
    pub fn new(rclone: &'a Rclone, remote_base: impl Into<String>) -> Self {
        let backend = RcloneJournalBackend::new(rclone, &remote_base.into());
        Self {
            inner: GenericJournalManager::new(backend),
        }
    }

    /// Create a new journal and upload it to the remote.
    pub fn create_journal(
        &self,
        operation: &str,
        snapshot_name: &str,
        backup_dir: &str,
    ) -> Result<()> {
        let journal = Journal {
            operation: operation.to_string(),
            snapshot_name: snapshot_name.to_string(),
            started_at: Utc::now(),
            backup_dir: backup_dir.to_string(),
            steps: create_sync_steps(),
        };

        self.inner
            .save_new_journal(journal)
            .map_err(IncrError::from)?;
        log::info!("Journal created for {} snapshot {}", operation, snapshot_name);
        Ok(())
    }

    /// Load an existing journal from the remote.
    pub fn load_journal(&self) -> Result<Option<Journal>> {
        self.inner.load_journal().map_err(IncrError::from)
    }

    /// Update a step's status and flush the journal to the remote.
    pub fn update_step(&self, step_name: &str, new_status: StepStatus) -> Result<()> {
        self.inner
            .update_step(step_name, new_status)
            .map_err(IncrError::from)
    }

    /// Delete the journal file from the remote.
    pub fn cleanup(&self) -> Result<()> {
        self.inner.cleanup().map_err(IncrError::from)
    }

    /// Find the first step that needs to be executed (Running or Pending).
    pub fn find_resume_point(journal: &Journal) -> Option<&str> {
        GenericJournalManager::<RcloneJournalBackend<'a>, Journal>::find_resume_point(journal)
    }
}

/// Create the standard set of sync steps for a new backup journal.
pub fn create_sync_steps() -> Vec<JournalStep> {
    let step_names = [
        "acquire_lock",
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

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_journal_serialization() {
        let journal = Journal {
            operation: "sync".to_string(),
            snapshot_name: "2026-10-28T08-00Z".to_string(),
            started_at: Utc::now(),
            backup_dir: "diffs/2026-10-28T08-00Z".to_string(),
            steps: create_sync_steps(),
        };

        let json = serde_json::to_string_pretty(&journal).unwrap();
        assert!(json.contains("\"operation\": \"sync\""));
        assert!(json.contains("acquire_lock"));
        assert!(json.contains("rclone_sync"));

        // Roundtrip
        let restored: Journal = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.operation, "sync");
        assert_eq!(restored.steps.len(), 6);
        assert_eq!(restored.steps[0].name, "acquire_lock");
    }

    #[test]
    fn test_step_status_serialization() {
        let step: JournalStep = JournalStep {
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
        assert_eq!(steps.len(), 6);
        assert_eq!(steps[0].name, "acquire_lock");
        assert_eq!(steps[5].name, "release_lock");
        for step in &steps {
            assert_eq!(step.status, StepStatus::Pending);
        }
    }

    #[test]
    fn test_find_resume_point_all_done() {
        let journal = Journal {
            operation: "sync".to_string(),
            snapshot_name: "2026-10-28T08-00Z".to_string(),
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
            snapshot_name: "2026-10-28T08-00Z".to_string(),
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

        assert_eq!(JournalManager::find_resume_point(&journal), Some("step2"));
    }
}
