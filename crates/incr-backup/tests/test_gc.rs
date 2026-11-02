mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn test_gc_requires_orphaned_diffs_flag() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    env.command()
        .args(["gc", "--remote", &env.remote_str()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No garbage collection task specified"))
        .stderr(predicate::str::contains("--orphaned-diffs"));
}

#[test]
fn test_gc_no_orphaned_diffs() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // Snapshot 1
    env.write_source_file("file1.txt", b"v1");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s1"])
        .assert()
        .success();

    // Snapshot 2 (generates diffs/s2)
    env.write_source_file("file1.txt", b"v2");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s2"])
        .assert()
        .success();

    // Run gc --orphaned-diffs when all diffs have matching manifests
    env.command()
        .args(["gc", "--orphaned-diffs", "--remote", &env.remote_str()])
        .assert()
        .success()
        .stdout(predicate::str::contains("No orphaned diff directories found"));

    // Ensure diffs/s2 is untouched
    let diffs_dir = env.remote_path.join("diffs");
    assert!(diffs_dir.join("s2").exists());
}

#[test]
fn test_gc_orphaned_diffs_purges_stale_directories_and_preserves_survivors() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // S1: initial
    env.write_source_file("fileA.txt", b"A v1");
    env.write_source_file("fileB.txt", b"B v1");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s1"])
        .assert()
        .success();

    // S2: modify A, delete B, add C -> diffs/s2
    env.write_source_file("fileA.txt", b"A v2");
    std::fs::remove_file(env.source_path.join("fileB.txt")).unwrap();
    env.write_source_file("fileC.txt", b"C v1");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s2"])
        .assert()
        .success();

    // S3: modify A, add D -> diffs/s3
    env.write_source_file("fileA.txt", b"A v3");
    env.write_source_file("fileD.txt", b"D v1");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s3"])
        .assert()
        .success();

    // S4: modify C -> diffs/s4
    env.write_source_file("fileC.txt", b"C v2");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s4"])
        .assert()
        .success();

    let diffs_dir = env.remote_path.join("diffs");
    let manifests_dir = env.remote_path.join(".meta/manifests");
    assert!(diffs_dir.join("s2").exists());
    assert!(diffs_dir.join("s3").exists());
    assert!(diffs_dir.join("s4").exists());

    // Simulate scenario: s2 manifest was deleted (e.g. before the bug fix, or accidental manifest removal)
    // and an abandoned leftover directory "stale_run" exists in diffs/
    let stale_dir = diffs_dir.join("stale_run");
    std::fs::create_dir_all(&stale_dir).unwrap();
    std::fs::write(stale_dir.join("abandoned.bin"), b"abandoned data").unwrap();

    std::fs::remove_file(manifests_dir.join("s2.json")).unwrap();

    // Test Dry-run
    env.command()
        .args([
            "gc",
            "--orphaned-diffs",
            "--dry-run",
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[DRY-RUN] Simulating garbage collection"))
        .stdout(predicate::str::contains("diffs/s2/"))
        .stdout(predicate::str::contains("diffs/stale_run/"))
        .stdout(predicate::str::contains("Summary: 2 orphaned diff directory/ies would be purged"));

    // Verify directories still exist after dry-run
    assert!(diffs_dir.join("s2").exists(), "s2 should remain after dry run");
    assert!(diffs_dir.join("stale_run").exists(), "stale_run should remain after dry run");

    // Test JSON output in dry-run
    env.command()
        .args([
            "gc",
            "--orphaned-diffs",
            "--dry-run",
            "--json",
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"total_orphaned_dirs\": 2"))
        .stdout(predicate::str::contains("\"dry_run\": true"));

    // Run actual GC
    env.command()
        .args([
            "gc",
            "--orphaned-diffs",
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Garbage collection at"))
        .stdout(predicate::str::contains("✓ diffs/s2/"))
        .stdout(predicate::str::contains("✓ diffs/stale_run/"))
        .stdout(predicate::str::contains("Successfully purged 2 orphaned diff directory/ies"));

    // Verify physical deletion
    assert!(!diffs_dir.join("s2").exists(), "diffs/s2 must be deleted");
    assert!(!diffs_dir.join("stale_run").exists(), "diffs/stale_run must be deleted");

    // Verify surviving snapshot diffs are retained
    assert!(diffs_dir.join("s3").exists(), "diffs/s3 must be preserved");
    assert!(diffs_dir.join("s4").exists(), "diffs/s4 must be preserved");

    // Verify surviving snapshots restore accurately
    let restore_s3_dir = env.temp_dir.path().join("restore_s3");
    env.command()
        .args([
            "restore",
            "s3",
            "--to",
            &restore_s3_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Verification: PASSED ✓"));

    assert_eq!(std::fs::read_to_string(restore_s3_dir.join("fileA.txt")).unwrap(), "A v3");
    assert_eq!(std::fs::read_to_string(restore_s3_dir.join("fileC.txt")).unwrap(), "C v1");
    assert_eq!(std::fs::read_to_string(restore_s3_dir.join("fileD.txt")).unwrap(), "D v1");

    let restore_s4_dir = env.temp_dir.path().join("restore_s4");
    env.command()
        .args([
            "restore",
            "s4",
            "--to",
            &restore_s4_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Verification: PASSED ✓"));

    assert_eq!(std::fs::read_to_string(restore_s4_dir.join("fileA.txt")).unwrap(), "A v3");
    assert_eq!(std::fs::read_to_string(restore_s4_dir.join("fileC.txt")).unwrap(), "C v2");
    assert_eq!(std::fs::read_to_string(restore_s4_dir.join("fileD.txt")).unwrap(), "D v1");

    // Verify .meta/snapshots.jsonl no longer references s2
    let catalog_path = env.remote_path.join(".meta/snapshots.jsonl");
    if catalog_path.exists() {
        let catalog_content = std::fs::read_to_string(&catalog_path).unwrap();
        assert!(!catalog_content.contains("\"name\":\"s2\""), "s2 should be cleaned from catalog");
    }
}

#[test]
fn test_gc_empty_remote() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    env.command()
        .args(["gc", "--orphaned-diffs", "--remote", &env.remote_str()])
        .assert()
        .success()
        .stdout(predicate::str::contains("No orphaned diff directories found"));
}

