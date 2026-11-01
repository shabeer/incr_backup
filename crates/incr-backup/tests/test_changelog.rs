mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn test_changelog_empty_remote() {
    let env = TestEnv::new();

    // Changelog on empty remote with no snapshots
    env.command()
        .args(["changelog", "--remote", &env.remote_str()])
        .assert()
        .success()
        .stdout(predicate::str::contains("No snapshots found in range."));

    // JSON on empty remote
    env.command()
        .args(["changelog", "--remote", &env.remote_str(), "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"initial\": null"))
        .stdout(predicate::str::contains("\"transitions\": []"));
}

#[test]
fn test_changelog_single_snapshot() {
    let env = TestEnv::new();
    env.write_source_file("file1.txt", b"hello");
    env.write_source_file("dir/file2.txt", b"world");

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
            "--name",
            "snap1",
        ])
        .assert()
        .success();

    // Human readable output for single snapshot
    env.command()
        .args(["changelog", "--remote", &env.remote_str()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Initial snapshot: snap1"))
        .stdout(predicate::str::contains("Files: 2"))
        .stdout(predicate::str::contains("Directories: 1"))
        .stdout(predicate::str::contains("(no subsequent snapshots to compare)"))
        // Must NOT list individual file names for initial set
        .stdout(predicate::str::contains("file1.txt").not())
        .stdout(predicate::str::contains("dir/file2.txt").not());

    // JSON output for single snapshot
    let assert_json = env
        .command()
        .args(["changelog", "--remote", &env.remote_str(), "--json"])
        .assert()
        .success();

    let stdout_json = String::from_utf8(assert_json.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout_json).unwrap();
    assert_eq!(parsed["initial"]["name"], "snap1");
    assert_eq!(parsed["initial"]["file_count"], 2);
    assert_eq!(parsed["initial"]["dir_count"], 1);
    assert_eq!(parsed["transitions"].as_array().unwrap().len(), 0);
}

#[test]
fn test_changelog_multi_snapshot_evolution_and_filters() {
    let env = TestEnv::new();

    // Snapshot 1 baseline
    env.write_source_file("root.txt", b"root v1");
    env.write_source_file("src/app.rs", b"app v1");
    env.write_source_file("docs/readme.md", b"readme v1");

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
            "--name",
            "snap1",
        ])
        .assert()
        .success();

    // Snapshot 2: update src/app.rs, delete docs/readme.md, add src/util.rs and config/app.toml
    env.write_source_file("src/app.rs", b"app v2 updated");
    env.delete_source_file("docs/readme.md");
    env.write_source_file("src/util.rs", b"util v1");
    env.write_source_file("config/app.toml", b"config v1");

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

    // Snapshot 3: add src/extra.rs
    env.write_source_file("src/extra.rs", b"extra v1");

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

    // 1. Full changelog (snap1 -> snap2 -> snap3)
    let full_assert = env
        .command()
        .args(["changelog", "--remote", &env.remote_str()])
        .assert()
        .success();

    let full_stdout = String::from_utf8(full_assert.get_output().stdout.clone()).unwrap();

    // Initial snapshot verification
    assert!(full_stdout.contains("Initial snapshot: snap1"));
    assert!(full_stdout.contains("Files: 3"));
    assert!(full_stdout.contains("Directories: 2"));

    // Snap 2 transition verification
    assert!(full_stdout.contains("Snapshot: snap2"));
    assert!(full_stdout.contains("[compared to snap1]"));
    assert!(full_stdout.contains("Files: 2 added, 1 deleted, 1 updated"));
    assert!(full_stdout.contains("+ config/app.toml"));
    assert!(full_stdout.contains("+ src/util.rs"));
    assert!(full_stdout.contains("- docs/readme.md"));
    assert!(full_stdout.contains("~ src/app.rs"));
    assert!(full_stdout.contains("Directories: 1 added, 1 deleted, 1 updated"));
    assert!(full_stdout.contains("+ config"));
    assert!(full_stdout.contains("- docs"));
    assert!(full_stdout.contains("~ src"));

    // Snap 3 transition verification
    assert!(full_stdout.contains("Snapshot: snap3"));
    assert!(full_stdout.contains("[compared to snap2]"));
    assert!(full_stdout.contains("Files: 1 added, 0 deleted, 0 updated"));
    assert!(full_stdout.contains("+ src/extra.rs"));
    assert!(full_stdout.contains("Directories: 0 added, 0 deleted, 1 updated"));

    // 2. Filter with --from snap2: snap2 is initial set, only snap3 is compared
    let from_assert = env
        .command()
        .args(["changelog", "--remote", &env.remote_str(), "--from", "snap2"])
        .assert()
        .success();

    let from_stdout = String::from_utf8(from_assert.get_output().stdout.clone()).unwrap();
    assert!(from_stdout.contains("Initial snapshot: snap2"));
    // Initial snapshot should show file count and dir count, but not individual file names
    assert!(from_stdout.contains("Files: 4")); // root.txt, src/app.rs, src/util.rs, config/app.toml
    assert!(from_stdout.contains("Directories: 2")); // src, config
    assert!(from_stdout.contains("Snapshot: snap3"));
    assert!(from_stdout.contains("[compared to snap2]"));
    assert!(!from_stdout.contains("Snapshot: snap2")); // snap2 is initial, not transition

    // 3. Filter with alias --start and --end: snap1 to snap2 only
    let range_assert = env
        .command()
        .args([
            "changelog",
            "--remote",
            &env.remote_str(),
            "--start",
            "snap1",
            "--end",
            "snap2",
        ])
        .assert()
        .success();

    let range_stdout = String::from_utf8(range_assert.get_output().stdout.clone()).unwrap();
    assert!(range_stdout.contains("Initial snapshot: snap1"));
    assert!(range_stdout.contains("Snapshot: snap2"));
    assert!(!range_stdout.contains("Snapshot: snap3"));

    // 4. JSON format verification
    let json_assert = env
        .command()
        .args(["changelog", "--remote", &env.remote_str(), "--json"])
        .assert()
        .success();

    let json_stdout = String::from_utf8(json_assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&json_stdout).unwrap();

    assert_eq!(json["initial"]["name"], "snap1");
    assert_eq!(json["initial"]["file_count"], 3);
    assert_eq!(json["initial"]["dir_count"], 2);

    let transitions = json["transitions"].as_array().unwrap();
    assert_eq!(transitions.len(), 2);

    // Transition 0 (snap2)
    assert_eq!(transitions[0]["snapshot"], "snap2");
    assert_eq!(transitions[0]["previous_snapshot"], "snap1");
    assert_eq!(transitions[0]["files"]["added_count"], 2);
    assert_eq!(transitions[0]["files"]["deleted_count"], 1);
    assert_eq!(transitions[0]["files"]["updated_count"], 1);
    assert_eq!(transitions[0]["directories"]["added_count"], 1);
    assert_eq!(transitions[0]["directories"]["deleted_count"], 1);
    assert_eq!(transitions[0]["directories"]["updated_count"], 1);

    // Transition 1 (snap3)
    assert_eq!(transitions[1]["snapshot"], "snap3");
    assert_eq!(transitions[1]["previous_snapshot"], "snap2");
    assert_eq!(transitions[1]["files"]["added_count"], 1);
    assert_eq!(transitions[1]["files"]["deleted_count"], 0);
    assert_eq!(transitions[1]["files"]["updated_count"], 0);
    assert_eq!(transitions[1]["directories"]["added_count"], 0);
    assert_eq!(transitions[1]["directories"]["deleted_count"], 0);
    assert_eq!(transitions[1]["directories"]["updated_count"], 1);
}

#[test]
fn test_changelog_to_timestamp_bounds_second_snapshot_on_or_before() {
    let env = TestEnv::new();

    env.write_source_file("f1.txt", b"v1");
    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // Snapshot 1
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "snap1"])
        .assert()
        .success();

    // Snapshot 2
    env.write_source_file("f2.txt", b"v2");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "snap2"])
        .assert()
        .success();

    // Sleep to ensure distinct second boundary between snapshots
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Snapshot 3
    env.write_source_file("f3.txt", b"v3");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "snap3"])
        .assert()
        .success();

    // Get timestamp of snap2 from manifest
    let snap2_manifest_path = format!("{}/.meta/manifests/snap2.json", env.remote_str());
    let manifest_content = std::fs::read_to_string(&snap2_manifest_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&manifest_content).unwrap();
    let snap2_ts_str = parsed["created_at"].as_str().unwrap();
    let snap2_dt: chrono::DateTime<chrono::Utc> = snap2_ts_str.parse().unwrap();

    // Timestamp 1 second after snap2 (strictly before snap3):
    // The second/ending snapshot must be on or before this timestamp -> snap2
    let ts_between_2_and_3 = (snap2_dt + chrono::Duration::seconds(1)).to_rfc3339();
    let assert_to = env
        .command()
        .args(["changelog", "--remote", &env.remote_str(), "--to", &ts_between_2_and_3])
        .assert()
        .success();

    let out_to = String::from_utf8(assert_to.get_output().stdout.clone()).unwrap();
    assert!(out_to.contains("Initial snapshot: snap1"));
    assert!(out_to.contains("Snapshot: snap2"));
    assert!(!out_to.contains("Snapshot: snap3"), "snap3 should not be included when --to is before snap3");

    // Exact timestamp of snap2: second snapshot must be snap2
    let assert_exact_to = env
        .command()
        .args(["changelog", "--remote", &env.remote_str(), "--to", &snap2_dt.to_rfc3339()])
        .assert()
        .success();

    let out_exact_to = String::from_utf8(assert_exact_to.get_output().stdout.clone()).unwrap();
    assert!(out_exact_to.contains("Initial snapshot: snap1"));
    assert!(out_exact_to.contains("Snapshot: snap2"));
    assert!(!out_exact_to.contains("Snapshot: snap3"));

    // Timestamp before snap1: no snapshots in range
    let ts_before_snap1 = (snap2_dt - chrono::Duration::hours(10)).to_rfc3339();
    env.command()
        .args(["changelog", "--remote", &env.remote_str(), "--to", &ts_before_snap1])
        .assert()
        .success()
        .stdout(predicate::str::contains("No snapshots found in range."));
}

