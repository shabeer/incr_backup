mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn test_remove_snapshots_basic_and_restore_after_timestamp() {
    let env = TestEnv::new();

    // 1. Initialize
    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // 2. Snapshot 1 (snap1)
    env.write_source_file("fileA.txt", b"A v1");
    env.write_source_file("fileB.txt", b"B v1");
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--name",
            "snap1",
        ])
        .assert()
        .success();

    // 3. Snapshot 2 (snap2)
    env.write_source_file("fileA.txt", b"A v2");
    env.write_source_file("fileC.txt", b"C v1");
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--name",
            "snap2",
        ])
        .assert()
        .success();

    // 4. Snapshot 3 (snap3)
    env.write_source_file("fileB.txt", b"B v2");
    env.write_source_file("fileD.txt", b"D v1");
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--name",
            "snap3",
        ])
        .assert()
        .success();

    // Verify 3 snapshots exist
    let manifests_dir = env.remote_path.join(".meta/manifests");
    assert!(manifests_dir.join("snap1.json").exists());
    assert!(manifests_dir.join("snap2.json").exists());
    assert!(manifests_dir.join("snap3.json").exists());

    // 5. Run `incr-backup remove_snapshots --to snap2`
    env.command()
        .args([
            "remove_snapshots",
            "--to",
            "snap2",
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Successfully deleted 2 snapshot(s)"))
        .stdout(predicate::str::contains("Remaining snapshots: 1"));

    // Verify snap1 and snap2 manifests are deleted
    assert!(!manifests_dir.join("snap1.json").exists(), "snap1.json must be deleted");
    assert!(!manifests_dir.join("snap2.json").exists(), "snap2.json must be deleted");

    // Verify snap3 manifest is preserved
    assert!(manifests_dir.join("snap3.json").exists(), "snap3.json must be preserved");

    // Verify .meta/snapshots.jsonl is updated
    let catalog_path = env.remote_path.join(".meta/snapshots.jsonl");
    assert!(catalog_path.exists(), "snapshots.jsonl must exist");
    let catalog_content = std::fs::read_to_string(&catalog_path).unwrap();
    assert!(!catalog_content.contains("snap1"), "snap1 must be removed from catalog");
    assert!(!catalog_content.contains("snap2"), "snap2 must be removed from catalog");
    assert!(catalog_content.contains("snap3"), "snap3 must remain in catalog");

    // Verify list command reflects the updated catalog
    env.command()
        .args(["list", "--remote", &env.remote_str()])
        .assert()
        .success()
        .stdout(predicate::str::contains("snap3"))
        .stdout(predicate::str::contains("Total: 1 snapshots"));

    // 6. Test restoring snap3 (which is timestamped after snap2)
    let restore_snap3_dir = env.temp_dir.path().join("restore_snap3");
    env.command()
        .args([
            "restore",
            "snap3",
            "--to",
            &restore_snap3_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Verification: PASSED ✓"));

    assert_eq!(
        std::fs::read_to_string(restore_snap3_dir.join("fileA.txt")).unwrap(),
        "A v2"
    );
    assert_eq!(
        std::fs::read_to_string(restore_snap3_dir.join("fileB.txt")).unwrap(),
        "B v2"
    );
    assert_eq!(
        std::fs::read_to_string(restore_snap3_dir.join("fileC.txt")).unwrap(),
        "C v1"
    );
    assert_eq!(
        std::fs::read_to_string(restore_snap3_dir.join("fileD.txt")).unwrap(),
        "D v1"
    );

    // 7. Test restoring current
    let restore_current_dir = env.temp_dir.path().join("restore_current");
    env.command()
        .args([
            "restore",
            "current",
            "--to",
            &restore_current_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success();

    assert_eq!(
        std::fs::read_to_string(restore_current_dir.join("fileA.txt")).unwrap(),
        "A v2"
    );
    assert_eq!(
        std::fs::read_to_string(restore_current_dir.join("fileB.txt")).unwrap(),
        "B v2"
    );

    // 8. Attempting to restore deleted snap1 must fail
    let restore_deleted_dir = env.temp_dir.path().join("restore_deleted");
    env.command()
        .args([
            "restore",
            "snap1",
            "--to",
            &restore_deleted_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .failure();
}

#[test]
fn test_remove_snapshots_restores_multiple_surviving_snapshots_with_diffs() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // S1: doc = v1, notes = v1
    env.write_source_file("doc.txt", b"v1 doc");
    env.write_source_file("notes.txt", b"v1 notes");
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--name",
            "s1",
        ])
        .assert()
        .success();

    // S2: doc = v2, notes = v1
    env.write_source_file("doc.txt", b"v2 doc");
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--name",
            "s2",
        ])
        .assert()
        .success();

    // S3: doc = v3, notes = v2
    env.write_source_file("doc.txt", b"v3 doc");
    env.write_source_file("notes.txt", b"v2 notes");
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--name",
            "s3",
        ])
        .assert()
        .success();

    // S4: doc = v4, notes = v2, new_file = v1
    env.write_source_file("doc.txt", b"v4 doc");
    env.write_source_file("new_file.txt", b"v1 new");
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--name",
            "s4",
        ])
        .assert()
        .success();

    // Delete snapshots up to s2 (s1 and s2 deleted, s3 and s4 kept)
    env.command()
        .args([
            "remove_snapshots",
            "--to",
            "s2",
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success();

    // Verify S3 restores to its exact state (doc = v3, notes = v2) with verification
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

    assert_eq!(
        std::fs::read_to_string(restore_s3_dir.join("doc.txt")).unwrap(),
        "v3 doc"
    );
    assert_eq!(
        std::fs::read_to_string(restore_s3_dir.join("notes.txt")).unwrap(),
        "v2 notes"
    );
    assert!(!restore_s3_dir.join("new_file.txt").exists());

    // Verify S4 restores to its exact state (doc = v4, notes = v2, new_file = v1)
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

    assert_eq!(
        std::fs::read_to_string(restore_s4_dir.join("doc.txt")).unwrap(),
        "v4 doc"
    );
    assert_eq!(
        std::fs::read_to_string(restore_s4_dir.join("notes.txt")).unwrap(),
        "v2 notes"
    );
    assert_eq!(
        std::fs::read_to_string(restore_s4_dir.join("new_file.txt")).unwrap(),
        "v1 new"
    );
}

#[test]
fn test_remove_snapshots_dry_run() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    env.write_source_file("a.txt", b"content");
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--name",
            "snap1",
        ])
        .assert()
        .success();

    // Run with --dry-run / -n
    env.command()
        .args([
            "remove_snapshots",
            "--to",
            "snap1",
            "--remote",
            &env.remote_str(),
            "-n",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[DRY-RUN] Simulating removal"))
        .stdout(predicate::str::contains("snap1"));

    // Manifest should STILL exist after dry-run
    let manifests_dir = env.remote_path.join(".meta/manifests");
    assert!(manifests_dir.join("snap1.json").exists(), "dry-run must not delete manifests");
}

#[test]
fn test_remove_snapshots_json_output() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    env.write_source_file("data.txt", b"data");
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--name",
            "snap1",
        ])
        .assert()
        .success();

    let output = env.command()
        .args([
            "remove_snapshots",
            "--to",
            "snap1",
            "--remote",
            &env.remote_str(),
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(json["to"], "snap1");
    assert_eq!(json["snapshots_removed"], 1);
    assert_eq!(json["snapshots_remaining"], 0);
    assert_eq!(json["dry_run"], false);
}

#[test]
fn test_remove_snapshots_no_matching_snapshots() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    env.write_source_file("data.txt", b"data");
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--name",
            "snap1",
        ])
        .assert()
        .success();

    // Query cutoff way in the past (e.g. 2000-01-01)
    env.command()
        .args([
            "remove_snapshots",
            "--to",
            "2000-01-01",
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("No matching snapshots found"));

    // snap1 must still exist
    let manifests_dir = env.remote_path.join(".meta/manifests");
    assert!(manifests_dir.join("snap1.json").exists());
}

#[test]
fn test_remove_snapshots_kebab_alias() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    env.write_source_file("file.txt", b"hello");
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--name",
            "snap1",
        ])
        .assert()
        .success();

    // Call using visible alias `remove-snapshots`
    env.command()
        .args([
            "remove-snapshots",
            "--to",
            "snap1",
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Successfully deleted 1 snapshot(s)"));

    let manifests_dir = env.remote_path.join(".meta/manifests");
    assert!(!manifests_dir.join("snap1.json").exists());
}
