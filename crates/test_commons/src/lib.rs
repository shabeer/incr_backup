//! # Test Commons — Reusable Test Infrastructure
//!
//! Provides isolated filesystem workspaces, CLI command runner helpers with
//! sanitized environments, sample test file generators, and mock backends.

pub mod workspace;
pub mod runner;
pub mod fixtures;
pub mod mocks;

pub use workspace::TempWorkspace;
pub use runner::CommandRunner;
pub use fixtures::create_sample_tree;
pub use mocks::MockStorage;

