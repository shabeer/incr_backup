mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn test_restore_current() {
    let env = TestEnv::new();
    env.write_source_file("file1.txt", b"Content 1");
    env.write_source_file("subdir/file2.txt", b"Content 2");

    // Init & sync
    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--level", "hourly"])
        .assert()
        .success();

    // Restore to target directory
    let restore_dir = env.temp_dir.path().join("restored_current");
    env.command()
        .args([
            "restore",
            "current",
            "--to",
            &restore_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Restored"));

    assert!(restore_dir.join("file1.txt").exists());
    assert_eq!(
        std::fs::read_to_string(restore_dir.join("file1.txt")).unwrap(),
        "Content 1"
    );
    assert!(restore_dir.join("subdir/file2.txt").exists());
}

#[test]
fn test_restore_point_in_time() {
    let env = TestEnv::new();
    env.write_source_file("document.txt", b"Version 1");
    env.write_source_file("static.txt", b"Never changes");

    // Init
    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // Sync v1
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

    // Modify document.txt
    env.write_source_file("document.txt", b"Version 2");

    // Sync v2
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

    // Now restore hourly.1 (which contains Version 1 of document.txt)
    let restore_dir = env.temp_dir.path().join("restored_v1");
    env.command()
        .args([
            "restore",
            "hourly.1",
            "--to",
            &restore_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Restored"));

    // Verify document.txt is Version 1 and static.txt was also restored
    assert!(restore_dir.join("document.txt").exists());
    assert_eq!(
        std::fs::read_to_string(restore_dir.join("document.txt")).unwrap(),
        "Version 1"
    );
    assert!(restore_dir.join("static.txt").exists());
    assert_eq!(
        std::fs::read_to_string(restore_dir.join("static.txt")).unwrap(),
        "Never changes"
    );
}

#[test]
fn test_restore_dest_not_empty_requires_overwrite() {
    let env = TestEnv::new();
    env.write_source_file("doc.txt", b"data");

    // Init & sync
    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--level", "hourly"])
        .assert()
        .success();

    // Create a non-empty restore destination
    let restore_dir = env.temp_dir.path().join("non_empty");
    std::fs::create_dir_all(&restore_dir).unwrap();
    std::fs::write(restore_dir.join("existing.txt"), "preexisting").unwrap();

    // Restore without --overwrite fails
    env.command()
        .args([
            "restore",
            "current",
            "--to",
            &restore_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not empty"));

    // Restore with --overwrite succeeds
    env.command()
        .args([
            "restore",
            "current",
            "--to",
            &restore_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--overwrite",
        ])
        .assert()
        .success();
}
