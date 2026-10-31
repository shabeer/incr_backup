//! # Generic Layered Configuration Module
//!
//! Provides reusable abstractions and utilities for multi-tier configuration cascades:
//! 1. Base configuration file (TOML / JSON)
//! 2. Environment variable overrides (configurable naming)
//! 3. Command-line argument overrides
//! 4. Validation
//!
//! ## Usage Example
//! ```rust
//! use common::config::{LayeredConfig, read_env_var, read_env_var_parsed, ConfigError};
//!
//! struct AppConfig {
//!     host: String,
//!     port: u16,
//! }
//!
//! struct CliOpts {
//!     port: Option<u16>,
//! }
//!
//! impl LayeredConfig for AppConfig {
//!     type CliOverrides = CliOpts;
//!     type Error = ConfigError;
//!
//!     fn apply_env_overrides(&mut self) {
//!         if let Some(h) = read_env_var("APP_HOST") {
//!             self.host = h;
//!         }
//!         if let Some(p) = read_env_var_parsed::<u16>("APP_PORT") {
//!             self.port = p;
//!         }
//!     }
//!
//!     fn apply_cli_overrides(&mut self, overrides: &Self::CliOverrides) {
//!         if let Some(p) = overrides.port {
//!             self.port = p;
//!         }
//!     }
//!
//!     fn validate(&self) -> Result<(), Self::Error> {
//!         if self.port == 0 {
//!             return Err(ConfigError::Validation("port cannot be 0".to_string()));
//!         }
//!         Ok(())
//!     }
//! }
//! ```

use std::path::{Path, PathBuf};
use std::str::FromStr;
use thiserror::Error;

// ── Error Types ─────────────────────────────────────────────────────────────

/// Errors that can occur during configuration loading, parsing, or validation.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum ConfigError {
    /// Configuration file not found at the specified path.
    #[error("configuration file not found: {0}")]
    FileNotFound(PathBuf),

    /// File exists but could not be read.
    #[error("failed to read configuration file '{path}': {reason}")]
    ReadError { path: PathBuf, reason: String },

    /// Invalid TOML syntax or schema.
    #[error("invalid configuration: {0}")]
    InvalidContent(String),

    /// Specific field validation failure.
    #[error("invalid configuration field '{field}': {reason}")]
    InvalidField { field: String, reason: String },

    /// General validation failure.
    #[error("configuration validation error: {0}")]
    Validation(String),
}

// ── Layered Config Trait ────────────────────────────────────────────────────

/// Trait defining the lifecycle of layered configuration resolution.
pub trait LayeredConfig: Sized {
    /// Associated CLI overrides type.
    type CliOverrides;
    /// Error type for validation.
    type Error;

    /// Apply environment variable overrides in-place.
    fn apply_env_overrides(&mut self);

    /// Apply command-line argument overrides in-place.
    fn apply_cli_overrides(&mut self, overrides: &Self::CliOverrides);

    /// Validate the final resolved configuration state.
    fn validate(&self) -> Result<(), Self::Error>;
}

// ── Generic File & Env Helpers ──────────────────────────────────────────────

/// Read and deserialize a TOML file from disk.
pub fn load_toml_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ConfigError> {
    if !path.exists() {
        return Err(ConfigError::FileNotFound(path.to_path_buf()));
    }

    let contents = std::fs::read_to_string(path).map_err(|e| ConfigError::ReadError {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;

    toml::from_str(&contents).map_err(|e| ConfigError::InvalidContent(e.to_string()))
}

/// Look for a configuration file at an explicit path, or search default relative/home paths.
pub fn find_config_file(explicit_path: Option<&Path>, default_filename: &str) -> Option<PathBuf> {
    if let Some(path) = explicit_path {
        if path.exists() {
            return Some(path.to_path_buf());
        }
        return None;
    }

    // 1. Current working directory
    let current_dir_file = PathBuf::from(default_filename);
    if current_dir_file.exists() {
        return Some(current_dir_file);
    }

    // 2. User config directory (~/.config/...)
    if let Ok(home) = std::env::var("HOME") {
        let home_config = PathBuf::from(home).join(".config").join(default_filename);
        if home_config.exists() {
            return Some(home_config);
        }
    }

    None
}

/// Read an environment variable, returning `None` if unset or empty.
pub fn read_env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(|val| {
        let trimmed = val.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// Read an environment variable and parse it into type `T`.
pub fn read_env_var_parsed<T: FromStr>(name: &str) -> Option<T> {
    read_env_var(name).and_then(|val| val.parse::<T>().ok())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq, serde::Deserialize)]
    struct DummyConfig {
        host: String,
        port: u16,
    }

    #[test]
    fn test_load_toml_file_nonexistent() {
        let res: Result<DummyConfig, _> = load_toml_file(Path::new("non_existent_file_12345.toml"));
        assert!(matches!(res, Err(ConfigError::FileNotFound(_))));
    }

    #[test]
    fn test_read_env_var_empty() {
        unsafe {
            std::env::set_var("TEST_EMPTY_VAR", "   ");
        }
        assert_eq!(read_env_var("TEST_EMPTY_VAR"), None);
    }
}

