//! # Generic Formatting Module
//!
//! Provides utilities for human-readable byte sizing and string representations.
//!
//! ## Usage Example
//! ```rust
//! use common::format::format_size;
//!
//! assert_eq!(format_size(500), "500 B");
//! assert_eq!(format_size(1024), "1.0 KB");
//! assert_eq!(format_size(10 * 1024 * 1024), "10.0 MB");
//! ```

/// Format a byte count into a human-readable string with units (`B`, `KB`, `MB`, `GB`, `TB`).
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;

    let b = bytes as f64;
    if b >= TB {
        format!("{:.1} TB", b / TB)
    } else if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1048576), "1.0 MB");
        assert_eq!(format_size(1073741824), "1.0 GB");
        assert_eq!(format_size(1099511627776), "1.0 TB");
    }
}
