//! # Test Mocks Module
//!
//! Provides controllable mock backends for testing concurrency, failure injection,
//! and write-ahead log recovery without external network or subprocess calls.
//!
//! ## Usage Example
//! ```rust
//! use test_commons::mocks::MockStorage;
//!
//! let storage = MockStorage::new();
//! storage.set("key", "value");
//! assert_eq!(storage.get("key"), Some("value".to_string()));
//! storage.set_fail_writes(true);
//! assert!(storage.write("key", "new").is_err());
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Thread-safe in-memory key-value store with failure injection capabilities.
#[derive(Debug, Clone, Default)]
pub struct MockStorage {
    data: Arc<Mutex<HashMap<String, String>>>,
    fail_writes: Arc<Mutex<bool>>,
    fail_reads: Arc<Mutex<bool>>,
}

impl MockStorage {
    /// Create a new mock storage instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Read value for a given key.
    pub fn get(&self, key: &str) -> Option<String> {
        let guard = self.data.lock().unwrap();
        guard.get(key).cloned()
    }

    /// Set value for a given key unconditionally.
    pub fn set(&self, key: &str, value: &str) {
        let mut guard = self.data.lock().unwrap();
        guard.insert(key.to_string(), value.to_string());
    }

    /// Remove a key.
    pub fn remove(&self, key: &str) {
        let mut guard = self.data.lock().unwrap();
        guard.remove(key);
    }

    /// Set whether simulated write operations should fail.
    pub fn set_fail_writes(&self, fail: bool) {
        let mut guard = self.fail_writes.lock().unwrap();
        *guard = fail;
    }

    /// Set whether simulated read operations should fail.
    pub fn set_fail_reads(&self, fail: bool) {
        let mut guard = self.fail_reads.lock().unwrap();
        *guard = fail;
    }

    /// Simulated read with failure injection.
    pub fn read(&self, key: &str) -> Result<Option<String>, String> {
        if *self.fail_reads.lock().unwrap() {
            return Err("simulated read failure".to_string());
        }
        Ok(self.get(key))
    }

    /// Simulated write with failure injection.
    pub fn write(&self, key: &str, value: &str) -> Result<(), String> {
        if *self.fail_writes.lock().unwrap() {
            return Err("simulated write failure".to_string());
        }
        self.set(key, value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_storage_failure_injection() {
        let storage = MockStorage::new();
        assert!(storage.write("k", "v").is_ok());
        assert_eq!(storage.read("k").unwrap(), Some("v".to_string()));

        storage.set_fail_writes(true);
        assert!(storage.write("k2", "v2").is_err());

        storage.set_fail_reads(true);
        assert!(storage.read("k").is_err());
    }
}

