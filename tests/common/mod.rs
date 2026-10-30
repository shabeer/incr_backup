//! Common test helpers for incr-backup integration tests.

use std::fs;
use std::path::PathBuf;
use std::process::{Command as StdCommand, Stdio};
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

/// Live Google Drive test environment backed by real cloud storage.
///
/// Loads configuration from `tests/incr-backup-testing.toml` and creates an isolated
/// directory namespace under the configured remote (e.g. `kaleemshabeer01_5TB_testing:incr_test_<name>_<id>`).
/// Automatically cleans up the cloud directory on drop.
#[allow(dead_code)]
pub struct LiveGdriveTestEnv {
    /// Temporary local directory for source files and restore destinations.
    pub temp_dir: TempDir,
    /// Local source directory.
    pub source_path: PathBuf,
    /// Path to tests/incr-backup-testing.toml.
    pub config_path: PathBuf,
    /// Base remote defined in tests/incr-backup-testing.toml (e.g. "kaleemshabeer01_5TB_testing:").
    pub base_remote: String,
    /// Dedicated remote path for this specific test case.
    pub test_remote: String,
    /// Path to rclone executable from config.
    pub rclone_path: String,
    /// Extra flags configured in tests/incr-backup-testing.toml.
    pub rclone_flags: Vec<String>,
    /// Configured timezone ("local" or "utc").
    pub timezone: String,
    /// Keep remote files for manual post-mortem inspection if set to true.
    pub keep_remote: bool,
}

#[allow(dead_code)]
impl LiveGdriveTestEnv {
    /// Initialize a new live Google Drive test environment.
    ///
    /// Reads `tests/incr-backup-testing.toml` and creates an isolated namespace on Google Drive.
    /// Returns `None` if `tests/incr-backup-testing.toml` does not exist or the remote is not reachable.
    pub fn new(test_name: &str) -> Option<Self> {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let config_path = manifest_dir.join("tests/incr-backup-testing.toml");

        if !config_path.exists() {
            eprintln!("Skipping live test: config file {:?} not found", config_path);
            return None;
        }

        let config = match incr_backup::config::Config::load(&config_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to parse config file {:?}: {}", config_path, e);
                return None;
            }
        };

        let rclone_path = config.rclone.path.clone();
        let rclone_flags: Vec<String> = config
            .rclone
            .extra_flags
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();

        let base_remote = config.remote_path.clone();

        // Check if rclone executable exists
        if !std::path::Path::new(&rclone_path).exists() {
            eprintln!(
                "Skipping live test: rclone binary {:?} not found",
                rclone_path
            );
            return None;
        }

        // Check connectivity to the remote
        let mut probe_cmd = StdCommand::new(&rclone_path);
        probe_cmd.args(["lsd", &base_remote]);
        for flag in &rclone_flags {
            probe_cmd.arg(flag);
        }
        probe_cmd.stdout(Stdio::null()).stderr(Stdio::null());

        match probe_cmd.status() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!(
                    "Skipping live test: remote {} unreachable (rclone exit: {})",
                    base_remote, status
                );
                return None;
            }
            Err(e) => {
                eprintln!(
                    "Skipping live test: failed to probe remote {}: {}",
                    base_remote, e
                );
                return None;
            }
        }

        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let source_path = temp_dir.path().join("source");
        fs::create_dir_all(&source_path).expect("failed to create source dir");

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let pid = std::process::id();
        let remote_clean = base_remote.trim_end_matches('/');
        let test_remote = format!("{}/live_{}_{}_{}", remote_clean, test_name, timestamp, pid);

        Some(Self {
            temp_dir,
            source_path,
            config_path,
            base_remote,
            test_remote,
            rclone_path,
            rclone_flags,
            timezone: config.timezone.to_string(),
            keep_remote: false,
        })
    }

    /// Source directory path string.
    pub fn source_str(&self) -> String {
        self.source_path.to_string_lossy().to_string()
    }

    /// Remote path string for this isolated test.
    pub fn remote_str(&self) -> String {
        self.test_remote.clone()
    }

    /// Write a file into the local source directory.
    pub fn write_source_file(&self, rel_path: &str, content: &[u8]) {
        let path = self.source_path.join(rel_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent dirs");
        }
        fs::write(path, content).expect("failed to write source file");
    }

    /// Delete a file from the local source directory.
    pub fn delete_source_file(&self, rel_path: &str) {
        let path = self.source_path.join(rel_path);
        if path.exists() {
            fs::remove_file(path).expect("failed to remove source file");
        }
    }

    /// Read a file from the local source directory.
    pub fn read_source_file(&self, rel_path: &str) -> Option<Vec<u8>> {
        let path = self.source_path.join(rel_path);
        if path.exists() {
            Some(fs::read(path).expect("failed to read source file"))
        } else {
            None
        }
    }

    /// Build a Command for the incr-backup binary pre-configured with
    /// `--config tests/incr-backup-testing.toml`, `--remote <test_remote>`, and `--source <source_path>`.
    pub fn command(&self) -> Command {
        let mut cmd = Command::cargo_bin("incr-backup").expect("binary incr-backup exists");
        cmd.env("RUSTC_WRAPPER", "");
        cmd.args([
            "--config",
            self.config_path.to_str().unwrap(),
            "--remote",
            &self.test_remote,
            "--source",
            &self.source_str(),
        ]);
        cmd
    }

    /// Build a Command for the incr-backup binary with ONLY `--config tests/incr-backup-testing.toml`.
    /// Does not override `--remote` or `--source`, testing the raw TOML defaults.
    pub fn raw_command(&self) -> Command {
        let mut cmd = Command::cargo_bin("incr-backup").expect("binary incr-backup exists");
        cmd.env("RUSTC_WRAPPER", "");
        cmd.args(["--config", self.config_path.to_str().unwrap()]);
        cmd
    }

    /// Execute rclone command directly with configured extra flags.
    pub fn rclone_exec(&self, args: &[&str]) -> std::process::Output {
        let mut cmd = StdCommand::new(&self.rclone_path);
        for arg in args {
            cmd.arg(arg);
        }
        for flag in &self.rclone_flags {
            cmd.arg(flag);
        }
        cmd.output().expect("failed to execute rclone")
    }

    /// Purge remote test directory from Google Drive.
    pub fn cleanup(&self) {
        if !self.keep_remote {
            let _ = self.rclone_exec(&["purge", &self.test_remote]);
        }
    }
}

impl Drop for LiveGdriveTestEnv {
    fn drop(&mut self) {
        self.cleanup();
    }
}
