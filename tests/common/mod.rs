//! Common test helpers for incr-backup integration tests.

use std::fs;
use std::path::PathBuf;
use assert_cmd::Command;
use tempfile::TempDir;

#[allow(dead_code)]
pub struct TestEnv {
    /// Temporary root directory keeping both source and remote.
    pub temp_dir: TempDir,
    pub source_path: PathBuf,
    pub remote_path: PathBuf,
}

#[allow(dead_code)]
impl TestEnv {
    /// Create a new isolated test environment.
    pub fn new() -> Self {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let source_path = temp_dir.path().join("source");
        let remote_path = temp_dir.path().join("remote");

        fs::create_dir_all(&source_path).expect("failed to create source dir");
        fs::create_dir_all(&remote_path).expect("failed to create remote dir");

        Self {
            temp_dir,
            source_path,
            remote_path,
        }
    }

    /// Remote path string suitable for rclone / CLI argument.
    pub fn remote_str(&self) -> String {
        self.remote_path.to_string_lossy().to_string()
    }

    /// Source path string suitable for CLI argument.
    pub fn source_str(&self) -> String {
        self.source_path.to_string_lossy().to_string()
    }

    /// Write a file into the source directory.
    pub fn write_source_file(&self, rel_path: &str, content: &[u8]) {
        let path = self.source_path.join(rel_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent dirs");
        }
        fs::write(path, content).expect("failed to write source file");
    }

    /// Write a file into the remote directory (e.g. for adopt tests).
    pub fn write_remote_file(&self, rel_path: &str, content: &[u8]) {
        let path = self.remote_path.join(rel_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent dirs");
        }
        fs::write(path, content).expect("failed to write remote file");
    }

    /// Delete a file from the source directory.
    pub fn delete_source_file(&self, rel_path: &str) {
        let path = self.source_path.join(rel_path);
        if path.exists() {
            fs::remove_file(path).expect("failed to remove source file");
        }
    }

    /// Build a Command for the incr-backup binary.
    pub fn command(&self) -> Command {
        let mut cmd = Command::cargo_bin("incr-backup").expect("binary incr-backup exists");
        cmd.env("RUSTC_WRAPPER", "");
        cmd
    }
}
