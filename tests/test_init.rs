mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn test_init_basic() {
    let env = TestEnv::new();
    env.write_source_file("hello.txt", b"Hello, World!");

    env.command()
        .args([
            "init",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--retention",
            "default",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Initialized"));

    // Verify remote structure
    assert!(env.remote_path.join("current").exists());
    assert!(env.remote_path.join("diffs").exists());
    assert!(env.remote_path.join(".meta").exists());
    assert!(env.remote_path.join(".meta/config.json").exists());
    assert!(env.remote_path.join(".meta/manifests/current.json").exists());
}

#[test]
fn test_init_adopt_existing_files() {
    let env = TestEnv::new();
    // Simulate preexisting files directly in remote
    env.write_remote_file("file1.txt", b"Content 1");
    env.write_remote_file("subdir/file2.txt", b"Content 2");

    env.command()
        .args([
            "init",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--adopt",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Adopting existing files"));

    // Verify files were moved into current/
    assert!(env.remote_path.join("current/file1.txt").exists());
    assert!(env.remote_path.join("current/subdir/file2.txt").exists());
    assert!(!env.remote_path.join("file1.txt").exists());
}

#[test]
fn test_init_fails_if_already_initialized() {
    let env = TestEnv::new();
    env.write_source_file("test.txt", b"test");

    // First init
    env.command()
        .args([
            "init",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success();

    // Second init should fail
    env.command()
        .args([
            "init",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already initialized"));
}

#[test]
fn test_init_fails_nonexistent_source() {
    let env = TestEnv::new();
    let nonexistent = env.temp_dir.path().join("does_not_exist");

    env.command()
        .args([
            "init",
            "--source",
            &nonexistent.to_string_lossy(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("source directory not found"));
}

