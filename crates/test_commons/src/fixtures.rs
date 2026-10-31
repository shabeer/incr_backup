//! # Test Fixtures Module
//!
//! Provides utilities for generating sample files, nested directories, and binary payloads.
//!
//! ## Usage Example
//! ```rust
//! use test_commons::workspace::TempWorkspace;
//! use test_commons::fixtures::create_sample_tree;
//!
//! let ws = TempWorkspace::new();
//! create_sample_tree(&ws, "test_data");
//! assert!(ws.exists("test_data/file1.txt"));
//! assert!(ws.exists("test_data/docs/notes.md"));
//! ```

use crate::workspace::TempWorkspace;

/// Generate a standardized nested directory tree inside a workspace.
pub fn create_sample_tree(workspace: &TempWorkspace, prefix: &str) {
    workspace.write_text(&format!("{}/file1.txt", prefix), "contents of file 1");
    workspace.write_text(&format!("{}/file2.txt", prefix), "contents of file 2");
    workspace.write_text(&format!("{}/docs/notes.md", prefix), "# Sample Notes\nHello");
    workspace.write_file(
        &format!("{}/binary.dat", prefix),
        &[0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_sample_tree() {
        let ws = TempWorkspace::new();
        create_sample_tree(&ws, "sample");
        assert!(ws.exists("sample/file1.txt"));
        assert!(ws.exists("sample/docs/notes.md"));
        assert_eq!(
            ws.read_file("sample/binary.dat").unwrap(),
            vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03]
        );
    }
}

