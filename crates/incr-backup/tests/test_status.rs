mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn test_status_empty_remote() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    env.command()
        .args(["status", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Backup status for"))
        .stdout(predicate::str::contains("Current:    0 files, 0 B"))
        .stdout(predicate::str::contains("Diffs:      0 files, 0 B"));
}

#[test]
fn test_status_individual_diff_directories() {
    let env = TestEnv::new();
    env.write_source_file("file1.txt", b"Original file 1 content");
    env.write_source_file("file2.txt", b"Original file 2 content");

    // Init
    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // Sync 1: initial backup, no diffs yet
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

    // Modify file1.txt (creates snap2 in diffs with 1 file)
    env.write_source_file("file1.txt", b"Updated file 1 content");
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

    // Modify file2.txt and add file3.txt (creates snap3 in diffs with 1 file)
    env.write_source_file("file2.txt", b"Updated file 2 content longer");
    env.write_source_file("file3.txt", b"Brand new file 3 content");
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

    // Run status and verify each diff directory is reported with file count and size
    let status_output = env
        .command()
        .args([
            "status",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Backup status for"))
        .stdout(predicate::str::contains("Current:    3 files"))
        .stdout(predicate::str::contains("Diffs:      2 files"))
        .stdout(predicate::str::contains("  snap2: 1 files, 23 B"))
        .stdout(predicate::str::contains("  snap3: 1 files, 23 B"));

    let stdout_str = String::from_utf8(status_output.get_output().stdout.clone()).unwrap();

    // Verify ordering: snap2 should appear before snap3
    let snap2_pos = stdout_str.find("  snap2: 1 files, 23 B").expect("snap2 missing");
    let snap3_pos = stdout_str.find("  snap3: 1 files, 23 B").expect("snap3 missing");
    assert!(snap2_pos < snap3_pos, "Diff directories should be sorted");
}

