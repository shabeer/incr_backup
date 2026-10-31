//! # Generic Subprocess Module
//!
//! Provides a configurable subprocess runner with support for default extra flags,
//! verbosity levels, dry-run simulation, and piped stdin streaming.
//!
//! ## Usage Example
//! ```rust
//! use common::subprocess::SubprocessRunner;
//!
//! let runner = SubprocessRunner::new("echo")
//!     .with_extra_flags(["world"])
//!     .with_dry_run(false);
//!
//! let output = runner.run(&["hello"], false).unwrap();
//! assert!(output.contains("hello world"));
//! ```

use log::{debug, info};
use std::io::Write;
use std::process::{Command, Stdio};
use thiserror::Error;

/// Errors arising from subprocess execution.
#[derive(Debug, Error)]
pub enum SubprocessError {
    /// The target executable binary was not found.
    #[error("binary not found at '{binary}': {source}")]
    NotFound {
        binary: String,
        #[source]
        source: std::io::Error,
    },

    /// The command exited with a non-zero exit code.
    #[error("command failed: {cmd} (exit {code})\nstderr: {stderr}")]
    Failed {
        cmd: String,
        code: i32,
        stderr: String,
    },

    /// Underlying standard I/O failure.
    #[error("I/O error during subprocess execution: {0}")]
    Io(#[from] std::io::Error),
}

/// Configurable subprocess executor with argument handling, extra flags, and dry-run mode.
#[derive(Debug, Clone)]
pub struct SubprocessRunner {
    /// Path or name of the binary executable.
    binary_path: String,
    /// Default flags appended to every invocation.
    extra_flags: Vec<String>,
    /// When true and allowed, mutating operations log instead of executing.
    dry_run: bool,
    /// Verbosity level controlling `-v` / `-vv` flags or logging output.
    verbose: u8,
}

impl SubprocessRunner {
    /// Create a new runner targeting the specified binary.
    pub fn new(binary_path: impl Into<String>) -> Self {
        Self {
            binary_path: binary_path.into(),
            extra_flags: Vec::new(),
            dry_run: false,
            verbose: 0,
        }
    }

    /// Set default extra flags passed to every command.
    pub fn with_extra_flags<I, S>(mut self, flags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.extra_flags = flags.into_iter().map(Into::into).collect();
        self
    }

    /// Set dry-run mode.
    pub fn with_dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }

    /// Set verbosity level.
    pub fn with_verbose(mut self, verbose: u8) -> Self {
        self.verbose = verbose;
        self
    }

    /// Binary path of this runner.
    pub fn binary_path(&self) -> &str {
        &self.binary_path
    }

    /// Extra flags configured on this runner.
    pub fn extra_flags(&self) -> &[String] {
        &self.extra_flags
    }

    /// Current dry run setting.
    pub fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// Current verbosity level.
    pub fn verbose(&self) -> u8 {
        self.verbose
    }

    /// Execute the binary with given arguments and return its standard output as a string.
    ///
    /// If `dry_run_allowed` is true and dry-run mode is active, the command execution
    /// is skipped and logged, returning an empty `Ok(String)`.
    pub fn run(
        &self,
        args: &[impl AsRef<str>],
        dry_run_allowed: bool,
    ) -> Result<String, SubprocessError> {
        let args_vec: Vec<&str> = args.iter().map(|a| a.as_ref()).collect();

        if self.dry_run && dry_run_allowed {
            info!(
                "[DRY-RUN] Would run: {} {} {}",
                self.binary_path,
                args_vec.join(" "),
                self.extra_flags.join(" ")
            );
            return Ok(String::new());
        }

        let mut cmd = Command::new(&self.binary_path);

        for arg in &args_vec {
            cmd.arg(arg);
        }

        for flag in &self.extra_flags {
            cmd.arg(flag);
        }

        if self.verbose == 1 {
            cmd.arg("-v");
        } else if self.verbose >= 2 {
            cmd.arg("-vv");
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let cmd_str = format!(
            "{} {} {}",
            self.binary_path,
            args_vec.join(" "),
            self.extra_flags.join(" ")
        );
        debug!("Executing: {}", cmd_str.trim());

        let output = cmd.output().map_err(|e| SubprocessError::NotFound {
            binary: self.binary_path.clone(),
            source: e,
        })?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let code = output.status.code().unwrap_or(-1);

            Err(SubprocessError::Failed {
                cmd: cmd_str.trim().to_string(),
                code,
                stderr,
            })
        }
    }

    /// Run the binary, streaming the provided string into standard input (`stdin`).
    pub fn run_piped_stdin(
        &self,
        args: &[impl AsRef<str>],
        stdin_data: &str,
    ) -> Result<String, SubprocessError> {
        let args_vec: Vec<&str> = args.iter().map(|a| a.as_ref()).collect();

        if self.dry_run {
            info!(
                "[DRY-RUN] Would stream {} bytes to: {} {}",
                stdin_data.len(),
                self.binary_path,
                args_vec.join(" ")
            );
            return Ok(String::new());
        }

        let mut cmd = Command::new(&self.binary_path);
        for arg in &args_vec {
            cmd.arg(arg);
        }
        for flag in &self.extra_flags {
            cmd.arg(flag);
        }

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        debug!(
            "Piping {} bytes to {} {}",
            stdin_data.len(),
            self.binary_path,
            args_vec.join(" ")
        );

        let mut child = cmd.spawn().map_err(|e| SubprocessError::NotFound {
            binary: self.binary_path.clone(),
            source: e,
        })?;

        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(stdin_data.as_bytes())?;
        }
        drop(child.stdin.take());

        let output = child.wait_with_output()?;

        let cmd_str = format!(
            "{} {} {}",
            self.binary_path,
            args_vec.join(" "),
            self.extra_flags.join(" ")
        );

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Err(SubprocessError::Failed {
                cmd: cmd_str.trim().to_string(),
                code: output.status.code().unwrap_or(-1),
                stderr,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subprocess_dry_run() {
        let runner = SubprocessRunner::new("non_existent_binary_xyz")
            .with_dry_run(true);

        let res = runner.run(&["arg1"], true);
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), "");
    }

    #[test]
    fn test_subprocess_echo() {
        let runner = SubprocessRunner::new("echo");
        let output = runner.run(&["testing_subprocess_echo"], false).unwrap();
        assert!(output.contains("testing_subprocess_echo"));
    }

    #[test]
    fn test_subprocess_piped_stdin() {
        let runner = SubprocessRunner::new("cat");
        let output = runner.run_piped_stdin(&[] as &[&str], "piped_input_data").unwrap();
        assert_eq!(output, "piped_input_data");
    }

    #[test]
    fn test_subprocess_not_found() {
        let runner = SubprocessRunner::new("totally_invalid_binary_name_12345");
        let err = runner.run(&[] as &[&str], false).unwrap_err();
        match err {
            SubprocessError::NotFound { .. } => {}
            _ => panic!("Expected NotFound, got {:?}", err),
        }
    }
}
