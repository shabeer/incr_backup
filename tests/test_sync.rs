mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn test_sync_basic_and_incremental() {
    let env = TestEnv::new();
    env.write_source_file("doc1.txt", b"Initial content 1");
    env.write_source_file("doc2.txt", b"Initial content 2");

    // 1. Init
    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // 2. Initial sync
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--level",
            "hourly",
            "--force-rotate",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Sync complete"));

    assert!(env.remote_path.join("current/doc1.txt").exists());
    assert!(env.remote_path.join("current/doc2.txt").exists());

    // 3. Modify doc1.txt, delete doc2.txt, add doc3.txt
    env.write_source_file("doc1.txt", b"Modified content 1");
    env.delete_source_file("doc2.txt");
    env.write_source_file("doc3.txt", b"New content 3");

    // Run sync again with force-rotate
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--level",
            "hourly",
            "--force-rotate",
        ])
        .assert()
        .success();

    // In current/, doc1 is modified, doc3 exists, doc2 is gone
    let current_doc1 = std::fs::read_to_string(env.remote_path.join("current/doc1.txt")).unwrap();
    assert_eq!(current_doc1, "Modified content 1");
    assert!(!env.remote_path.join("current/doc2.txt").exists());
    assert!(env.remote_path.join("current/doc3.txt").exists());

    // In diffs/, old doc1 and deleted doc2 are preserved in backup-dir
    let diff_entries = std::fs::read_dir(env.remote_path.join("diffs")).unwrap();
    let diff_dirs: Vec<String> = diff_entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    assert!(!diff_dirs.is_empty(), "diffs directory should contain snapshot dirs");

    // 4. Test list command
    env.command()
        .args([
            "list",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("hourly."));

    // 5. Test status command
    env.command()
        .args([
            "status",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Backup status"));
}

#[test]
fn test_sync_dry_run() {
    let env = TestEnv::new();
    env.write_source_file("test.txt", b"test data");

    // Init first
    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // Dry-run sync
    env.command()
        .args([
            "--dry-run",
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--level",
            "hourly",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[DRY-RUN]"));
}

#[test]
fn test_sync_timezone_arguments() {
    let env = TestEnv::new();
    env.write_source_file("doc1.txt", b"Initial content");

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // 1. Sync with --timezone local (or --tz local)
    env.write_source_file("doc1.txt", b"Local timezone sync");
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--level",
            "hourly",
            "--force-rotate",
            "--timezone",
            "local",
        ])
        .assert()
        .success();

    let diff_entries = std::fs::read_dir(env.remote_path.join("diffs")).unwrap();
    let diff_dirs: Vec<String> = diff_entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    assert!(!diff_dirs.is_empty(), "expected local diff directory to be created");
    let local_snap = &diff_dirs[0];
    assert!(
        local_snap.contains('+') || local_snap.contains('-'),
        "Local snapshot should contain numeric offset: {}",
        local_snap
    );
    assert!(
        !local_snap.contains(':'),
        "Snapshot directory must not contain forbidden colons: {}",
        local_snap
    );

    // 2. Sync with --tz utc
    env.write_source_file("doc1.txt", b"UTC timezone sync");
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--level",
            "hourly",
            "--force-rotate",
            "--tz",
            "utc",
        ])
        .assert()
        .success();

    let diff_entries_2 = std::fs::read_dir(env.remote_path.join("diffs")).unwrap();
    let diff_dirs_2: Vec<String> = diff_entries_2
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    assert!(
        diff_dirs_2.iter().any(|d| d.ends_with('Z')),
        "At least one snapshot should end with Z (UTC indicator): {:?}",
        diff_dirs_2
    );
}

