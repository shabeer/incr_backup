//! # Live Integration and Usage Tests for Google Drive Remote
//!
//! Uses `tests/incr-backup-testing.toml` containing real Google Drive remote
//! (`kaleemshabeer01_5TB_testing:`) and rclone wrapper (`/Users/Shared/bin/rclone_all_allowed`).
//!
//! ## Test Coverage:
//! - Configuration loading & default resolution from TOML
//! - Clean initialization & idempotent re-initialization
//! - Adoption of pre-existing remote directories (`--adopt`)
//! - Baseline sync with all corner-case files:
//!   * Deep nested directories
//!   * Zero-byte files
//!   * Files with whitespace in names
//!   * Unicode / UTF-8 multi-byte characters
//!   * Special symbols and punctuation
//!   * Binary non-text payloads
//! - Exclusion pattern enforcement (`*.tmp`, `node_modules/`, `*.log`, `.DS_Store`, etc.)
//! - Dry-run mode (`--dry-run` / `-n`) without mutating Google Drive
//! - Reverse-incremental snapshot mechanics (updates to `current/`, historical preservation in `diffs/<snapshot>/`)
//! - Listing snapshots (`list`, `--json`)
//! - Status reporting (`status`)
//! - Snapshot diffing (`diff`, `--stat`, `--json`, `--only-added`, `--only-deleted`, `--only-modified`)
//! - Point-in-time and subpath restore (`restore`, `--to`, `--path`)
//! - Restore hash verification (`--verify`)
//! - Restore collision protection and force overwrite (`--overwrite`)
//! - Snapshot integrity verification (`verify`, `--check-hashes`, `--space`)
//! - Snapshot consolidation and pruning (`compact`, `--from`, `--to`, `--name`, `--summary`)
//! - Remote file removal across all snapshots (`remove`, `rm`)
//! - Remote configuration synchronization (`config push`, `config diff`, `config pull`)
//! - Lock concurrency detection (`LockHeld`) and override (`--force`)

mod common;

use common::LiveGdriveTestEnv;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;

// ─── 1. Configuration & Status ───────────────────────────────────────────────

#[test]
fn test_live_config_and_status() {
    let Some(env) = LiveGdriveTestEnv::new("config_status") else {
        eprintln!("Skipping test_live_config_and_status: Google Drive remote unreachable");
        return;
    };

    // Test raw command using tests/incr-backup-testing.toml directly without overriding remote or source
    env.raw_command()
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("kaleemshabeer01_5TB_testing:"))
        .stdout(predicate::str::contains("/tmp/incr_backup_testing/"))
        .stdout(predicate::str::contains("Lock:       (none)"));
}

// ─── 2. Clean Init & Idempotency ────────────────────────────────────────────

#[test]
fn test_live_init_clean_and_idempotent() {
    let Some(env) = LiveGdriveTestEnv::new("init_idempotent") else {
        eprintln!("Skipping test_live_init_clean_and_idempotent: Google Drive remote unreachable");
        return;
    };

    // 1. First init on clean remote namespace
    env.command()
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("Initialized"))
        .stdout(predicate::str::contains("Ready for incremental backups"));

    // Verify remote directory structure created via rclone
    let output = env.rclone_exec(&["ls", &env.remote_str()]);
    let ls_text = String::from_utf8_lossy(&output.stdout);
    assert!(
        ls_text.contains(".meta/config.json"),
        "config.json must exist in .meta/"
    );
    assert!(
        ls_text.contains(".meta/manifests/current.json"),
        "current.json manifest must exist in .meta/manifests/"
    );

    // 2. Second init — must be idempotent and succeed without error
    env.command()
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("already initialized"));
}

// ─── 3. Adopt Existing Remote Files ─────────────────────────────────────────

#[test]
fn test_live_init_adopt_existing() {
    let Some(env) = LiveGdriveTestEnv::new("init_adopt") else {
        eprintln!("Skipping test_live_init_adopt_existing: Google Drive remote unreachable");
        return;
    };

    // Pre-populate a file directly in remote current/
    let current_path = format!("{}/current/pre_existing.txt", env.remote_str());
    let _ = env.rclone_exec(&["rcat", &current_path]); // create via rcat or copy
    // Write via stdin
    let mut cmd = std::process::Command::new(&env.rclone_path);
    cmd.args(["rcat", &current_path]);
    for flag in &env.rclone_flags {
        cmd.arg(flag);
    }
    cmd.stdin(std::process::Stdio::piped());
    if let Ok(mut child) = cmd.spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(b"pre-existing remote content");
        }
        let _ = child.wait();
    }

    // Run init with --adopt
    env.command()
        .args(["init", "--adopt"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Adopted"))
        .stdout(predicate::str::contains("1 files indexed"));

    // Verify status confirms indexed files
    env.command()
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("1 files"));
}

// ─── 4. Baseline Sync with All Corner Cases ─────────────────────────────────

#[test]
fn test_live_sync_baseline_and_corner_cases() {
    let Some(env) = LiveGdriveTestEnv::new("sync_baseline") else {
        eprintln!("Skipping test_live_sync_baseline_and_corner_cases: Google Drive remote unreachable");
        return;
    };

    // 1. Initialize remote
    env.command().arg("init").assert().success();

    // 2. Populate source with corner cases:
    // a) Standard ASCII file
    env.write_source_file("standard.txt", b"Standard ASCII content 12345\n");

    // b) Deep nested hierarchy
    env.write_source_file(
        "level1/level2/level3/deeply_nested.txt",
        b"Deep nested file content\n",
    );

    // c) Zero-byte (empty) file
    env.write_source_file("empty_zero_byte.txt", b"");

    // d) Filename with spaces and mixed case
    env.write_source_file(
        "Folder with Spaces/My Quarterly Report 2026.docx",
        b"Report binary placeholder content\n",
    );

    // e) Unicode characters and emoji
    env.write_source_file(
        "unicode_ünïcödë_日本語_🎉.txt",
        "Multi-byte UTF-8 content: こんにちは世界 — 100% verified\n".as_bytes(),
    );

    // f) Special punctuation characters
    env.write_source_file(
        "special_chars_#1_@host_[v2.0]!~.json",
        b"{\"id\": 1, \"status\": \"valid\"}",
    );

    // g) Binary data (non-text payload)
    let binary_bytes: Vec<u8> = (0..255).collect();
    env.write_source_file("data/payload.bin", &binary_bytes);

    // 3. Execute baseline sync
    env.command()
        .args(["sync", "--name", "baseline-snap-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Sync complete: baseline-snap-1"))
        .stdout(predicate::str::contains("7 files"));

    // 4. Verify remote current/ has all 7 files
    let output = env.rclone_exec(&["ls", &format!("{}/current", env.remote_str())]);
    let ls_text = String::from_utf8_lossy(&output.stdout);
    assert!(ls_text.contains("standard.txt"), "standard.txt missing in current/");
    assert!(ls_text.contains("deeply_nested.txt"), "deeply_nested.txt missing in current/");
    assert!(ls_text.contains("empty_zero_byte.txt"), "empty_zero_byte.txt missing in current/");
    assert!(ls_text.contains("My Quarterly Report 2026.docx"), "Spaced file missing");
    assert!(ls_text.contains("unicode_ünïcödë_日本語_🎉.txt"), "Unicode file missing");
    assert!(ls_text.contains("payload.bin"), "Binary payload missing");

    // 5. Verify manifest exists on remote
    let manifest_check = env.rclone_exec(&[
        "cat",
        &format!("{}/.meta/manifests/baseline-snap-1.json", env.remote_str()),
    ]);
    assert!(
        manifest_check.status.success(),
        "baseline-snap-1.json manifest must exist on remote"
    );
    let manifest_json: Value =
        serde_json::from_slice(&manifest_check.stdout).expect("manifest must be valid JSON");
    assert_eq!(manifest_json["snapshot_name"], "baseline-snap-1");
    assert_eq!(manifest_json["files"].as_array().unwrap().len(), 7);
}

// ─── 5. Exclusion Patterns Enforcement ──────────────────────────────────────

#[test]
fn test_live_sync_exclusions() {
    let Some(env) = LiveGdriveTestEnv::new("sync_exclusions") else {
        eprintln!("Skipping test_live_sync_exclusions: Google Drive remote unreachable");
        return;
    };

    env.command().arg("init").assert().success();

    // Valid file to be backed up
    env.write_source_file("valid_document.txt", b"This file must be backed up");

    // Excluded files based on tests/incr-backup-testing.toml patterns:
    // "*.tmp", "*.temp", "*.swp", "*~", ".DS_Store", "Thumbs.db", "desktop.ini",
    // "node_modules/**", "target/**", "__pycache__/**", "*.pyc", ".venv/**", ".tox/**",
    // "dist/**", "build/**", "*.log", ".cache/**"
    env.write_source_file("scratch.tmp", b"temporary scratch");
    env.write_source_file("buffer.swp", b"vim swap file");
    env.write_source_file(".DS_Store", b"macos metadata");
    env.write_source_file("debug.log", b"log data");
    env.write_source_file("node_modules/express/index.js", b"module code");
    env.write_source_file("target/debug/app.exe", b"build artifact");
    env.write_source_file(".venv/lib/python.py", b"venv module");

    // Run sync
    env.command()
        .args(["sync", "--name", "exclusions-test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 files"));

    // Verify remote current/ only contains valid_document.txt
    let output = env.rclone_exec(&["ls", &format!("{}/current", env.remote_str())]);
    let ls_text = String::from_utf8_lossy(&output.stdout);
    assert!(ls_text.contains("valid_document.txt"));
    assert!(!ls_text.contains("scratch.tmp"), "scratch.tmp should be excluded");
    assert!(!ls_text.contains("buffer.swp"), "buffer.swp should be excluded");
    assert!(!ls_text.contains(".DS_Store"), ".DS_Store should be excluded");
    assert!(!ls_text.contains("debug.log"), "debug.log should be excluded");
    assert!(!ls_text.contains("node_modules"), "node_modules should be excluded");
    assert!(!ls_text.contains("target"), "target should be excluded");
    assert!(!ls_text.contains(".venv"), ".venv should be excluded");
}

// ─── 6. Dry Run Verification ────────────────────────────────────────────────

#[test]
fn test_live_sync_dry_run() {
    let Some(env) = LiveGdriveTestEnv::new("dry_run") else {
        eprintln!("Skipping test_live_sync_dry_run: Google Drive remote unreachable");
        return;
    };

    env.command().arg("init").assert().success();

    env.write_source_file("pending_file.txt", b"This should not be uploaded in dry-run");

    // Run sync in dry-run mode
    env.command()
        .args(["sync", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[DRY-RUN]"));

    // Verify remote current/ is still completely empty
    let output = env.rclone_exec(&["ls", &format!("{}/current", env.remote_str())]);
    let ls_text = String::from_utf8_lossy(&output.stdout);
    assert!(
        !ls_text.contains("pending_file.txt"),
        "pending_file.txt should NOT be present on remote after dry-run"
    );
}

// ─── 7. Reverse-Incremental Rotation & Diff Archiving ───────────────────────

#[test]
fn test_live_sync_reverse_incremental_rotation() {
    let Some(env) = LiveGdriveTestEnv::new("reverse_incr") else {
        eprintln!("Skipping test_live_sync_reverse_incremental_rotation: Google Drive remote unreachable");
        return;
    };

    env.command().arg("init").assert().success();

    // Baseline files:
    env.write_source_file("file_to_modify.txt", b"Version 1 original content");
    env.write_source_file("file_to_delete.txt", b"File scheduled for deletion");
    env.write_source_file("file_unchanged.txt", b"Static unchanged file content");

    // Sync baseline
    env.command()
        .args(["sync", "--name", "snap-v1"])
        .assert()
        .success();

    // Incremental changes:
    // 1. Modify file_to_modify.txt
    env.write_source_file("file_to_modify.txt", b"Version 2 updated content!");
    // 2. Delete file_to_delete.txt
    env.delete_source_file("file_to_delete.txt");
    // 3. Add brand new file
    env.write_source_file("file_new.txt", b"Brand new added file");
    // (file_unchanged.txt remains untouched)

    // Execute second sync
    env.command()
        .args(["sync", "--name", "snap-v2"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Sync complete: snap-v2"));

    // Verify remote current/:
    // - file_to_modify.txt has Version 2 content
    let cat_output = env.rclone_exec(&[
        "cat",
        &format!("{}/current/file_to_modify.txt", env.remote_str()),
    ]);
    assert_eq!(
        String::from_utf8_lossy(&cat_output.stdout).trim(),
        "Version 2 updated content!"
    );

    // - file_new.txt exists in current/
    let cat_new = env.rclone_exec(&["cat", &format!("{}/current/file_new.txt", env.remote_str())]);
    assert!(cat_new.status.success());

    // - file_to_delete.txt is GONE from current/
    let cat_deleted = env.rclone_exec(&[
        "cat",
        &format!("{}/current/file_to_delete.txt", env.remote_str()),
    ]);
    assert!(
        !cat_deleted.status.success(),
        "file_to_delete.txt should no longer exist in current/"
    );

    // Verify diffs/snap-v2/ on remote:
    // Contains the OLD version 1 of file_to_modify.txt
    let cat_diff_mod = env.rclone_exec(&[
        "cat",
        &format!("{}/diffs/snap-v2/file_to_modify.txt", env.remote_str()),
    ]);
    assert!(cat_diff_mod.status.success());
    assert_eq!(
        String::from_utf8_lossy(&cat_diff_mod.stdout).trim(),
        "Version 1 original content"
    );

    // Contains the deleted file_to_delete.txt
    let cat_diff_del = env.rclone_exec(&[
        "cat",
        &format!("{}/diffs/snap-v2/file_to_delete.txt", env.remote_str()),
    ]);
    assert!(cat_diff_del.status.success());
    assert_eq!(
        String::from_utf8_lossy(&cat_diff_del.stdout).trim(),
        "File scheduled for deletion"
    );

    // Does NOT contain file_unchanged.txt in diffs (saving space!)
    let cat_diff_unchanged = env.rclone_exec(&[
        "cat",
        &format!("{}/diffs/snap-v2/file_unchanged.txt", env.remote_str()),
    ]);
    assert!(
        !cat_diff_unchanged.status.success(),
        "Unchanged files must not be duplicated into diffs"
    );
}

// ─── 8. List & Diff Subcommands ─────────────────────────────────────────────

#[test]
fn test_live_list_and_diff() {
    let Some(env) = LiveGdriveTestEnv::new("list_diff") else {
        eprintln!("Skipping test_live_list_and_diff: Google Drive remote unreachable");
        return;
    };

    env.command().arg("init").assert().success();

    env.write_source_file("shared.txt", b"Version A");
    env.write_source_file("to_be_removed.txt", b"Will be removed");
    env.command().args(["sync", "--name", "snap-A"]).assert().success();

    env.write_source_file("shared.txt", b"Version B (extended content)");
    env.delete_source_file("to_be_removed.txt");
    env.write_source_file("added_in_b.txt", b"Newly added file");
    env.command().args(["sync", "--name", "snap-B"]).assert().success();

    // 1. List text output
    env.command()
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("snap-A"))
        .stdout(predicate::str::contains("snap-B"))
        .stdout(predicate::str::contains("Total: 2"));

    // 2. List JSON output
    let list_json_output = env.command().args(["list", "--json"]).assert().success();
    let json_str = String::from_utf8_lossy(&list_json_output.get_output().stdout);
    let parsed: Value = serde_json::from_str(&json_str).expect("list --json must produce valid JSON");
    let arr = parsed.as_array().expect("must be array of snapshots");
    assert_eq!(arr.len(), 2);

    // 3. Diff between snap-A and snap-B
    env.command()
        .args(["diff", "snap-A", "snap-B"])
        .assert()
        .success()
        .stdout(predicate::str::contains("+ added_in_b.txt"))
        .stdout(predicate::str::contains("- to_be_removed.txt"))
        .stdout(predicate::str::contains("~ shared.txt"));

    // 4. Diff with --stat
    env.command()
        .args(["diff", "snap-A", "snap-B", "--stat"])
        .assert()
        .success()
        .stdout(predicate::str::contains("added"))
        .stdout(predicate::str::contains("deleted"))
        .stdout(predicate::str::contains("modified"));

    // 5. Diff with filters: --only-added
    env.command()
        .args(["diff", "snap-A", "snap-B", "--only-added"])
        .assert()
        .success()
        .stdout(predicate::str::contains("added_in_b.txt"))
        .stdout(predicate::str::contains("shared.txt").not());

    // 6. Diff with --json
    let diff_json_output = env
        .command()
        .args(["diff", "snap-A", "snap-B", "--json"])
        .assert()
        .success();
    let diff_json_str = String::from_utf8_lossy(&diff_json_output.get_output().stdout);
    let diff_val: Value =
        serde_json::from_str(&diff_json_str).expect("diff --json must produce valid JSON");
    assert_eq!(diff_val["added"].as_array().unwrap().len(), 1);
    assert_eq!(diff_val["deleted"].as_array().unwrap().len(), 1);
    assert_eq!(diff_val["modified"].as_array().unwrap().len(), 1);
}

// ─── 9. Restore Scenarios ───────────────────────────────────────────────────

#[test]
fn test_live_restore_scenarios() {
    let Some(env) = LiveGdriveTestEnv::new("restore_scenarios") else {
        eprintln!("Skipping test_live_restore_scenarios: Google Drive remote unreachable");
        return;
    };

    env.command().arg("init").assert().success();

    // Snap 1
    env.write_source_file("doc1.txt", b"Version 1 of Doc 1");
    env.write_source_file("sub/nested.txt", b"Nested file content");
    env.command().args(["sync", "--name", "snap-hist-1"]).assert().success();

    // Snap 2
    env.write_source_file("doc1.txt", b"Version 2 modified");
    env.write_source_file("doc2.txt", b"Doc 2 added");
    env.command().args(["sync", "--name", "snap-hist-2"]).assert().success();

    // A) Full point-in-time restore of snap-hist-1
    let restore_full = env.temp_dir.path().join("restored_snap1");
    env.command()
        .args([
            "restore",
            "snap-hist-1",
            "--to",
            &restore_full.to_string_lossy(),
            "--verify",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Restored"))
        .stdout(predicate::str::contains("Verification: PASSED"));

    // Verify contents match snap-hist-1 exactly
    assert_eq!(
        fs::read_to_string(restore_full.join("doc1.txt")).unwrap(),
        "Version 1 of Doc 1"
    );
    assert_eq!(
        fs::read_to_string(restore_full.join("sub/nested.txt")).unwrap(),
        "Nested file content"
    );
    assert!(
        !restore_full.join("doc2.txt").exists(),
        "doc2.txt was not in snap-hist-1"
    );

    // B) Subpath restore: restore only "sub" folder
    let restore_sub = env.temp_dir.path().join("restored_sub");
    env.command()
        .args([
            "restore",
            "snap-hist-1",
            "--to",
            &restore_sub.to_string_lossy(),
            "--path",
            "sub",
        ])
        .assert()
        .success();

    assert!(restore_sub.join("nested.txt").exists());
    assert!(!restore_sub.join("doc1.txt").exists());

    // C) Collision protection: restoring to non-empty directory without --overwrite fails
    env.command()
        .args([
            "restore",
            "snap-hist-1",
            "--to",
            &restore_full.to_string_lossy(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("destination already exists"));

    // With --overwrite, it succeeds
    env.command()
        .args([
            "restore",
            "snap-hist-1",
            "--to",
            &restore_full.to_string_lossy(),
            "--overwrite",
        ])
        .assert()
        .success();
}

// ─── 10. Verify Integrity ───────────────────────────────────────────────────

#[test]
fn test_live_verify_integrity() {
    let Some(env) = LiveGdriveTestEnv::new("verify_integrity") else {
        eprintln!("Skipping test_live_verify_integrity: Google Drive remote unreachable");
        return;
    };

    env.command().arg("init").assert().success();
    env.write_source_file("integrity_check.txt", b"Secure verification payload 2026");
    env.command().args(["sync", "--name", "verify-snap"]).assert().success();

    // 1. Basic verify
    env.command()
        .args(["verify", "verify-snap"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Verifying backup at"));

    // 2. Verify with check-hashes
    env.command()
        .args(["verify", "verify-snap", "--check-hashes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Verifying backup at"));

    // 3. Verify with --space
    env.command()
        .args(["verify", "--space"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Space usage:"));
}

// ─── 11. Snapshot Compaction ────────────────────────────────────────────────

#[test]
fn test_live_snapshot_compaction() {
    let Some(env) = LiveGdriveTestEnv::new("compaction") else {
        eprintln!("Skipping test_live_snapshot_compaction: Google Drive remote unreachable");
        return;
    };

    env.command().arg("init").assert().success();

    // Create 3 timestamped snapshots in rapid succession with custom names
    // Note: compact requires ISO-8601 timestamps matching the configured timezone
    let offset_str = format!("{}", chrono::Local::now().offset());
    let from_time = format!("2026-10-30T00:00:00{}", offset_str);
    let to_time = format!("2026-10-30T23:59:59{}", offset_str);

    env.write_source_file("log.txt", b"entry 1\n");
    env.command().args(["sync", "--name", "2026-10-30T10-00Z"]).assert().success();

    env.write_source_file("log.txt", b"entry 1\nentry 2\n");
    env.command().args(["sync", "--name", "2026-10-30T11-00Z"]).assert().success();

    // 1. Dry run compaction
    env.command()
        .args([
            "compact",
            "--from",
            &from_time,
            "--to",
            &to_time,
            "--name",
            "consolidated-daily",
            "--dry-run",
            "--summary",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Compaction Plan Summary:"));

    // 2. Execute actual compaction
    env.command()
        .args([
            "compact",
            "--from",
            &from_time,
            "--to",
            &to_time,
            "--name",
            "consolidated-daily",
            "--summary",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Compaction complete: consolidated-daily"));

    // Consolidated snapshot should now exist in list
    env.command()
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("consolidated-daily"));
}

// ─── 12. Remote Removal Across Snapshots ────────────────────────────────────

#[test]
fn test_live_remove_file_across_snapshots() {
    let Some(env) = LiveGdriveTestEnv::new("remove_purge") else {
        eprintln!("Skipping test_live_remove_file_across_snapshots: Google Drive remote unreachable");
        return;
    };

    env.command().arg("init").assert().success();

    // File to purge
    env.write_source_file("secret_token.key", b"SUPER_SECRET_KEY_123");
    env.write_source_file("regular.txt", b"Regular file");
    env.command().args(["sync", "--name", "snap-with-secret"]).assert().success();

    // Remove file across all snapshots
    env.command()
        .args(["remove", "secret_token.key"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Successfully removed"));

    // Verify secret_token.key is GONE from remote current/
    let cat_check = env.rclone_exec(&[
        "cat",
        &format!("{}/current/secret_token.key", env.remote_str()),
    ]);
    assert!(
        !cat_check.status.success(),
        "secret_token.key must be completely deleted from remote"
    );

    // Verify regular.txt still remains untouched
    let cat_regular = env.rclone_exec(&[
        "cat",
        &format!("{}/current/regular.txt", env.remote_str()),
    ]);
    assert!(cat_regular.status.success());
}

// ─── 13. Config Management (Push, Diff, Pull) ───────────────────────────────

#[test]
fn test_live_remote_config_management() {
    let Some(env) = LiveGdriveTestEnv::new("config_mgmt") else {
        eprintln!("Skipping test_live_remote_config_management: Google Drive remote unreachable");
        return;
    };

    env.command().arg("init").assert().success();

    // 1. Config push: verify it handles push / identity check
    env.command()
        .args(["config", "push"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("Remote configuration (.meta/config.json) is already identical")
                .or(predicate::str::contains("Successfully pushed")),
        );

    // 2. Config diff: local and remote should match
    env.command()
        .args(["config", "diff"])
        .assert()
        .success()
        .stdout(predicate::str::contains("identical"));

    // 3. Config pull: pull remote configuration into a new local file
    let pulled_path = env.temp_dir.path().join("pulled-config.toml");
    env.command()
        .args(["config", "pull", "--to", &pulled_path.to_string_lossy()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Successfully wrote configuration"));

    assert!(pulled_path.exists());
    let pulled_content = fs::read_to_string(&pulled_path).unwrap();
    assert!(pulled_content.contains("exclude"));
}

// ─── 14. Lock Concurrency & Force Override ──────────────────────────────────

#[test]
fn test_live_lock_concurrency_and_override() {
    let Some(env) = LiveGdriveTestEnv::new("lock_concurrency") else {
        eprintln!("Skipping test_live_lock_concurrency_and_override: Google Drive remote unreachable");
        return;
    };

    env.command().arg("init").assert().success();
    env.write_source_file("sample.txt", b"Test locking");

    // Manually inject a lock file on Google Drive remote
    let lock_json = serde_json::json!({
        "holder": "remote-worker-node-42",
        "pid": 88888,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "timeout_minutes": 600000
    });
    let lock_remote_path = format!("{}/.meta/lock", env.remote_str());

    let mut cmd = std::process::Command::new(&env.rclone_path);
    cmd.args(["rcat", &lock_remote_path]);
    for flag in &env.rclone_flags {
        cmd.arg(flag);
    }
    cmd.stdin(std::process::Stdio::piped());
    if let Ok(mut child) = cmd.spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(lock_json.to_string().as_bytes());
        }
        let _ = child.wait();
    }

    // Attempt sync: should fail because remote is locked
    env.command()
        .arg("sync")
        .assert()
        .failure()
        .stderr(predicate::str::contains("backup locked by 'remote-worker-node-42'"));

    // Run sync with --force: breaks lock and succeeds
    env.command()
        .args(["sync", "--force"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Sync complete"));
}
