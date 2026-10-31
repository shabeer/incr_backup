//! # Subprocess CLI Test Runner
//!
//! Provides execution and assertion helpers for CLI integration testing.
//!
//! ## Usage Example
//! ```rust,no_run
//! use test_commons::runner::CommandRunner;
//!
//! let mut runner = CommandRunner::for_bin("incr-backup");
//! runner.cmd().arg("--help").assert().success();
//! ```

use assert_cmd::Command;

/// Test command runner configured with sanitized environment variables.
pub struct CommandRunner {
    cmd: Command,
}

impl CommandRunner {
    /// Create a runner for a cargo binary name.
    pub fn for_bin(bin_name: &str) -> Self {
        let mut cmd = Command::cargo_bin(bin_name)
            .unwrap_or_else(|e| panic!("binary '{}' should exist: {}", bin_name, e));
        // Neutralize sccache or custom wrapper socket permissions in sandboxes
        cmd.env("RUSTC_WRAPPER", "");
        Self { cmd }
    }

    /// Access mutable reference to the underlying assert_cmd Command.
    pub fn cmd(&mut self) -> &mut Command {
        &mut self.cmd
    }

    /// Consume into underlying Command.
    pub fn into_cmd(self) -> Command {
        self.cmd
    }
}

