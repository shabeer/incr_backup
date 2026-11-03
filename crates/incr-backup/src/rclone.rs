//! # Rclone Module — Subprocess Wrapper
//!
//! Thin wrapper around the `rclone` CLI binary. Every remote I/O operation
//! in incr-backup goes through this module as a subprocess call.
//!
//! ## Analogy
//! Like a Java `ProcessBuilder` wrapper or Python's `subprocess.run()` helper —
//! builds command lines, captures output, checks exit codes, and converts
//! failures into typed errors.
//!
//! ## Design
//! We do NOT reimplement any rclone functionality. This module is purely
//! orchestration: "call rclone with these args and give me the result."

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{IncrError, Result};
use common::subprocess::SubprocessRunner;

/// Wrapper around the rclone binary for all remote operations.
///
/// Constructed once from [`Config`] and shared (by reference) across all
/// manager objects. Wraps a generic [`SubprocessRunner`] with rclone-specific
/// subcommands.
pub struct Rclone {
    runner: SubprocessRunner,
}

/// Parsed output of `rclone size --json`.
///
/// Example JSON: `{"count":45231,"bytes":214748364800}`
#[derive(Debug, Deserialize)]
pub struct SizeOutput {
    pub count: u64,
    pub bytes: u64,
}

impl Rclone {
    /// Create a new Rclone wrapper from the application config.
    pub fn new(config: &Config) -> Self {
        let extra_flags: Vec<String> = if config.rclone.extra_flags.is_empty() {
            Vec::new()
        } else {
            config
                .rclone
                .extra_flags
                .split_whitespace()
                .map(|s| s.to_string())
                .collect()
        };

        let runner = SubprocessRunner::new(&config.rclone.path)
            .with_extra_flags(extra_flags)
            .with_dry_run(config.dry_run)
            .with_verbose(config.verbose);

        Self { runner }
    }

    /// Path to the rclone binary.
    pub fn binary_path(&self) -> &str {
        self.runner.binary_path()
    }

    /// Extra flags configured on this rclone instance.
    pub fn extra_flags(&self) -> &[String] {
        self.runner.extra_flags()
    }

    /// Whether dry-run mode is enabled.
    pub fn is_dry_run(&self) -> bool {
        self.runner.is_dry_run()
    }

    // ─── Core Operations ────────────────────────────────────────────────

    /// Run `rclone sync source dest [--backup-dir backup_dir] [--exclude ...]`.
    ///
    /// This is the primary data transfer command. With `--backup-dir`, rclone
    /// server-side-moves old versions from `dest` to `backup_dir` before
    /// uploading new versions. Zero extra bandwidth for the old files.
    ///
    /// # Arguments
    /// * `source` — Local source path (e.g., `"/data/important/"`)
    /// * `dest` — Remote destination (e.g., `"gdrive:backup/myhost/current/"`)
    /// * `backup_dir` — Where old versions go (e.g., `"gdrive:backup/myhost/diffs/hourly.0__..."`)
    /// * `excludes` — Glob patterns to exclude (each becomes `--exclude <pattern>`)
    pub fn sync(
        &self,
        source: &str,
        dest: &str,
        backup_dir: Option<&str>,
        excludes: &[String],
    ) -> Result<String> {
        let mut args = vec!["sync".to_string(), source.to_string(), dest.to_string()];

        if let Some(bdir) = backup_dir {
            args.push("--backup-dir".to_string());
            args.push(bdir.to_string());
        }

        for pattern in excludes {
            args.push("--exclude".to_string());
            args.push(pattern.clone());
        }

        self.run_rclone(&args, /* is_mutating */ true)
    }

    /// Build command arguments for `rclone lsjson`.
    pub fn lsjson_args(remote_path: &str, recursive: bool, with_hash: bool) -> Vec<String> {
        let mut args = vec!["lsjson".to_string(), remote_path.to_string()];

        if recursive {
            args.push("--recursive".to_string());
            args.push("--fast-list".to_string());
        }
        if with_hash {
            args.push("--hash".to_string());
        }

        // `--no-mimetype` to reduce output size (we don't need MIME types)
        args.push("--no-mimetype".to_string());
        args
    }

    /// Run `rclone lsjson [--recursive] [--fast-list] [--hash] <remote_path>`.
    ///
    /// Returns raw JSON output listing files at the remote path.
    /// Used to build manifests and check remote state.
    ///
    /// Always executes (even in dry-run mode) since it's read-only.
    pub fn lsjson(&self, remote_path: &str, recursive: bool, with_hash: bool) -> Result<String> {
        let args = Self::lsjson_args(remote_path, recursive, with_hash);
        self.run_rclone(&args, /* is_mutating */ false)
    }

    /// Convenience: `rclone lsjson --recursive --hash <path>`.
    pub fn lsjson_recursive_hash(&self, remote_path: &str) -> Result<String> {
        self.lsjson(remote_path, true, true)
    }

    /// Run `rclone lsjson <remote_path> [--recursive] [--fast-list] [--hash] [--include <pattern>...]`.
    ///
    /// Scopes the listing to files matching the provided include patterns.
    /// Always executes (even in dry-run mode) since it's read-only.
    pub fn lsjson_filtered(
        &self,
        remote_path: &str,
        recursive: bool,
        with_hash: bool,
        includes: &[String],
    ) -> Result<String> {
        let mut args = Self::lsjson_args(remote_path, recursive, with_hash);
        for inc in includes {
            args.push("--include".to_string());
            args.push(inc.clone());
        }
        self.run_rclone(&args, /* is_mutating */ false)
    }

    /// Run `rclone move source dest [--exclude ...] [--delete-empty-src-dirs]`.
    ///
    /// On Google Drive, this is a **server-side** operation — only metadata
    /// is updated, no data re-upload. Used during `init --adopt` to move
    /// existing files into `current/`.
    pub fn move_dir(
        &self,
        source: &str,
        dest: &str,
        excludes: &[String],
        delete_empty_src_dirs: bool,
    ) -> Result<String> {
        let mut args = vec!["move".to_string(), source.to_string(), dest.to_string()];

        for pattern in excludes {
            args.push("--exclude".to_string());
            args.push(pattern.clone());
        }

        if delete_empty_src_dirs {
            args.push("--delete-empty-src-dirs".to_string());
        }

        self.run_rclone(&args, /* is_mutating */ true)
    }

    /// Run `rclone moveto source dest`.
    ///
    /// Renames a single file or directory on the remote. Server-side on GDrive.
    /// Used for staging commits or snapshot renaming (e.g., during compaction).
    pub fn moveto(&self, source: &str, dest: &str) -> Result<String> {
        self.run_rclone(
            &["moveto".to_string(), source.to_string(), dest.to_string()],
            true,
        )
    }

    /// Run `rclone move source dest --files-from <temp_file> --transfers 8 --checkers 8`.
    ///
    /// Moves a batch of files from `source` to `dest` using a file list.
    /// On Google Drive and supported cloud remotes, this performs server-side moves
    /// concurrently using rclone's internal worker pool.
    pub fn move_files_from(
        &self,
        source: &str,
        dest: &str,
        relative_paths: &[String],
    ) -> Result<String> {
        if relative_paths.is_empty() {
            return Ok(String::new());
        }

        use std::io::Write;
        let mut temp = tempfile::NamedTempFile::new().map_err(IncrError::Io)?;
        for path in relative_paths {
            writeln!(temp, "{path}").map_err(IncrError::Io)?;
        }
        temp.flush().map_err(IncrError::Io)?;

        let temp_path = temp.path().to_string_lossy().to_string();

        let args = vec![
            "move".to_string(),
            source.to_string(),
            dest.to_string(),
            "--files-from".to_string(),
            temp_path,
            "--transfers".to_string(),
            "8".to_string(),
            "--checkers".to_string(),
            "8".to_string(),
        ];

        self.run_rclone(&args, /* is_mutating */ true)
    }

    /// Run `rclone purge <remote_path>`.
    ///
    /// Completely deletes the directory and all of its contents in one operation.
    /// WARNING: Do NOT pass `--exclude`, `--include`, or filter flags to `purge`.
    pub fn purge(&self, remote_path: &str) -> Result<String> {
        self.run_rclone(&["purge".to_string(), remote_path.to_string()], true)
    }

    /// Run `rclone mkdir <remote_path>`.
    ///
    /// Creates a directory on the remote. Idempotent — creating an existing
    /// directory is a no-op.
    pub fn mkdir(&self, remote_path: &str) -> Result<String> {
        self.run_rclone(&["mkdir".to_string(), remote_path.to_string()], true)
    }

    /// Run `rclone deletefile <remote_path>`.
    ///
    /// Deletes a single file from the remote. Used for lock, journal, and manifest cleanup.
    pub fn delete_file(&self, remote_path: &str) -> Result<String> {
        self.run_rclone(&["deletefile".to_string(), remote_path.to_string()], true)
    }

    /// Run `rclone delete <remote_path> [--include <pattern>...]`.
    ///
    /// Pure file-level deletion without `--rmdirs`, safe under restricted `drive.file` scope.
    /// Deletes files in `remote_path` matching include patterns (or all files if empty).
    pub fn delete(&self, remote_path: &str, includes: &[String]) -> Result<String> {
        let mut args = vec![
            "delete".to_string(),
            remote_path.to_string(),
        ];
        for inc in includes {
            args.push("--include".to_string());
            args.push(inc.clone());
        }
        self.run_rclone(&args, /* is_mutating */ true)
    }

    /// Run `rclone rmdirs <remote_dir>`.
    ///
    /// Removes an empty specific directory (e.g. `remote:diffs/TIMESTAMP` or `.meta/staging/TIMESTAMP`).
    /// Targeted strictly at the specific directory, never at parent containers like `diffs/` or `current/`.
    pub fn rmdir_specific(&self, remote_dir: &str) -> Result<String> {
        self.run_rclone(&["rmdirs".to_string(), remote_dir.to_string()], true)
    }

    /// Run `rclone cat <remote_path>` — read a remote file's contents.
    ///
    /// Returns the file contents as a string. Used to read lock files,
    /// journal files, and manifests from the remote.
    ///
    /// Always executes (read-only).
    pub fn cat(&self, remote_path: &str) -> Result<String> {
        self.run_rclone(&["cat".to_string(), remote_path.to_string()], false)
    }

    /// Write data to a remote file via `rclone rcat <remote_path>`.
    ///
    /// Pipes `data` to rclone's stdin, which writes it to the remote path.
    /// Used to upload lock files, journal files, manifests, and config.
    pub fn rcat(&self, remote_path: &str, data: &str) -> Result<String> {
        self.runner
            .run_piped_stdin(&["rcat", remote_path], data)
            .map_err(IncrError::from)
    }

    /// Run `rclone size <remote_path> --json`.
    ///
    /// Returns a tuple of `(file_count, total_bytes)`.
    /// Used by the verify command to cross-check against manifests.
    ///
    /// Always executes (read-only).
    pub fn size(&self, remote_path: &str) -> Result<SizeOutput> {
        let output = self.run_rclone(
            &["size".to_string(), remote_path.to_string(), "--json".to_string()],
            false,
        )?;

        let size: SizeOutput = serde_json::from_str(&output).map_err(|e| {
            IncrError::RcloneFailed {
                cmd: format!("rclone size {} --json", remote_path),
                code: 0,
                stderr: format!("failed to parse size output: {e}. Output: {output}"),
            }
        })?;

        Ok(size)
    }

    /// Run `rclone lsd <remote_path>` — list directories only.
    ///
    /// Returns a list of directory names. Used to discover existing
    /// snapshot directories under `diffs/`.
    ///
    /// Always executes (read-only).
    pub fn lsd(&self, remote_path: &str) -> Result<Vec<String>> {
        let clean_path = remote_path.trim_end_matches('/');

        // Try lsjson first
        let output = self.run_rclone(
            &[
                "lsjson".to_string(),
                clean_path.to_string(),
                "--dirs-only".to_string(),
            ],
            false,
        );

        let mut dirs = Vec::new();
        let mut lsjson_succeeded = false;

        if let Ok(json_str) = output {
            if let Ok(entries) = serde_json::from_str::<Vec<LsjsonEntry>>(&json_str) {
                lsjson_succeeded = true;
                for e in entries {
                    if e.is_dir {
                        let p = if !e.name.is_empty() {
                            e.name
                        } else if !e.path.is_empty() {
                            e.path.rsplit('/').next().unwrap_or(&e.path).to_string()
                        } else {
                            continue;
                        };
                        let trimmed = p.trim_matches('/').to_string();
                        if !trimmed.is_empty() && !dirs.contains(&trimmed) {
                            dirs.push(trimmed);
                        }
                    }
                }
            }
        }

        // If lsjson failed or produced an empty list, fallback to `rclone lsf --dirs-only`
        if !lsjson_succeeded || dirs.is_empty() {
            if let Ok(lsf_output) = self.run_rclone(
                &[
                    "lsf".to_string(),
                    clean_path.to_string(),
                    "--dirs-only".to_string(),
                ],
                false,
            ) {
                for line in lsf_output.lines() {
                    let trimmed = line.trim().trim_matches('/').to_string();
                    if !trimmed.is_empty() && !dirs.contains(&trimmed) {
                        dirs.push(trimmed);
                    }
                }
            }
        }

        Ok(dirs)
    }

    /// Run `rclone lsf <remote_path>` — list files only.
    ///
    /// Returns a list of file names/paths. Used to check manifest existence.
    ///
    /// Always executes (read-only).
    pub fn lsf(&self, remote_path: &str) -> Result<Vec<String>> {
        let output = self.run_rclone(&["lsf".to_string(), remote_path.to_string()], false)?;

        Ok(output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect())
    }

    /// Run `rclone lsf <remote_path> --files-only --recursive` — list all files recursively.
    pub fn lsf_files_recursive(&self, remote_path: &str) -> Result<Vec<String>> {
        let output = self.run_rclone(
            &[
                "lsf".to_string(),
                remote_path.to_string(),
                "--files-only".to_string(),
                "--recursive".to_string(),
            ],
            false,
        )?;

        Ok(output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.trim_end_matches('/').to_string())
            .collect())
    }

    /// Run `rclone copy <source> <dest> --files-from <path>`.
    ///
    /// Copies specific files listed in a files-from text file.
    /// Used during restore to selectively copy files from different
    /// remote locations (current/ and various diffs/).
    pub fn copy_files(&self, source: &str, dest: &str, files_from: &str) -> Result<String> {
        self.run_rclone(
            &[
                "copy".to_string(),
                source.to_string(),
                dest.to_string(),
                "--files-from".to_string(),
                files_from.to_string(),
            ],
            true,
        )
    }

    /// Run `rclone copy <source> <dest>`.
    ///
    /// Copy files/directories from source to dest.
    pub fn copy(&self, source: &str, dest: &str) -> Result<String> {
        self.run_rclone(
            &["copy".to_string(), source.to_string(), dest.to_string()],
            true,
        )
    }

    /// Run `rclone copyto <source> <dest>`.
    ///
    /// Copy a single file or directory to a specific destination path.
    pub fn copyto(&self, source: &str, dest: &str) -> Result<String> {
        self.run_rclone(
            &["copyto".to_string(), source.to_string(), dest.to_string()],
            true,
        )
    }

    /// List entries at the root of a remote path (non-recursive).
    pub fn list_root_entries(&self, remote_path: &str) -> Result<Vec<RemoteRootEntry>> {
        let json_output = match self.lsjson(remote_path, false, false) {
            Ok(out) => out,
            Err(IncrError::RcloneFailed { code, ref stderr, .. })
                if code == 3 || stderr.to_lowercase().contains("directory not found") =>
            {
                return Ok(Vec::new());
            }
            Err(e) => return Err(e),
        };

        if json_output.trim().is_empty() {
            return Ok(Vec::new());
        }

        let entries: Vec<RemoteRootEntry> = serde_json::from_str(&json_output).map_err(|e| {
            IncrError::InvalidManifest {
                path: remote_path.to_string(),
                reason: format!("failed to parse root entries: {e}"),
            }
        })?;

        Ok(entries)
    }

    /// Compute or fetch MD5 hash for a remote file by streaming/downloading if needed.
    pub fn compute_remote_md5(&self, remote_file_path: &str) -> Result<String> {
        if let Ok(output) = self.run_rclone(
            &[
                "hashsum".to_string(),
                "md5".to_string(),
                remote_file_path.to_string(),
            ],
            false,
        ) {
            for line in output.lines() {
                let trimmed = line.trim();
                if let Some((hash, _)) = trimmed.split_once("  ") {
                    let h = hash.trim();
                    if !h.is_empty() {
                        return Ok(h.to_string());
                    }
                } else if trimmed.len() == 32 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Ok(trimmed.to_string());
                }
            }
        }

        // Fallback: cat content and compute md5
        let content = self.cat(remote_file_path)?;
        Ok(format!("{:x}", Md5::digest(content.as_bytes())))
    }

    // ─── Private Helpers ────────────────────────────────────────────────

    /// Build and execute an rclone command, capturing output.
    ///
    /// This is the central dispatch method. All public methods delegate here.
    ///
    /// # Arguments
    /// * `args` — Command arguments (excluding the binary name)
    /// * `is_mutating` — If `true` and `dry_run` is enabled, log and skip
    ///
    /// # Returns
    /// Stdout as a String on success.
    ///
    /// # Errors
    /// * [`IncrError::RcloneNotFound`] if the binary doesn't exist
    /// * [`IncrError::RcloneFailed`] if the exit code is non-zero
    fn run_rclone(&self, args: &[String], is_mutating: bool) -> Result<String> {
        self.runner
            .run(args, is_mutating)
            .map_err(IncrError::from)
    }
}

/// Entry at the root of a remote from `rclone lsjson`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteRootEntry {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub is_dir: bool,
}

/// Entry from `rclone lsjson` output — used internally for parsing.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LsjsonEntry {
    #[serde(default)]
    path: String,
    #[serde(default)]
    name: String,
    #[serde(default = "default_is_dir")]
    is_dir: bool,
}

fn default_is_dir() -> bool {
    true
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            source_path: "/tmp/test-source".to_string(),
            remote_path: "test-remote:backup".to_string(),
            rclone: crate::config::RcloneConfig {
                path: "rclone".to_string(),
                extra_flags: String::new(),
            },
            exclude: crate::config::ExcludeConfig::default(),
            verbose: 0,
            dry_run: true, // always dry-run in tests
            timezone: crate::config::SnapshotTimezone::default(),
        }
    }

    #[test]
    fn test_rclone_new() {
        let config = test_config();
        let rclone = Rclone::new(&config);
        assert_eq!(rclone.binary_path(), "rclone");
        assert!(rclone.extra_flags().is_empty());
        assert!(rclone.is_dry_run());
    }

    #[test]
    fn test_rclone_extra_flags_parsing() {
        let mut config = test_config();
        config.rclone.extra_flags = "--drive-acknowledge-abuse --transfers 8".to_string();
        let rclone = Rclone::new(&config);
        assert_eq!(
            rclone.extra_flags(),
            vec!["--drive-acknowledge-abuse", "--transfers", "8"]
        );
    }

    #[test]
    fn test_dry_run_sync_returns_ok() {
        let config = test_config();
        let rclone = Rclone::new(&config);
        // Dry-run sync should succeed without actually running rclone
        let result = rclone.sync("/source", "remote:dest", Some("remote:bak"), &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_dry_run_rcat_returns_ok() {
        let config = test_config();
        let rclone = Rclone::new(&config);
        let result = rclone.rcat("remote:file.json", "{}");
        assert!(result.is_ok());
    }

    #[test]
    fn test_dry_run_delete_returns_ok() {
        let config = test_config();
        let rclone = Rclone::new(&config);
        let result = rclone.delete("remote:dest", &["*.tmp".to_string()]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_dry_run_rmdir_specific_returns_ok() {
        let config = test_config();
        let rclone = Rclone::new(&config);
        let result = rclone.rmdir_specific("remote:diffs/2026-11-02T08-00Z");
        assert!(result.is_ok());
    }

    #[test]
    fn test_dry_run_purge_returns_ok() {
        let config = test_config();
        let rclone = Rclone::new(&config);
        let result = rclone.purge("remote:diffs/2026-11-02T08-00Z");
        assert!(result.is_ok());
    }

    #[test]
    fn test_dry_run_move_files_from_returns_ok() {
        let config = test_config();
        let rclone = Rclone::new(&config);
        let files = vec!["file1.txt".to_string(), "subdir/file2.txt".to_string()];
        let result = rclone.move_files_from("remote:src", "remote:dst", &files);
        assert!(result.is_ok());
    }

    #[test]
    fn test_size_output_deserialization() {
        let json = r#"{"count":45231,"bytes":214748364800}"#;
        let size: SizeOutput = serde_json::from_str(json).unwrap();
        assert_eq!(size.count, 45231);
        assert_eq!(size.bytes, 214748364800);
    }

    #[test]
    fn test_lsjson_args_recursive_includes_fast_list() {
        let args = Rclone::lsjson_args("remote:path", true, true);
        assert!(args.contains(&"--recursive".to_string()));
        assert!(args.contains(&"--fast-list".to_string()));
        assert!(args.contains(&"--hash".to_string()));
        assert!(args.contains(&"--no-mimetype".to_string()));
    }

    #[test]
    fn test_lsjson_args_non_recursive_omits_fast_list() {
        let args = Rclone::lsjson_args("remote:path", false, false);
        assert!(!args.contains(&"--recursive".to_string()));
        assert!(!args.contains(&"--fast-list".to_string()));
        assert!(!args.contains(&"--hash".to_string()));
        assert!(args.contains(&"--no-mimetype".to_string()));
    }
}

