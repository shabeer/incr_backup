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
fn test_init_empty_remote_silently_ignores_adopt() {
    let env = TestEnv::new();
    env.write_source_file("hello.txt", b"Hello from source");

    // Remote is empty, pass --adopt
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
        .stdout(predicate::str::contains("Initialized"))
        .stdout(predicate::str::contains("Adopting existing files").not())
        .stdout(predicate::str::contains("[WARN]").not());

    // Verify remote structure
    assert!(env.remote_path.join("current").exists());
    assert!(env.remote_path.join("diffs").exists());
    assert!(env.remote_path.join(".meta").exists());
    assert!(env.remote_path.join(".meta/config.json").exists());
    assert!(env.remote_path.join(".meta/manifests/current.json").exists());
}

#[test]
fn test_init_fails_without_adopt_if_remote_not_empty() {
    let env = TestEnv::new();
    env.write_source_file("hello.txt", b"Hello");
    // Write an unmanaged file into remote root
    env.write_remote_file("unmanaged.txt", b"I was already here");

    // Init without --adopt must fail with suggestion
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
        .stderr(predicate::str::contains("not an initialized backup namespace"))
        .stderr(predicate::str::contains("re-run with '--adopt'"));

    // Ensure no backup structure was created
    assert!(!env.remote_path.join("current").exists());
    assert!(!env.remote_path.join(".meta").exists());
    assert!(!env.remote_path.join("diffs").exists());
    assert!(env.remote_path.join("unmanaged.txt").exists());
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
        .stdout(predicate::str::contains("[WARN] Adopting existing remote contents into 'current/'"))
        .stdout(predicate::str::contains("Adopting existing files"))
        .stdout(predicate::str::contains("Adopted 2 files into current/"));

    // Verify files were moved into current/
    assert!(env.remote_path.join("current/file1.txt").exists());
    assert!(env.remote_path.join("current/subdir/file2.txt").exists());
    assert!(!env.remote_path.join("file1.txt").exists());
    assert!(env.remote_path.join("diffs").exists());
    assert!(env.remote_path.join(".meta/config.json").exists());
}

#[test]
fn test_init_already_initialized_is_noop() {
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

    // Second init is a NO-OP (exit 0)
    env.command()
        .args([
            "init",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[INFO] Remote already initialized"));

    // Second init with --adopt is also a NO-OP (exit 0, ignore --adopt)
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
        .stdout(predicate::str::contains("[INFO] Remote already initialized"));
}

#[test]
fn test_init_already_initialized_warns_on_stray_files() {
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

    // Add a stray file at root of initialized remote
    env.write_remote_file("stray_notes.txt", b"stray notes");

    // Init should succeed (NO-OP) but warn about stray file
    env.command()
        .args([
            "init",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[INFO] Remote already initialized"))
        .stderr(predicate::str::contains("[WARN] Unmanaged root files/directories detected"))
        .stderr(predicate::str::contains("stray_notes.txt"));
}

#[test]
fn test_init_adopt_on_partial_structure_overwrites_config() {
    let env = TestEnv::new();
    env.write_source_file("source.txt", b"source data");

    // Simulate an incomplete/interrupted structure: has .meta/ with minimal retention, and unmanaged file at root
    env.write_remote_file("unadopted.txt", b"unadopted data");

    // Run first with minimal
    env.command()
        .args([
            "init",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--adopt",
            "--retention",
            "minimal",
        ])
        .assert()
        .success();

    // Verify retention was minimal
    let config_content = std::fs::read_to_string(env.remote_path.join(".meta/config.json")).unwrap();
    assert!(config_content.contains("\"hourly\": 3"));

    // Now remove diffs/ to simulate partial structure
    let _ = std::fs::remove_dir_all(env.remote_path.join("diffs"));
    assert!(!env.remote_path.join("diffs").exists());

    // Add another unmanaged file at remote root
    env.write_remote_file("new_file.txt", b"new content");

    // Re-run with --adopt and --retention aggressive
    env.command()
        .args([
            "init",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--adopt",
            "--retention",
            "aggressive",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[WARN] Adopting existing remote contents into 'current/'"));

    // Verify diffs was recreated
    assert!(env.remote_path.join("diffs").exists());
    // Verify new file was moved into current/
    assert!(env.remote_path.join("current/new_file.txt").exists());
    // Verify config was overwritten with aggressive retention
    let overwritten_config = std::fs::read_to_string(env.remote_path.join(".meta/config.json")).unwrap();
    assert!(overwritten_config.contains("\"hourly\": 12"));
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
