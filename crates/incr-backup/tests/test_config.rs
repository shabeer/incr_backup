mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn test_config_push_pull_diff_and_warning() {
    let env = TestEnv::new();
    env.write_source_file("hello.txt", b"Hello World");

    // Initialize with exclude pattern ["*.tmp"]
    env.command()
        .args([
            "init",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--exclude",
            "*.tmp",
        ])
        .assert()
        .success();

    // Create a local config file with updated exclude patterns ["*.tmp", "*.log"]
    let local_cfg_path = env.temp_dir.path().join("my-config.toml");
    let local_toml = format!(
        r#"[source]
path = "{}"

[remote]
path = "{}"

[exclude]
patterns = ["*.tmp", "*.log"]
"#,
        env.source_str(),
        env.remote_str()
    );
    std::fs::write(&local_cfg_path, &local_toml).unwrap();

    // 1. Verify that `config diff` detects the difference between local and remote
    env.command()
        .args([
            "-c",
            local_cfg_path.to_str().unwrap(),
            "config",
            "diff",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Exclude Patterns: (DIFFER)"))
        .stdout(predicate::str::contains("Found 1 difference(s)"));

    // 2. Run `config push` to update remote metadata
    env.command()
        .args([
            "-c",
            local_cfg_path.to_str().unwrap(),
            "config",
            "push",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Updating remote metadata"))
        .stdout(predicate::str::contains("Successfully pushed local configuration to remote metadata"));

    // 3. Now `config diff` should report identical
    env.command()
        .args([
            "-c",
            local_cfg_path.to_str().unwrap(),
            "config",
            "diff",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Local configuration and remote metadata are identical"));

    // 4. Test `config pull` to a new file
    let pulled_cfg_path = env.temp_dir.path().join("pulled-config.toml");
    env.command()
        .args([
            "config",
            "pull",
            "--remote",
            &env.remote_str(),
            "--to",
            pulled_cfg_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Successfully wrote configuration"));

    let pulled_content = std::fs::read_to_string(&pulled_cfg_path).unwrap();
    assert!(pulled_content.contains("*.log"));
}

#[test]
fn test_config_file_with_timezone_local() {
    let env = TestEnv::new();
    env.write_source_file("file1.txt", b"Testing timezone in config");

    // Config file with timezone = local placed after [remote] (the common pattern)
    let cfg_path = env.temp_dir.path().join("local-tz-config.toml");
    let toml = format!(
        r#"[source]
path = "{}"

[remote]
path = "{}"

timezone = local
"#,
        env.source_str(),
        env.remote_str()
    );
    std::fs::write(&cfg_path, &toml).unwrap();

    // Init with this config
    env.command()
        .args(["-c", cfg_path.to_str().unwrap(), "init"])
        .assert()
        .success();

    // First sync
    env.command()
        .args([
            "-c",
            cfg_path.to_str().unwrap(),
            "sync",
        ])
        .assert()
        .success();

    // Modify source and sync again to trigger a historical diff snapshot
    env.write_source_file("file1.txt", b"Modified for snapshot");
    env.command()
        .args([
            "-c",
            cfg_path.to_str().unwrap(),
            "sync",
        ])
        .assert()
        .success();

    let diff_entries = std::fs::read_dir(env.remote_path.join("diffs")).unwrap();
    let diff_dirs: Vec<String> = diff_entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    assert!(!diff_dirs.is_empty(), "expected snapshot in diffs/");
    for snap in &diff_dirs {
        assert!(
            snap.contains('+') || snap.contains('-'),
            "Snapshot {} should use local timezone offset, not UTC",
            snap
        );
        assert!(
            !snap.ends_with('Z'),
            "Snapshot {} should not end with Z when timezone=local",
            snap
        );
    }
}
