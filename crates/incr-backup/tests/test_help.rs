mod common;

use common::TestEnv;

#[test]
fn test_help_menu_global_options_at_bottom() {
    let env = TestEnv::new();

    // Test main command help
    let assert = env.command().arg("--help").assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("Global Options:"));
    let global_idx = stdout.find("Global Options:").unwrap();
    let commands_idx = stdout.find("Commands:").unwrap();
    assert!(global_idx > commands_idx);

    // Test subcommand help (sync)
    let assert_sync = env.command().args(["sync", "--help"]).assert().success();
    let stdout_sync = String::from_utf8(assert_sync.get_output().stdout.clone()).unwrap();
    assert!(stdout_sync.contains("Options:"));
    assert!(stdout_sync.contains("Global Options:"));

    let options_idx = stdout_sync.find("Options:").unwrap();
    let global_options_idx = stdout_sync.find("Global Options:").unwrap();

    // Subcommand-specific options must be at top, global options at bottom
    assert!(
        global_options_idx > options_idx,
        "Global Options must be below subcommand Options"
    );

    // Subcommand specific options (--name, --force) are before Global Options
    let name_idx = stdout_sync.find("--name").unwrap();
    let force_idx = stdout_sync.find("--force").unwrap();
    assert!(name_idx < global_options_idx);
    assert!(force_idx < global_options_idx);

    // Global options (--config, --dry-run) are after Global Options heading
    let config_idx = stdout_sync.find("--config").unwrap();
    let dry_run_idx = stdout_sync.find("--dry-run").unwrap();
    assert!(config_idx > global_options_idx);
    assert!(dry_run_idx > global_options_idx);

    // Test subcommand with arguments (restore)
    let assert_restore = env.command().args(["restore", "--help"]).assert().success();
    let stdout_restore = String::from_utf8(assert_restore.get_output().stdout.clone()).unwrap();
    let args_idx = stdout_restore.find("Arguments:").unwrap();
    let options_idx = stdout_restore.find("Options:").unwrap();
    let global_options_idx = stdout_restore.find("Global Options:").unwrap();

    assert!(args_idx < options_idx);
    assert!(options_idx < global_options_idx);
}

#[test]
fn test_diff_help_snapshot_older_newer_statement() {
    let env = TestEnv::new();

    let assert_diff = env.command().args(["diff", "--help"]).assert().success();
    let stdout_diff = String::from_utf8(assert_diff.get_output().stdout.clone()).unwrap();

    assert!(
        stdout_diff.contains("snapshot1 should be older and snapshot2 should be newer"),
        "diff help should state that snapshot1 should be older and snapshot2 should be newer"
    );
}

#[test]
fn test_changelog_help() {
    let env = TestEnv::new();

    let assert_changelog = env.command().args(["changelog", "--help"]).assert().success();
    let stdout_changelog = String::from_utf8(assert_changelog.get_output().stdout.clone()).unwrap();

    assert!(stdout_changelog.contains("Show changelog across snapshots"));
    assert!(stdout_changelog.contains("--from"));
    assert!(stdout_changelog.contains("--to"));
    assert!(stdout_changelog.contains("--json"));
}

#[test]
fn test_list_help() {
    let env = TestEnv::new();

    let assert_list = env.command().args(["list", "--help"]).assert().success();
    let stdout_list = String::from_utf8(assert_list.get_output().stdout.clone()).unwrap();

    assert!(stdout_list.contains("List all snapshots"));
    assert!(stdout_list.contains("--from"));
    assert!(stdout_list.contains("--to"));
    assert!(stdout_list.contains("--json"));
}


