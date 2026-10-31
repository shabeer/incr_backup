//! # Generic Write-Ahead Log (WAL) / Journal Module
//!
//! Provides backend-agnostic Write-Ahead Logging for tracking multistep operations
//! and enabling automated crash-recovery and resumption.
//!
//! ## Usage Example
//! ```rust
//! use common::journal::{GenericJournalManager, InMemoryJournalBackend, JournalStep, SimpleJournal, StepStatus};
//! use chrono::Utc;
//!
//! // 1. Setup backend and manager
//! let backend = InMemoryJournalBackend::new("sample-task-wal");
//! let manager: GenericJournalManager<_, SimpleJournal> = GenericJournalManager::new(backend);
//!
//! // 2. Create and persist a new journal with pending steps
//! let journal = SimpleJournal {
//!     operation: "export_database".to_string(),
//!     identifier: "db_backup_001".to_string(),
//!     started_at: Utc::now(),
//!     steps: vec![
//!         JournalStep::new("dump_schema"),
//!         JournalStep::new("dump_tables"),
//!         JournalStep::new("upload_archive"),
//!     ],
//! };
//! manager.save_new_journal(journal).unwrap();
//!
//! // 3. Update step transitions
//! manager.update_step("dump_schema", StepStatus::Running).unwrap();
//! manager.update_step("dump_schema", StepStatus::Done).unwrap();
//!
//! // 4. Check resume point after a simulated crash
//! let loaded = manager.load_journal().unwrap().unwrap();
//! let resume_step = GenericJournalManager::<InMemoryJournalBackend, SimpleJournal>::find_resume_point(&loaded);
//! assert_eq!(resume_step, Some("dump_tables"));
//!
//! // 5. Cleanup when finished
//! manager.cleanup().unwrap();
//! assert!(manager.load_journal().unwrap().is_none());
//! ```

use chrono::{DateTime, Utc};
use log::{debug, info};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use thiserror::Error;

// ── Error Types ─────────────────────────────────────────────────────────────

/// Errors originating from journal and WAL operations.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum JournalError {
    /// Incomplete operation found on startup.
    #[error("incomplete operation found (started {started_at}): {operation} — resuming")]
    IncompleteJournal {
        operation: String,
        started_at: String,
    },

    /// Corrupted or invalid journal payload.
    #[error("corrupt journal file: {0}")]
    CorruptJournal(String),

    /// Journal storage backend operation failed.
    #[error("journal backend error: {0}")]
    Backend(String),

    /// Requested step not found in the journal.
    #[error("step '{0}' not found in journal")]
    StepNotFound(String),

    /// Attempted to update when no active journal was loaded.
    #[error("no active journal to update")]
    NoActiveJournal,
}

// ── Data Structures ─────────────────────────────────────────────────────────

/// Status of an individual journal step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    /// Step has not started yet.
    Pending,
    /// Step is currently executing.
    Running,
    /// Step completed successfully.
    Done,
    /// Step failed with an error message.
    Failed(String),
}

/// A single step in a journal workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalStep<T = serde_json::Value> {
    /// Step name (e.g. "acquire_lock", "sync_data", "commit").
    pub name: String,
    /// Current execution status.
    pub status: StepStatus,
    /// Optional structured details or metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<T>,
}

impl JournalStep<serde_json::Value> {
    /// Create a pending step with no details.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: StepStatus::Pending,
            details: None,
        }
    }
}

/// Contract for journal data representations.
pub trait JournalRecord: Serialize + for<'de> Deserialize<'de> + Clone {
    /// Slice of journal steps.
    fn steps(&self) -> &[JournalStep];

    /// Mutable slice of journal steps.
    fn steps_mut(&mut self) -> &mut [JournalStep];

    /// Name or type of the operation.
    fn operation(&self) -> &str;

    /// When the operation started.
    fn started_at(&self) -> DateTime<Utc>;
}

/// Simple generic journal record for general-purpose applications.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimpleJournal {
    pub operation: String,
    pub identifier: String,
    pub started_at: DateTime<Utc>,
    pub steps: Vec<JournalStep>,
}

impl JournalRecord for SimpleJournal {
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

// ── Backend Trait ───────────────────────────────────────────────────────────

/// Storage abstraction for persisting and reading the WAL / journal file.
pub trait JournalBackend {
    /// Read raw journal content. Returns `Ok(None)` if absent.
    fn read_journal(&self) -> Result<Option<String>, JournalError>;

    /// Persist updated journal content atomically.
    fn write_journal(&self, content: &str) -> Result<(), JournalError>;

    /// Delete the journal file upon task completion. Idempotent.
    fn delete_journal(&self) -> Result<(), JournalError>;

    /// Human-readable identifier or location.
    fn journal_identifier(&self) -> &str;
}

// ── Generic Journal Manager ─────────────────────────────────────────────────

/// Generic manager coordinating journal lifecycle, status updates, and crash resumption.
pub struct GenericJournalManager<B: JournalBackend, J: JournalRecord> {
    backend: B,
    journal: RefCell<Option<J>>,
}

impl<B: JournalBackend, J: JournalRecord> GenericJournalManager<B, J> {
    /// Create a new journal manager with the specified backend.
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            journal: RefCell::new(None),
        }
    }

    /// Access the underlying backend.
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Save a newly created journal to the backend and retain an in-memory copy.
    pub fn save_new_journal(&self, journal: J) -> Result<(), JournalError> {
        let json = serde_json::to_string_pretty(&journal)
            .map_err(|e| JournalError::Backend(format!("serialization error: {e}")))?;

        self.backend.write_journal(&json)?;
        *self.journal.borrow_mut() = Some(journal);
        Ok(())
    }

    /// Load an existing journal from the backend.
    pub fn load_journal(&self) -> Result<Option<J>, JournalError> {
        match self.backend.read_journal()? {
            Some(contents) => {
                let j: J = serde_json::from_str(&contents).map_err(|e| {
                    JournalError::CorruptJournal(format!(
                        "failed to parse journal: {e}. Contents: {contents}"
                    ))
                })?;
                *self.journal.borrow_mut() = Some(j.clone());
                Ok(Some(j))
            }
            None => Ok(None),
        }
    }

    /// Update a step's status in memory and persist immediately to the backend.
    pub fn update_step(&self, step_name: &str, new_status: StepStatus) -> Result<(), JournalError> {
        let mut journal_ref = self.journal.borrow_mut();
        let journal = journal_ref.as_mut().ok_or(JournalError::NoActiveJournal)?;

        let step = journal.steps_mut().iter_mut().find(|s| s.name == step_name);
        match step {
            Some(s) => {
                debug!("Journal: {} -> {:?}", step_name, new_status);
                s.status = new_status;
            }
            None => {
                return Err(JournalError::StepNotFound(step_name.to_string()));
            }
        }

        let json = serde_json::to_string_pretty(journal)
            .map_err(|e| JournalError::Backend(format!("serialization error: {e}")))?;

        self.backend.write_journal(&json)?;
        Ok(())
    }

    /// Delete the journal file after successful completion.
    pub fn cleanup(&self) -> Result<(), JournalError> {
        info!("Cleaning up journal at {}", self.backend.journal_identifier());
        self.backend.delete_journal()?;
        *self.journal.borrow_mut() = None;
        Ok(())
    }

    /// Find the first non-Done step (Running, Pending, or Failed) where resumption should begin.
    pub fn find_resume_point(journal: &J) -> Option<&str> {
        for step in journal.steps() {
            match step.status {
                StepStatus::Running | StepStatus::Pending | StepStatus::Failed(_) => {
                    return Some(&step.name);
                }
                StepStatus::Done => continue,
            }
        }
        None
    }
}

// ── In-Memory Backend (for Tests / Reusable Mock) ────────────────────────────

/// Thread-safe in-memory journal backend.
#[derive(Debug, Clone, Default)]
pub struct InMemoryJournalBackend {
    name: String,
    data: Arc<Mutex<Option<String>>>,
}

impl InMemoryJournalBackend {
    /// Create a new in-memory journal storage target.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            data: Arc::new(Mutex::new(None)),
        }
    }
}

impl JournalBackend for InMemoryJournalBackend {
    fn read_journal(&self) -> Result<Option<String>, JournalError> {
        let guard = self.data.lock().map_err(|e| JournalError::Backend(e.to_string()))?;
        Ok(guard.clone())
    }

    fn write_journal(&self, content: &str) -> Result<(), JournalError> {
        let mut guard = self.data.lock().map_err(|e| JournalError::Backend(e.to_string()))?;
        *guard = Some(content.to_string());
        Ok(())
    }

    fn delete_journal(&self) -> Result<(), JournalError> {
        let mut guard = self.data.lock().map_err(|e| JournalError::Backend(e.to_string()))?;
        *guard = None;
        Ok(())
    }

    fn journal_identifier(&self) -> &str {
        &self.name
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generic_journal_lifecycle() {
        let backend = InMemoryJournalBackend::new("test-journal");
        let mgr: GenericJournalManager<_, SimpleJournal> = GenericJournalManager::new(backend);

        assert!(mgr.load_journal().unwrap().is_none());

        let initial = SimpleJournal {
            operation: "test-op".to_string(),
            identifier: "task-1".to_string(),
            started_at: Utc::now(),
            steps: vec![
                JournalStep::new("step1"),
                JournalStep::new("step2"),
            ],
        };

        mgr.save_new_journal(initial.clone()).unwrap();

        let loaded = mgr.load_journal().unwrap().expect("journal exists");
        assert_eq!(loaded.steps.len(), 2);
        assert_eq!(GenericJournalManager::<InMemoryJournalBackend, SimpleJournal>::find_resume_point(&loaded), Some("step1"));

        // Update step 1 to Running then Done
        mgr.update_step("step1", StepStatus::Running).unwrap();
        mgr.update_step("step1", StepStatus::Done).unwrap();

        let loaded2 = mgr.load_journal().unwrap().unwrap();
        assert_eq!(GenericJournalManager::<InMemoryJournalBackend, SimpleJournal>::find_resume_point(&loaded2), Some("step2"));

        // Finish step 2
        mgr.update_step("step2", StepStatus::Done).unwrap();
        let loaded3 = mgr.load_journal().unwrap().unwrap();
        assert_eq!(GenericJournalManager::<InMemoryJournalBackend, SimpleJournal>::find_resume_point(&loaded3), None);

        // Cleanup
        mgr.cleanup().unwrap();
        assert!(mgr.load_journal().unwrap().is_none());
    }
}

