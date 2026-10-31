//! # Isolated Temporary Workspace
//!
//! Provides an isolated temporary filesystem sandbox for integration and unit testing.
//!
//! ## Usage Example
//! ```rust
//! use test_commons::workspace::TempWorkspace;
//!
//! let ws = TempWorkspace::new();
//! let file_path = ws.write_text("sub/hello.txt", "hello world");
//! assert!(ws.exists("sub/hello.txt"));
//! assert_eq!(ws.read_text("sub/hello.txt").unwrap(), "hello world");
//! ws.delete_file("sub/hello.txt");
//! assert!(!ws.exists("sub/hello.txt"));
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Generic isolated temporary filesystem workspace for integration tests.
pub struct TempWorkspace {
    /// Underlying temporary directory which is deleted on drop.
    pub temp_dir: TempDir,
}

impl TempWorkspace {
    /// Create a new isolated workspace in a fresh temporary directory.
    pub fn new() -> Self {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        Self { temp_dir }
    }

    /// Root filesystem path of the temporary workspace.
    pub fn root(&self) -> &Path {
        self.temp_dir.path()
    }

    /// Resolve a relative path inside the workspace.
    pub fn path_of(&self, rel_path: impl AsRef<Path>) -> PathBuf {
        self.temp_dir.path().join(rel_path)
    }

    /// Create a subdirectory inside the workspace, creating parent directories as needed.
    pub fn create_dir(&self, rel_path: &str) -> PathBuf {
        let dir = self.path_of(rel_path);
        fs::create_dir_all(&dir).expect("failed to create directory");
        dir
    }

    /// Write binary content to a file, creating parent directories if needed.
    pub fn write_file(&self, rel_path: &str, content: &[u8]) -> PathBuf {
        let path = self.path_of(rel_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent directories");
        }
        fs::write(&path, content).expect("failed to write test file");
        path
    }

    /// Write text string to a file, creating parent directories if needed.
    pub fn write_text(&self, rel_path: &str, text: &str) -> PathBuf {
        self.write_file(rel_path, text.as_bytes())
    }

    /// Read raw file bytes from a path inside the workspace.
    pub fn read_file(&self, rel_path: &str) -> Option<Vec<u8>> {
        let path = self.path_of(rel_path);
        if path.exists() {
            Some(fs::read(path).expect("failed to read file"))
        } else {
            None
        }
    }

    /// Read file content as a UTF-8 string.
    pub fn read_text(&self, rel_path: &str) -> Option<String> {
        self.read_file(rel_path)
            .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
    }

    /// Delete a file inside the workspace if it exists.
    pub fn delete_file(&self, rel_path: &str) {
        let path = self.path_of(rel_path);
        if path.exists() {
            fs::remove_file(path).expect("failed to remove file");
        }
    }

    /// Check if a relative path exists inside the workspace.
    pub fn exists(&self, rel_path: &str) -> bool {
        self.path_of(rel_path).exists()
    }
}

impl Default for TempWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_operations() {
        let ws = TempWorkspace::new();
        let path = ws.write_text("a/b/test.txt", "content123");
        assert!(path.exists());
        assert_eq!(ws.read_text("a/b/test.txt").unwrap(), "content123");
        assert!(ws.exists("a/b/test.txt"));

        ws.delete_file("a/b/test.txt");
        assert!(!ws.exists("a/b/test.txt"));
    }
}

