//! # Configuration Module
//!
//! Loads, validates, and manages configuration for incr-backup.
//!
//! ## How Configuration Works
//! Configuration flows through a three-layer priority cascade (highest wins):
//!
//! 1. **TOML file** (`incr-backup.toml`) — base configuration
//! 2. **Environment variables** (`INCR_BACKUP_SOURCE`, etc.) — override TOML
//! 3. **CLI arguments** (`--source`, `--remote`, etc.) — override everything
//!
//! ## Analogy
//! Like Java's Spring configuration: properties file < environment < command line.
//! Or Python's `configparser` with `os.environ` fallbacks.
//!
//! ## Key Types
//! - [`Config`] — the full in-memory configuration (local use)
//! - [`RemoteConfig`] — the subset serialized to `.meta/config.json` on the remote
//! - [`RetentionConfig`] — how many snapshots to keep per tier
//! - [`RetentionPreset`] — named presets (default / aggressive / minimal)

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{IncrError, Result};

// ─── Retention Presets ──────────────────────────────────────────────────────

/// Named retention presets that map to specific snapshot-count configurations.
///
/// Use during `init` to quickly set retention policy without specifying
/// individual counts. Each preset balances history depth vs. storage cost.
///
/// Like a Java enum with a `toConfig()` method — each variant carries
/// predefined numeric values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionPreset {
    /// Balanced retention: 6 hourly, 7 daily, 4 weekly, 6 monthly.
    /// Covers ~6h + 1 week + 1 month + 6 months of history.
    Default,

    /// Maximum history: 12 hourly, 14 daily, 8 weekly, 12 monthly.
    /// Uses more remote storage but keeps deeper history.
    Aggressive,

    /// Space-saving: 3 hourly, 3 daily, 2 weekly, 3 monthly.
    /// Minimal footprint, less rollback depth.
    Minimal,
}

impl RetentionPreset {
    /// Convert this preset into a concrete [`RetentionConfig`] with numeric counts.
    ///
    /// # Example
    /// ```rust,ignore
    /// let retention = RetentionPreset::Default.to_retention();
    /// assert_eq!(retention.hourly, 6);
    /// ```
    pub fn to_retention(self) -> RetentionConfig {
        match self {
            // `match` is exhaustive — the compiler forces us to handle every variant.
            // Like Python's match/case (3.10+) but with compile-time completeness checks.
            RetentionPreset::Default => RetentionConfig {
                hourly: 6,
                daily: 7,
                weekly: 4,
                monthly: 6,
                min_interval_minutes: 30,
            },
            RetentionPreset::Aggressive => RetentionConfig {
                hourly: 12,
                daily: 14,
                weekly: 8,
                monthly: 12,
                min_interval_minutes: 30,
            },
            RetentionPreset::Minimal => RetentionConfig {
                hourly: 3,
                daily: 3,
                weekly: 2,
                monthly: 3,
                min_interval_minutes: 30,
            },
        }
    }

    /// Parse a preset name from a string (case-insensitive).
    ///
    /// Used when parsing CLI `--retention <PRESET>` argument.
    ///
    /// # Errors
    /// Returns [`IncrError::InvalidConfig`] if the string doesn't match any preset.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self> {
        // Like Python: {"default": ..., "aggressive": ..., "minimal": ...}.get(s.lower())
        match s.to_lowercase().as_str() {
            "default" => Ok(RetentionPreset::Default),
            "aggressive" => Ok(RetentionPreset::Aggressive),
            "minimal" => Ok(RetentionPreset::Minimal),
            _ => Err(IncrError::InvalidConfig(format!(
                "unknown retention preset '{}'. Expected: default, aggressive, minimal",
                s
            ))),
        }
    }
}

// ─── Retention Configuration ────────────────────────────────────────────────

/// How many snapshots to keep at each rotation tier.
///
/// Each field is the maximum number of snapshot slots for that tier.
/// When a tier is full and a new snapshot is created, the oldest is deleted.
///
/// `min_interval_minutes` controls the minimum gap between rotations at the
/// same level — runs within this window update the current snapshot in-place
/// instead of rotating (prevents rapid runs from filling all slots).
///
/// ## Analogy
/// Like rsnapshot's `retain` directives in `/etc/rsnapshot.conf`:
/// ```text
/// retain  hourly  6
/// retain  daily   7
/// retain  weekly  4
/// retain  monthly 6
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionConfig {
    /// Number of hourly snapshot slots (e.g., 6 = keep last 6 hourly snapshots).
    pub hourly: u32,

    /// Number of daily snapshot slots.
    pub daily: u32,

    /// Number of weekly snapshot slots.
    pub weekly: u32,

    /// Number of monthly snapshot slots.
    pub monthly: u32,

    /// Minimum minutes between rotations at the same level.
    /// If a snapshot was created less than this many minutes ago, the tool
    /// updates it in-place instead of rotating. Set to 0 for rsnapshot behavior
    /// (always rotate).
    pub min_interval_minutes: u32,
}

impl Default for RetentionConfig {
    /// Returns the "Default" preset: 6/7/4/6 with 30-minute minimum interval.
    fn default() -> Self {
        RetentionPreset::Default.to_retention()
    }
}

// ─── Rclone Configuration ───────────────────────────────────────────────────

/// Configuration for the rclone binary used as the transport layer.
///
/// All remote operations go through rclone subprocess calls. This struct
/// controls which binary is used and any extra flags to pass.
///
/// ## Analogy
/// Like configuring a `ProcessBuilder` (Java) or `subprocess.Popen` (Python)
/// with a specific executable path and default arguments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RcloneConfig {
    /// Path to the rclone binary. Defaults to `"rclone"` (found via PATH).
    /// Override with `--rclone-path` or `INCR_BACKUP_RCLONE_PATH`.
    #[serde(default = "default_rclone_path")]
    pub path: String,

    /// Extra flags appended to every rclone invocation.
    /// Example: `"--drive-acknowledge-abuse --transfers 8"`.
    /// Override with `--rclone-flags` or `INCR_BACKUP_RCLONE_FLAGS`.
    #[serde(default)]
    pub extra_flags: String,
}

/// Default rclone path — just "rclone", relying on PATH resolution.
fn default_rclone_path() -> String {
    "rclone".to_string()
}

impl Default for RcloneConfig {
    fn default() -> Self {
        Self {
            path: default_rclone_path(),
            extra_flags: String::new(),
        }
    }
}

// ─── Exclude Patterns ───────────────────────────────────────────────────────

/// File exclusion patterns passed to rclone's `--exclude` flag.
///
/// Each pattern follows rclone's filter syntax (glob-like).
/// Common patterns: `*.tmp`, `.DS_Store`, `thumbs.db`, `node_modules/**`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExcludeConfig {
    /// List of glob patterns to exclude from backup.
    /// Each pattern is passed as a separate `--exclude <PATTERN>` to rclone.
    #[serde(default)]
    pub patterns: Vec<String>,
}

// ─── TOML File Structures ───────────────────────────────────────────────────
// These mirror the TOML file layout. Serde deserializes TOML sections into
// nested structs — like Python's configparser sections or Java Properties groups.

/// Intermediate struct for serializing and deserializing the `[source]` TOML section.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TomlSource {
    path: String,
}

/// Intermediate struct for serializing and deserializing the `[remote]` TOML section.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TomlRemote {
    path: String,
}

/// Top-level TOML file structure matching `incr-backup.toml`.
///
/// Used for deserializing and serializing configuration files.
/// The `Option` fields handle missing TOML sections gracefully.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TomlConfig {
    source: Option<TomlSource>,
    remote: Option<TomlRemote>,
    retention: Option<RetentionConfig>,
    rclone: Option<RcloneConfig>,
    exclude: Option<ExcludeConfig>,
}

// ─── CLI Override Arguments ─────────────────────────────────────────────────

/// CLI and environment variable overrides for configuration.
///
/// Each field is `Option<T>` — `None` means "don't override, keep the value
/// from the TOML file or default."
///
/// ## Priority
/// CLI args > environment variables > TOML file > built-in defaults.
///
/// ## Analogy
/// Like Java's `Optional<String>` or Python's `kwargs.get("source", None)` —
/// only override if explicitly provided.
#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    /// Override source directory path.
    pub source: Option<String>,

    /// Override rclone remote path (e.g., `gdrive:backup/myhost`).
    pub remote: Option<String>,

    /// Override path to rclone binary.
    pub rclone_path: Option<String>,

    /// Override extra rclone flags.
    pub rclone_flags: Option<String>,

    /// Enable verbose output (maps to log level).
    pub verbose: Option<u8>,

    /// Enable dry-run mode (no mutations).
    pub dry_run: Option<bool>,
}

// ─── Main Config ────────────────────────────────────────────────────────────

/// Full in-memory configuration for incr-backup.
///
/// This is the "resolved" config after merging TOML file + env vars + CLI args.
/// All fields are concrete (no `Option`s) — missing values use defaults.
///
/// ## Lifecycle
/// 1. `Config::load(path)` — read TOML file into this struct
/// 2. `config.with_overrides(cli)` — layer on CLI/env overrides
/// 3. `config.validate()` — check for invalid values
/// 4. Pass to command handlers
///
/// ## Analogy
/// Like a Spring `@Configuration` bean or Python's `dataclass` holding
/// all resolved settings.
#[derive(Debug, Clone)]
pub struct Config {
    /// Local source directory to back up.
    pub source_path: String,

    /// Rclone remote path (e.g., `gdrive:backup/myhost`).
    pub remote_path: String,

    /// Retention policy: how many snapshots per tier.
    pub retention: RetentionConfig,

    /// Rclone binary configuration.
    pub rclone: RcloneConfig,

    /// File exclusion patterns.
    pub exclude: ExcludeConfig,

    /// Verbosity level (0=normal, 1=-v, 2=-vv, 3=-vvv).
    pub verbose: u8,

    /// Dry-run mode: show what would happen without making changes.
    pub dry_run: bool,
}

impl Config {
    /// Load configuration from a TOML file.
    ///
    /// Reads and parses the file at `path`. Missing optional sections
    /// (retention, rclone, exclude) use built-in defaults. The `source` and
    /// `remote` sections may be absent if they'll be provided via CLI/env later.
    ///
    /// # Errors
    /// - [`IncrError::Config`] if the file cannot be read
    /// - [`IncrError::Toml`] if the TOML is syntactically invalid
    ///
    /// # Example
    /// ```rust,ignore
    /// let config = Config::load(Path::new("incr-backup.toml"))?;
    /// ```
    pub fn load(path: &Path) -> Result<Self> {
        // `std::fs::read_to_string` reads the entire file into a String.
        // `map_err` converts the IO error into our custom error type —
        // like Java's catch(IOException e) { throw new ConfigException(...); }
        let content = std::fs::read_to_string(path).map_err(|e| {
            IncrError::Config(format!("cannot read config file '{}': {}", path.display(), e))
        })?;

        // `toml::from_str` deserializes TOML into our TomlConfig struct.
        // The `?` operator propagates any parse error as IncrError::Toml
        // (thanks to the `#[from]` attribute in error.rs).
        let toml_config: TomlConfig = toml::from_str(&content)?;

        // Convert the TOML-specific struct into our main Config.
        // `unwrap_or_default()` = Python's `dict.get(key, default_value)` —
        // use the parsed value if present, otherwise the Default impl.
        Ok(Config {
            source_path: toml_config
                .source
                .map(|s| s.path)
                .unwrap_or_default(),
            remote_path: toml_config
                .remote
                .map(|r| r.path)
                .unwrap_or_default(),
            retention: toml_config.retention.unwrap_or_default(),
            rclone: toml_config.rclone.unwrap_or_default(),
            exclude: toml_config.exclude.unwrap_or_default(),
            verbose: 0,
            dry_run: false,
        })
    }

    /// Create a default configuration (no TOML file).
    ///
    /// Useful when all settings come from CLI arguments and environment
    /// variables. Every field gets its built-in default.
    pub fn default_config() -> Self {
        Config {
            source_path: String::new(),
            remote_path: String::new(),
            retention: RetentionConfig::default(),
            rclone: RcloneConfig::default(),
            exclude: ExcludeConfig::default(),
            verbose: 0,
            dry_run: false,
        }
    }

    /// Apply CLI and environment variable overrides to this configuration.
    ///
    /// Overrides follow a priority cascade:
    /// 1. Check CLI argument (from `overrides`)
    /// 2. If CLI is `None`, check environment variable
    /// 3. If env var is unset, keep the existing TOML/default value
    ///
    /// Returns a new `Config` with overrides applied (consumes `self`).
    ///
    /// ## Analogy
    /// Like Python's `{**defaults, **env_vars, **cli_args}` dict merge, or
    /// Java's builder pattern: `config.withSource(cliSource).withRemote(cliRemote)`.
    pub fn with_overrides(mut self, overrides: &CliOverrides) -> Self {
        // For each field: CLI arg > env var > existing value.
        // `std::env::var("NAME").ok()` returns Option<String> — like Python's os.environ.get().

        // Source path
        if let Some(ref source) = overrides.source {
            self.source_path = source.clone();
        } else if let Ok(env_source) = std::env::var("INCR_BACKUP_SOURCE") {
            self.source_path = env_source;
        }

        // Remote path
        if let Some(ref remote) = overrides.remote {
            self.remote_path = remote.clone();
        } else if let Ok(env_remote) = std::env::var("INCR_BACKUP_REMOTE") {
            self.remote_path = env_remote;
        }

        // Rclone binary path
        if let Some(ref rclone_path) = overrides.rclone_path {
            self.rclone.path = rclone_path.clone();
        } else if let Ok(env_rclone) = std::env::var("INCR_BACKUP_RCLONE_PATH") {
            self.rclone.path = env_rclone;
        }

        // Rclone extra flags
        if let Some(ref rclone_flags) = overrides.rclone_flags {
            self.rclone.extra_flags = rclone_flags.clone();
        } else if let Ok(env_flags) = std::env::var("INCR_BACKUP_RCLONE_FLAGS") {
            self.rclone.extra_flags = env_flags;
        }

        // Verbosity
        if let Some(verbose) = overrides.verbose {
            self.verbose = verbose;
        }

        // Dry-run
        if let Some(dry_run) = overrides.dry_run {
            self.dry_run = dry_run;
        }

        self
    }

    /// Validate the configuration for logical correctness.
    ///
    /// Checks that required fields are set, retention counts are positive,
    /// and the source directory exists (if local validation is appropriate).
    ///
    /// Call this after `load()` + `with_overrides()` but before using the config.
    ///
    /// # Errors
    /// - [`IncrError::InvalidConfig`] for invalid retention, empty paths
    /// - [`IncrError::SourceNotFound`] if the source directory doesn't exist
    /// - [`IncrError::SourceNotDirectory`] if the source path isn't a directory
    pub fn validate(&self) -> Result<()> {
        // Check required fields are non-empty
        if self.source_path.is_empty() {
            return Err(IncrError::InvalidConfig(
                "source path is required. Set [source].path in config, --source flag, or INCR_BACKUP_SOURCE env var".to_string(),
            ));
        }

        if self.remote_path.is_empty() {
            return Err(IncrError::InvalidConfig(
                "remote path is required. Set [remote].path in config, --remote flag, or INCR_BACKUP_REMOTE env var".to_string(),
            ));
        }

        // Validate retention counts — each must be > 0.
        // Zero slots means that tier would never keep any snapshots,
        // which is almost certainly a user error.
        if self.retention.hourly == 0 {
            return Err(IncrError::InvalidConfig(
                "retention.hourly must be > 0".to_string(),
            ));
        }
        if self.retention.daily == 0 {
            return Err(IncrError::InvalidConfig(
                "retention.daily must be > 0".to_string(),
            ));
        }
        if self.retention.weekly == 0 {
            return Err(IncrError::InvalidConfig(
                "retention.weekly must be > 0".to_string(),
            ));
        }
        if self.retention.monthly == 0 {
            return Err(IncrError::InvalidConfig(
                "retention.monthly must be > 0".to_string(),
            ));
        }

        // Validate source directory exists and is a directory.
        // `PathBuf::from` converts a string to a path — like Java's `Paths.get()`.
        let source = PathBuf::from(&self.source_path);
        if !source.exists() {
            return Err(IncrError::SourceNotFound(source));
        }
        if !source.is_dir() {
            return Err(IncrError::SourceNotDirectory(source));
        }

        Ok(())
    }

    /// Convert this local config into a [`RemoteConfig`] for storage on the remote.
    ///
    /// The remote config is a subset of the full config — it stores the
    /// information needed to understand and manage the backup without
    /// access to the original TOML file.
    ///
    /// This is serialized to `.meta/config.json` on the remote.
    pub fn to_remote_config(&self) -> RemoteConfig {
        RemoteConfig {
            source_path: self.source_path.clone(),
            retention: self.retention.clone(),
            exclude_patterns: self.exclude.patterns.clone(),
        }
    }

    /// Convert this configuration into a TOML-formatted string.
    ///
    /// Useful for writing out a local configuration file during `config pull`.
    pub fn to_toml_string(&self) -> Result<String> {
        let toml_config = TomlConfig {
            source: if self.source_path.is_empty() {
                None
            } else {
                Some(TomlSource {
                    path: self.source_path.clone(),
                })
            },
            remote: if self.remote_path.is_empty() {
                None
            } else {
                Some(TomlRemote {
                    path: self.remote_path.clone(),
                })
            },
            retention: Some(self.retention.clone()),
            rclone: Some(self.rclone.clone()),
            exclude: if self.exclude.patterns.is_empty() {
                None
            } else {
                Some(self.exclude.clone())
            },
        };

        toml::to_string_pretty(&toml_config).map_err(|e| {
            IncrError::Config(format!("failed to serialize config to TOML: {e}"))
        })
    }
}

// ─── Remote Config (stored on remote) ───────────────────────────────────────

/// Configuration subset stored on the remote at `.meta/config.json`.
///
/// Contains the information needed to understand the backup structure
/// without access to the local TOML file. This is uploaded during `init`
/// and updated whenever retention or exclude settings change.
///
/// ## What's NOT stored remotely
/// - `rclone.path` — local binary path is irrelevant to the remote
/// - `verbose`, `dry_run` — runtime flags, not persistent config
///
/// ## Analogy
/// Like a database migration metadata table — stores enough info to
/// understand the schema without the original migration scripts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteConfig {
    /// The local source path that was backed up.
    /// Informational — helps identify what this backup contains.
    pub source_path: String,

    /// Retention policy governing snapshot rotation.
    pub retention: RetentionConfig,

    /// File exclusion patterns (same as local config).
    pub exclude_patterns: Vec<String>,
}

impl RemoteConfig {
    /// Serialize this config to a pretty-printed JSON string.
    ///
    /// Used when uploading to `.meta/config.json` via `rclone rcat`.
    ///
    /// # Errors
    /// Returns [`IncrError::Json`] if serialization fails (unlikely for this struct).
    pub fn to_json(&self) -> Result<String> {
        // `serde_json::to_string_pretty` = Python's `json.dumps(obj, indent=2)`
        let json = serde_json::to_string_pretty(self)?;
        Ok(json)
    }

    /// Deserialize a [`RemoteConfig`] from a JSON string.
    ///
    /// Used when reading `.meta/config.json` downloaded from the remote.
    ///
    /// # Errors
    /// Returns [`IncrError::Json`] if the JSON is malformed or missing required fields.
    pub fn from_json(data: &str) -> Result<Self> {
        // `serde_json::from_str` = Python's `json.loads(data)` with type checking.
        // Unlike Python dicts, this ensures every field exists and has the right type.
        let config: RemoteConfig = serde_json::from_str(data)?;
        Ok(config)
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Helper: write a TOML string to a temporary file and return the path.
    fn write_toml(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn test_retention_preset_default() {
        let r = RetentionPreset::Default.to_retention();
        assert_eq!(r.hourly, 6);
        assert_eq!(r.daily, 7);
        assert_eq!(r.weekly, 4);
        assert_eq!(r.monthly, 6);
        assert_eq!(r.min_interval_minutes, 30);
    }

    #[test]
    fn test_retention_preset_aggressive() {
        let r = RetentionPreset::Aggressive.to_retention();
        assert_eq!(r.hourly, 12);
        assert_eq!(r.daily, 14);
        assert_eq!(r.weekly, 8);
        assert_eq!(r.monthly, 12);
    }

    #[test]
    fn test_retention_preset_minimal() {
        let r = RetentionPreset::Minimal.to_retention();
        assert_eq!(r.hourly, 3);
        assert_eq!(r.daily, 3);
        assert_eq!(r.weekly, 2);
        assert_eq!(r.monthly, 3);
    }

    #[test]
    fn test_retention_preset_from_str() {
        assert_eq!(
            RetentionPreset::from_str("default").unwrap(),
            RetentionPreset::Default
        );
        assert_eq!(
            RetentionPreset::from_str("AGGRESSIVE").unwrap(),
            RetentionPreset::Aggressive
        );
        assert_eq!(
            RetentionPreset::from_str("Minimal").unwrap(),
            RetentionPreset::Minimal
        );
        assert!(RetentionPreset::from_str("unknown").is_err());
    }

    #[test]
    fn test_load_full_config() {
        let toml = r#"
[source]
path = "/data/important"

[remote]
path = "gdrive:backup/myhost"

[retention]
hourly = 8
daily = 10
weekly = 5
monthly = 8
min_interval_minutes = 15

[rclone]
path = "/usr/bin/rclone"
extra_flags = "--drive-acknowledge-abuse"

[exclude]
patterns = ["*.tmp", ".DS_Store"]
"#;
        let f = write_toml(toml);
        let config = Config::load(f.path()).unwrap();

        assert_eq!(config.source_path, "/data/important");
        assert_eq!(config.remote_path, "gdrive:backup/myhost");
        assert_eq!(config.retention.hourly, 8);
        assert_eq!(config.retention.daily, 10);
        assert_eq!(config.retention.weekly, 5);
        assert_eq!(config.retention.monthly, 8);
        assert_eq!(config.retention.min_interval_minutes, 15);
        assert_eq!(config.rclone.path, "/usr/bin/rclone");
        assert_eq!(config.rclone.extra_flags, "--drive-acknowledge-abuse");
        assert_eq!(config.exclude.patterns, vec!["*.tmp", ".DS_Store"]);
    }

    #[test]
    fn test_load_minimal_config() {
        // Only source and remote — everything else should default.
        let toml = r#"
[source]
path = "/data"

[remote]
path = "gdrive:bak"
"#;
        let f = write_toml(toml);
        let config = Config::load(f.path()).unwrap();

        assert_eq!(config.source_path, "/data");
        assert_eq!(config.remote_path, "gdrive:bak");
        // Defaults
        assert_eq!(config.retention.hourly, 6);
        assert_eq!(config.rclone.path, "rclone");
        assert!(config.exclude.patterns.is_empty());
    }

    #[test]
    fn test_load_empty_config() {
        // Completely empty TOML — all defaults, source/remote empty.
        let f = write_toml("");
        let config = Config::load(f.path()).unwrap();

        assert_eq!(config.source_path, "");
        assert_eq!(config.remote_path, "");
        assert_eq!(config.retention, RetentionConfig::default());
    }

    #[test]
    fn test_load_nonexistent_file() {
        let result = Config::load(Path::new("/nonexistent/path/config.toml"));
        assert!(result.is_err());
        // Should be a Config error, not an IO error
        match result.unwrap_err() {
            IncrError::Config(msg) => assert!(msg.contains("cannot read config file")),
            other => panic!("expected Config error, got: {:?}", other),
        }
    }

    #[test]
    fn test_load_invalid_toml() {
        let f = write_toml("this is not [valid toml =");
        let result = Config::load(f.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_with_overrides() {
        let config = Config::default_config();
        let overrides = CliOverrides {
            source: Some("/override/source".to_string()),
            remote: Some("gdrive:override".to_string()),
            rclone_path: Some("/opt/rclone".to_string()),
            rclone_flags: Some("--fast-list".to_string()),
            verbose: Some(2),
            dry_run: Some(true),
        };

        let config = config.with_overrides(&overrides);

        assert_eq!(config.source_path, "/override/source");
        assert_eq!(config.remote_path, "gdrive:override");
        assert_eq!(config.rclone.path, "/opt/rclone");
        assert_eq!(config.rclone.extra_flags, "--fast-list");
        assert_eq!(config.verbose, 2);
        assert!(config.dry_run);
    }

    #[test]
    fn test_with_partial_overrides() {
        let toml = r#"
[source]
path = "/original"

[remote]
path = "gdrive:original"
"#;
        let f = write_toml(toml);
        let config = Config::load(f.path()).unwrap();

        // Only override source, keep remote from TOML
        let overrides = CliOverrides {
            source: Some("/new/source".to_string()),
            ..Default::default()
        };

        let config = config.with_overrides(&overrides);

        assert_eq!(config.source_path, "/new/source");
        assert_eq!(config.remote_path, "gdrive:original"); // unchanged
    }

    #[test]
    fn test_validate_missing_source() {
        let config = Config {
            source_path: String::new(),
            remote_path: "gdrive:bak".to_string(),
            retention: RetentionConfig::default(),
            rclone: RcloneConfig::default(),
            exclude: ExcludeConfig::default(),
            verbose: 0,
            dry_run: false,
        };
        let result = config.validate();
        assert!(result.is_err());
        match result.unwrap_err() {
            IncrError::InvalidConfig(msg) => assert!(msg.contains("source path is required")),
            other => panic!("expected InvalidConfig, got: {:?}", other),
        }
    }

    #[test]
    fn test_validate_missing_remote() {
        let config = Config {
            source_path: "/tmp".to_string(),
            remote_path: String::new(),
            retention: RetentionConfig::default(),
            rclone: RcloneConfig::default(),
            exclude: ExcludeConfig::default(),
            verbose: 0,
            dry_run: false,
        };
        let result = config.validate();
        assert!(result.is_err());
        match result.unwrap_err() {
            IncrError::InvalidConfig(msg) => assert!(msg.contains("remote path is required")),
            other => panic!("expected InvalidConfig, got: {:?}", other),
        }
    }

    #[test]
    fn test_validate_zero_retention() {
        let config = Config {
            source_path: "/tmp".to_string(),
            remote_path: "gdrive:bak".to_string(),
            retention: RetentionConfig {
                hourly: 0,
                daily: 7,
                weekly: 4,
                monthly: 6,
                min_interval_minutes: 30,
            },
            rclone: RcloneConfig::default(),
            exclude: ExcludeConfig::default(),
            verbose: 0,
            dry_run: false,
        };
        let result = config.validate();
        assert!(result.is_err());
        match result.unwrap_err() {
            IncrError::InvalidConfig(msg) => assert!(msg.contains("hourly must be > 0")),
            other => panic!("expected InvalidConfig, got: {:?}", other),
        }
    }

    #[test]
    fn test_validate_source_not_found() {
        let config = Config {
            source_path: "/nonexistent/path/that/does/not/exist".to_string(),
            remote_path: "gdrive:bak".to_string(),
            retention: RetentionConfig::default(),
            rclone: RcloneConfig::default(),
            exclude: ExcludeConfig::default(),
            verbose: 0,
            dry_run: false,
        };
        let result = config.validate();
        assert!(result.is_err());
        match result.unwrap_err() {
            IncrError::SourceNotFound(_) => {} // expected
            other => panic!("expected SourceNotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_validate_valid_config() {
        // Use /tmp as source since it always exists
        let config = Config {
            source_path: "/tmp".to_string(),
            remote_path: "gdrive:bak".to_string(),
            retention: RetentionConfig::default(),
            rclone: RcloneConfig::default(),
            exclude: ExcludeConfig::default(),
            verbose: 0,
            dry_run: false,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_to_remote_config() {
        let config = Config {
            source_path: "/data/important".to_string(),
            remote_path: "gdrive:backup/myhost".to_string(),
            retention: RetentionConfig {
                hourly: 6,
                daily: 7,
                weekly: 4,
                monthly: 6,
                min_interval_minutes: 30,
            },
            rclone: RcloneConfig::default(),
            exclude: ExcludeConfig {
                patterns: vec!["*.tmp".to_string(), ".DS_Store".to_string()],
            },
            verbose: 2,
            dry_run: true,
        };

        let remote = config.to_remote_config();

        assert_eq!(remote.source_path, "/data/important");
        assert_eq!(remote.retention.hourly, 6);
        assert_eq!(remote.exclude_patterns, vec!["*.tmp", ".DS_Store"]);
    }

    #[test]
    fn test_remote_config_json_roundtrip() {
        let remote = RemoteConfig {
            source_path: "/data/important".to_string(),
            retention: RetentionConfig::default(),
            exclude_patterns: vec!["*.tmp".to_string()],
        };

        // Serialize to JSON
        let json = remote.to_json().unwrap();
        assert!(json.contains("source_path"));
        assert!(json.contains("/data/important"));

        // Deserialize back
        let restored = RemoteConfig::from_json(&json).unwrap();
        assert_eq!(restored.source_path, remote.source_path);
        assert_eq!(restored.retention.hourly, remote.retention.hourly);
        assert_eq!(restored.exclude_patterns, remote.exclude_patterns);
    }

    #[test]
    fn test_remote_config_from_invalid_json() {
        let result = RemoteConfig::from_json("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_config_to_toml_string() {
        let config = Config {
            source_path: "/data/docs".to_string(),
            remote_path: "gdrive:backups/host1".to_string(),
            retention: RetentionConfig {
                hourly: 3,
                daily: 3,
                weekly: 2,
                monthly: 3,
                min_interval_minutes: 30,
            },
            rclone: RcloneConfig::default(),
            exclude: ExcludeConfig {
                patterns: vec!["*.tmp".to_string()],
            },
            verbose: 0,
            dry_run: false,
        };

        let toml_str = config.to_toml_string().unwrap();
        assert!(toml_str.contains("/data/docs"));
        assert!(toml_str.contains("gdrive:backups/host1"));
        assert!(toml_str.contains("hourly = 3"));

        // Verify it can be loaded back
        let mut tmp_file = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut tmp_file, toml_str.as_bytes()).unwrap();
        let loaded = Config::load(tmp_file.path()).unwrap();
        assert_eq!(loaded.source_path, "/data/docs");
        assert_eq!(loaded.remote_path, "gdrive:backups/host1");
        assert_eq!(loaded.retention.hourly, 3);
        assert_eq!(loaded.exclude.patterns, vec!["*.tmp"]);
    }

    #[test]
    fn test_example_config_file_parses() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let example_path = manifest_dir.join("config/incr-backup-example.toml");
        assert!(example_path.exists());
        let config = Config::load(&example_path).expect("example config should parse cleanly");
        assert_eq!(config.source_path, "/home/user/documents");
        assert_eq!(config.remote_path, "gdrive:backups/myhost");
        assert_eq!(config.retention.hourly, 6);
        assert_eq!(config.retention.daily, 7);
        assert_eq!(config.retention.weekly, 4);
        assert_eq!(config.retention.monthly, 6);
        assert_eq!(config.retention.min_interval_minutes, 30);
        assert!(config.exclude.patterns.contains(&"*.tmp".to_string()));
    }
}
