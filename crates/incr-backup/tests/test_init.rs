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
    // Simulate preexisting files and directories directly in remote
    env.write_remote_file("file1.txt", b"Content 1");
    env.write_remote_file("subdir/file2.txt", b"Content 2");
    std::fs::create_dir_all(env.remote_path.join("empty_dir")).unwrap();

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
    assert!(!env.remote_path.join("subdir").exists());
    assert!(!env.remote_path.join("empty_dir").exists());
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

    // Simulate an incomplete/interrupted structure: has .meta/ and unmanaged file at root
    env.write_remote_file("unadopted.txt", b"unadopted data");

    // Run first
    env.command()
        .args([
            "init",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--adopt",
            "--exclude",
            "*.tmp",
        ])
        .assert()
        .success();

    // Verify config was created
    let config_content = std::fs::read_to_string(env.remote_path.join(".meta/config.json")).unwrap();
    assert!(config_content.contains("*.tmp"));

    // Now remove diffs/ to simulate partial structure
    let _ = std::fs::remove_dir_all(env.remote_path.join("diffs"));
    assert!(!env.remote_path.join("diffs").exists());

    // Stage incomplete adopt journal
    let journal = serde_json::json!({
        "operation": "init_adopt",
        "snapshot_name": "current",
        "started_at": chrono::Utc::now().to_rfc3339(),
        "backup_dir": "",
        "steps": [
            { "name": "move_root_items", "status": "Pending" },
            { "name": "create_structure", "status": "Pending" },
            { "name": "upload_config", "status": "Pending" },
            { "name": "generate_manifest", "status": "Pending" },
            { "name": "upload_manifest", "status": "Pending" },
            { "name": "cleanup_journal", "status": "Pending" }
        ]
    });
    std::fs::write(
        env.remote_path.join(".meta/journal.json"),
        serde_json::to_string_pretty(&journal).unwrap(),
    )
    .unwrap();

    // Add another unmanaged file at remote root
    env.write_remote_file("new_file.txt", b"new content");

    // Re-run with --adopt and new exclude
    env.command()
        .args([
            "init",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--adopt",
            "--exclude",
            "*.log",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[WARN] Adopting existing remote contents into 'current/'"));

    // Verify diffs was recreated
    assert!(env.remote_path.join("diffs").exists());
    // Verify new file was moved into current/
    assert!(env.remote_path.join("current/new_file.txt").exists());
    // Verify config was overwritten with new exclude
    let overwritten_config = std::fs::read_to_string(env.remote_path.join(".meta/config.json")).unwrap();
    assert!(overwritten_config.contains("*.log"));
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

#[test]
fn test_init_from_config_file_without_flags() {
    let env = TestEnv::new();
    env.write_source_file("hello.txt", b"Hello from config");

    let cfg_path = env.temp_dir.path().join("incr-backup.toml");
    let toml = format!(
        r#"[source]
path = "{}"

[remote]
path = "{}"

[exclude]
patterns = ["*.bak"]
"#,
        env.source_str(),
        env.remote_str()
    );
    std::fs::write(&cfg_path, &toml).unwrap();

    env.command()
        .args(["-c", cfg_path.to_str().unwrap(), "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Initialized"));

    // Verify remote structure
    assert!(env.remote_path.join("current").exists());
    assert!(env.remote_path.join("diffs").exists());
    assert!(env.remote_path.join(".meta").exists());
    assert!(env.remote_path.join(".meta/config.json").exists());
    assert!(env.remote_path.join(".meta/manifests/current.json").exists());

    // Verify uploaded remote config matches local config
    let config_content = std::fs::read_to_string(env.remote_path.join(".meta/config.json")).unwrap();
    assert!(config_content.contains("*.bak"));
}

#[test]
fn test_init_config_with_cli_override() {
    let env = TestEnv::new();
    env.write_source_file("hello.txt", b"Hello");

    // Other source and remote directories
    let other_source = env.temp_dir.path().join("other_source");
    std::fs::create_dir_all(&other_source).unwrap();
    std::fs::write(other_source.join("other.txt"), b"Other").unwrap();

    let other_remote = env.temp_dir.path().join("other_remote");
    std::fs::create_dir_all(&other_remote).unwrap();

    let cfg_path = env.temp_dir.path().join("incr-backup.toml");
    let toml = format!(
        r#"[source]
path = "{}"

[remote]
path = "{}"
"#,
        env.source_str(),
        env.remote_str()
    );
    std::fs::write(&cfg_path, &toml).unwrap();

    // Override remote via CLI flag
    env.command()
        .args([
            "-c",
            cfg_path.to_str().unwrap(),
            "init",
            "--remote",
            other_remote.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Initialized"));

    // other_remote should be initialized, not env.remote_path
    assert!(other_remote.join("current").exists());
    assert!(!env.remote_path.join("current").exists());
}

#[test]
fn test_init_fails_when_source_missing() {
    let env = TestEnv::new();
    let cfg_path = env.temp_dir.path().join("no-source.toml");
    let toml = format!(
        r#"[remote]
path = "{}"
"#,
        env.remote_str()
    );
    std::fs::write(&cfg_path, &toml).unwrap();

    env.command()
        .args(["-c", cfg_path.to_str().unwrap(), "init"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("source path is required"));
}

#[test]
fn test_init_fails_when_remote_missing() {
    let env = TestEnv::new();
    let cfg_path = env.temp_dir.path().join("no-remote.toml");
    let toml = format!(
        r#"[source]
path = "{}"
"#,
        env.source_str()
    );
    std::fs::write(&cfg_path, &toml).unwrap();

    env.command()
        .args(["-c", cfg_path.to_str().unwrap(), "init"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("remote path is required"));
}

#[test]
fn test_init_adopt_fails_when_current_exists_without_journal() {
    let env = TestEnv::new();
    env.write_source_file("source.txt", b"source data");
    // Pre-create current/ directory without any journal
    env.write_remote_file("current/unmanaged.txt", b"unmanaged data");

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
        .failure()
        .stderr(predicate::str::contains(
            "already contains 'current/' without an in-progress adopt journal",
        ))
        .stderr(predicate::str::contains(
            "manually rename or delete 'current/'",
        ));
}

#[test]
fn test_init_adopt_resumes_when_incomplete_journal_present() {
    let env = TestEnv::new();
    env.write_source_file("source.txt", b"source data");
    env.write_remote_file("current/already_moved.txt", b"moved data");

    // Stage incomplete adopt journal where move_root_items was Done, create_structure was Pending
    let journal = serde_json::json!({
        "operation": "init_adopt",
        "snapshot_name": "current",
        "started_at": chrono::Utc::now().to_rfc3339(),
        "backup_dir": "",
        "steps": [
            { "name": "move_root_items", "status": "Done" },
            { "name": "create_structure", "status": "Pending" },
            { "name": "upload_config", "status": "Pending" },
            { "name": "generate_manifest", "status": "Pending" },
            { "name": "upload_manifest", "status": "Pending" },
            { "name": "cleanup_journal", "status": "Pending" }
        ]
    });
    std::fs::create_dir_all(env.remote_path.join(".meta")).unwrap();
    std::fs::write(
        env.remote_path.join(".meta/journal.json"),
        serde_json::to_string_pretty(&journal).unwrap(),
    )
    .unwrap();

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
        .stdout(predicate::str::contains("Found incomplete adopt journal"))
        .stdout(predicate::str::contains("Resuming adoption"))
        .stdout(predicate::str::contains("Successfully adopted 1 files"));

    // Journal should be cleaned up after successful adoption
    assert!(!env.remote_path.join(".meta/journal.json").exists());
    assert!(env.remote_path.join(".meta/manifests/current.json").exists());
}

#[test]
fn test_init_adopt_fails_on_missing_hashes_without_flag() {
    use std::os::unix::fs::PermissionsExt;

    let env = TestEnv::new();
    env.write_source_file("source.txt", b"source data");
    env.write_remote_file("data.txt", b"some content to adopt");

    // Create a mock rclone wrapper that strips md5 from lsjson output
    let mock_path = env.temp_dir.path().join("mock_rclone.sh");
    let script = r#"#!/bin/sh
if [ "$1" = "lsjson" ]; then
    rclone "$@" | sed 's/"md5"/"other_hash"/g'
else
    exec rclone "$@"
fi
"#;
    std::fs::write(&mock_path, script).unwrap();
    std::fs::set_permissions(&mock_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    env.command()
        .args([
            "init",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--adopt",
            "--rclone-path",
            mock_path.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("missing precomputed MD5 hashes"))
        .stderr(predicate::str::contains("pass '--download-to-hash'"));
}

#[test]
fn test_init_adopt_succeeds_with_download_to_hash_flag() {
    use std::os::unix::fs::PermissionsExt;

    let env = TestEnv::new();
    env.write_source_file("source.txt", b"source data");
    env.write_remote_file("data.txt", b"some content to adopt");

    // Create a mock rclone wrapper that strips md5 from lsjson output
    let mock_path = env.temp_dir.path().join("mock_rclone_download.sh");
    let script = r#"#!/bin/sh
if [ "$1" = "lsjson" ]; then
    rclone "$@" | sed 's/"md5"/"other_hash"/g'
else
    exec rclone "$@"
fi
"#;
    std::fs::write(&mock_path, script).unwrap();
    std::fs::set_permissions(&mock_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    env.command()
        .args([
            "init",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--adopt",
            "--download-to-hash",
            "--rclone-path",
            mock_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Successfully adopted 1 files"))
        .stdout(predicate::str::contains("Initialized"));

    // Verify manifest has non-empty valid 32-character MD5 hash
    let manifest_str = std::fs::read_to_string(
        env.remote_path.join(".meta/manifests/current.json"),
    )
    .unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest_str).unwrap();
    let md5 = manifest["files"][0]["md5"].as_str().unwrap();
    assert_eq!(md5.len(), 32);
}

#[test]
fn test_init_adopt_uses_dir_move_timing() {
    let env = TestEnv::new();
    env.write_source_file("source.txt", b"source data");

    // Setup multiple root directories and root files
    env.write_remote_file("folder_a/item1.txt", b"content 1");
    env.write_remote_file("folder_b/item2.txt", b"content 2");
    env.write_remote_file("root_file.txt", b"content root");

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
        .stdout(predicate::str::contains("[1/5] Inspecting remote status..."))
        .stdout(predicate::str::contains("[2/5] Adopting root entries into 'current/'..."))
        .stdout(predicate::str::contains("- Moved 2 directories and 1 files (DirMove)..."))
        .stdout(predicate::str::contains("[3/5] Creating remote structure and config..."))
        .stdout(predicate::str::contains("[4/5] Scanning adopted files for manifest (fast-list)..."))
        .stdout(predicate::str::contains("[5/5] Generating and committing initial manifest..."))
        .stdout(predicate::str::contains("Successfully adopted 3 files"));
}
