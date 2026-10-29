mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn test_config_push_pull_diff_and_warning() {
    let env = TestEnv::new();
    env.write_source_file("hello.txt", b"Hello World");

    // Initialize with default retention (6h/7d/4w/6m)
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
        .success();

    // Create a local config file with minimal retention (3h/3d/2w/3m)
    let local_cfg_path = env.temp_dir.path().join("my-config.toml");
    let local_toml = format!(
        r#"[source]
path = "{}"

[remote]
path = "{}"

[retention]
hourly = 3
daily = 3
weekly = 2
monthly = 3
min_interval_minutes = 30
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
        .stdout(predicate::str::contains("Retention: (DIFFER)"))
        .stdout(predicate::str::contains("Found 1 difference(s)"));

    // 2. Verify that `status` displays a warning notice
    env.command()
        .args([
            "-c",
            local_cfg_path.to_str().unwrap(),
            "status",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Warning:  Local retention differs from remote metadata"))
        .stdout(predicate::str::contains("Run `incr-backup config push` to sync"));

    // 3. Verify that `verify` displays a warning notice
    env.command()
        .args([
            "-c",
            local_cfg_path.to_str().unwrap(),
            "verify",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("retention differs from local"));

    // 4. Run `config push` to update remote metadata
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

    // 5. Now `config diff` should report identical
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

    // 6. Test `config pull` to a new file
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
    assert!(pulled_content.contains("hourly = 3"));
    assert!(pulled_content.contains("daily = 3"));
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
            "--level",
            "hourly",
            "--force-rotate",
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
            "--level",
            "hourly",
            "--force-rotate",
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
