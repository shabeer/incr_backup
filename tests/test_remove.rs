mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn test_remove_single_file_from_all_snapshots() {
    let env = TestEnv::new();
    env.write_source_file("file1.txt", b"Regular file 1");
    env.write_source_file("secret.txt", b"Top secret initial v1");
    env.write_source_file("notes.txt", b"Meeting notes");

    // 1. Initialize
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
        ])
        .assert()
        .success();

    assert!(env.remote_path.join("current/secret.txt").exists());
    assert!(env.remote_path.join("current/file1.txt").exists());

    // 3. Modify secret.txt and file1.txt, then sync again to create diffs
    env.write_source_file("secret.txt", b"Top secret modified v2");
    env.write_source_file("file1.txt", b"Regular file 1 modified");

    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success();

    // Verify secret.txt is in current and also in diffs/
    assert!(env.remote_path.join("current/secret.txt").exists());
    let diffs_dir = env.remote_path.join("diffs");
    let mut found_secret_in_diffs = false;
    for entry in std::fs::read_dir(&diffs_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.path().join("secret.txt").exists() {
            found_secret_in_diffs = true;
            break;
        }
    }
    assert!(found_secret_in_diffs, "secret.txt should exist in diffs directory before remove");

    // 4. Run `incr-backup remove secret.txt`
    env.command()
        .args([
            "remove",
            "secret.txt",
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Successfully removed"));

    // 5. Verify secret.txt is GONE from current/
    assert!(!env.remote_path.join("current/secret.txt").exists(), "secret.txt must be removed from current/");

    // 6. Verify secret.txt is GONE from all diff directories
    for entry in std::fs::read_dir(&diffs_dir).unwrap() {
        let entry = entry.unwrap();
        assert!(!entry.path().join("secret.txt").exists(), "secret.txt must be removed from diffs/{}", entry.file_name().to_string_lossy());
    }

    // 7. Verify manifests no longer contain secret.txt
    let manifests_dir = env.remote_path.join(".meta/manifests");
    for entry in std::fs::read_dir(&manifests_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
            let content = std::fs::read_to_string(entry.path()).unwrap();
            let manifest: incr_backup::manifest::Manifest = serde_json::from_str(&content).unwrap();
            assert!(
                !manifest.files.iter().any(|f| f.path == "secret.txt"),
                "Manifest {} must not contain secret.txt",
                entry.file_name().to_string_lossy()
            );
        }
    }

    // 8. Other files must be preserved
    assert!(env.remote_path.join("current/file1.txt").exists());
    assert!(env.remote_path.join("current/notes.txt").exists());

    // 9. Verify backup integrity
    env.command()
        .args(["verify", "--remote", &env.remote_str()])
        .assert()
        .success()
        .stdout(predicate::str::contains("All checks passed"));
}

#[test]
fn test_remove_directory_from_all_snapshots() {
    let env = TestEnv::new();
    env.write_source_file("docs/report.txt", b"Annual Report");
    env.write_source_file("sensitive/passwords.txt", b"admin=1234");
    env.write_source_file("sensitive/sub/key.pem", b"PRIVATE KEY DATA");

    // 1. Initialize
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
        ])
        .assert()
        .success();

    // 3. Modify sensitive files and sync again
    env.write_source_file("sensitive/passwords.txt", b"admin=5678");
    env.write_source_file("sensitive/new_secret.txt", b"new confidential info");

    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success();

    assert!(env.remote_path.join("current/sensitive/passwords.txt").exists());
    assert!(env.remote_path.join("current/sensitive/sub/key.pem").exists());

    // 4. Run `incr-backup remove sensitive/`
    env.command()
        .args([
            "remove",
            "sensitive/",
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Successfully removed"));

    // 5. Verify entire sensitive/ directory is purged from current/
    assert!(!env.remote_path.join("current/sensitive").exists(), "sensitive/ dir must be removed from current/");

    // 6. Verify sensitive/ is purged from all diffs
    let diffs_dir = env.remote_path.join("diffs");
    for entry in std::fs::read_dir(&diffs_dir).unwrap() {
        let entry = entry.unwrap();
        assert!(!entry.path().join("sensitive").exists(), "sensitive/ must not exist in diffs/{}", entry.file_name().to_string_lossy());
    }

    // 7. Verify manifests do not contain any sensitive/* files
    let manifests_dir = env.remote_path.join(".meta/manifests");
    for entry in std::fs::read_dir(&manifests_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
            let content = std::fs::read_to_string(entry.path()).unwrap();
            let manifest: incr_backup::manifest::Manifest = serde_json::from_str(&content).unwrap();
            for file in &manifest.files {
                assert!(
                    !file.path.starts_with("sensitive/"),
                    "Manifest {} should not contain file in sensitive/: {}",
                    entry.file_name().to_string_lossy(),
                    file.path
                );
            }
        }
    }

    // 8. docs/report.txt is still intact
    assert!(env.remote_path.join("current/docs/report.txt").exists());

    // 9. Verify passes
    env.command()
        .args(["verify", "--remote", &env.remote_str()])
        .assert()
        .success();
}

#[test]
fn test_remove_dry_run() {
    let env = TestEnv::new();
    env.write_source_file("temp.log", b"Log data");
    env.write_source_file("app.rs", b"fn main() {}");

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success();

    // Run remove with --dry-run
    env.command()
        .args([
            "remove",
            "temp.log",
            "--remote",
            &env.remote_str(),
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[DRY-RUN]"));

    // File must STILL exist on remote because dry-run was used
    assert!(env.remote_path.join("current/temp.log").exists());
}

#[test]
fn test_remove_multiple_files_and_json() {
    let env = TestEnv::new();
    env.write_source_file("doc1.txt", b"Keep this");
    env.write_source_file("leak1.env", b"SECRET_KEY=abc");
    env.write_source_file("leak2.pem", b"CERTIFICATE");

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success();

    // Run remove with multiple positional args and --json
    let output = env.command()
        .args([
            "remove",
            "leak1.env",
            "leak2.pem",
            "--remote",
            &env.remote_str(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json_str = String::from_utf8(output).unwrap();
    let result: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(result["unique_files_removed"], 2);
    // 2 files in each of `current.json` and `hourly.0.json` = 4 instances total
    assert_eq!(result["total_file_instances_removed"], 4);
    assert_eq!(result["snapshots_affected"].as_array().unwrap().len(), 2);
    assert!(!env.remote_path.join("current/leak1.env").exists());
    assert!(!env.remote_path.join("current/leak2.pem").exists());
    assert!(env.remote_path.join("current/doc1.txt").exists());
}

#[test]
fn test_remove_alias_rm() {
    let env = TestEnv::new();
    env.write_source_file("file.txt", b"Hello");
    env.write_source_file("discard.txt", b"Discard me");

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success();

    // Use `rm` alias instead of `remove`
    env.command()
        .args([
            "rm",
            "discard.txt",
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Successfully removed"));

    assert!(!env.remote_path.join("current/discard.txt").exists());
}

#[test]
fn test_remove_nonexistent_target() {
    let env = TestEnv::new();
    env.write_source_file("file.txt", b"Hello");

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success();

    // Nonexistent target should not fail, but indicate nothing was found
    env.command()
        .args([
            "remove",
            "nonexistent.txt",
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("No matching files found"));
}

#[test]
fn test_remove_root_disallowed() {
    let env = TestEnv::new();
    env.write_source_file("file.txt", b"Hello");

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // Attempting to remove root "/" or "." should fail
    env.command()
        .args([
            "remove",
            "/",
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .failure();

    env.command()
        .args([
            "remove",
            ".",
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .failure();
}