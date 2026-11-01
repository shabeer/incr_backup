mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn test_list_empty_remote() {
    let env = TestEnv::new();

    // Plain text on empty remote without filters
    env.command()
        .args(["list", "--remote", &env.remote_str()])
        .assert()
        .success()
        .stdout(predicate::str::contains("(no snapshots yet)"))
        .stdout(predicate::str::contains("Total: 0 snapshots"));

    // Plain text on empty remote with --from and --to
    env.command()
        .args([
            "list",
            "--remote",
            &env.remote_str(),
            "--from",
            "snap1",
            "--to",
            "snap2",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("(no snapshots yet)"))
        .stdout(predicate::str::contains("Total: 0 snapshots"));

    // JSON output on empty remote
    let assert_json = env
        .command()
        .args(["list", "--remote", &env.remote_str(), "--json"])
        .assert()
        .success();

    let stdout_json = String::from_utf8(assert_json.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout_json).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 0);
}

#[test]
fn test_list_from_and_to_filters() {
    let env = TestEnv::new();

    // Initialize repo
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

    // Snapshot 3
    env.write_source_file("f3.txt", b"v3");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "snap3"])
        .assert()
        .success();

    // 1. Unfiltered list: shows all 3 snapshots
    let all_out = env
        .command()
        .args(["list", "--remote", &env.remote_str()])
        .assert()
        .success();
    let all_str = String::from_utf8(all_out.get_output().stdout.clone()).unwrap();
    assert!(all_str.contains("snap1"));
    assert!(all_str.contains("snap2"));
    assert!(all_str.contains("snap3"));
    assert!(all_str.contains("Total: 3 snapshots"));

    // 2. Filter --from snap2: snap2 and snap3
    let from_out = env
        .command()
        .args(["list", "--remote", &env.remote_str(), "--from", "snap2"])
        .assert()
        .success();
    let from_str = String::from_utf8(from_out.get_output().stdout.clone()).unwrap();
    assert!(!from_str.contains("snap1"));
    assert!(from_str.contains("snap2"));
    assert!(from_str.contains("snap3"));
    assert!(from_str.contains("Total: 2 snapshots"));

    // 3. Filter --to snap2: snap1 and snap2
    let to_out = env
        .command()
        .args(["list", "--remote", &env.remote_str(), "--to", "snap2"])
        .assert()
        .success();
    let to_str = String::from_utf8(to_out.get_output().stdout.clone()).unwrap();
    assert!(to_str.contains("snap1"));
    assert!(to_str.contains("snap2"));
    assert!(!to_str.contains("snap3"));
    assert!(to_str.contains("Total: 2 snapshots"));

    // 4. Filter --from snap2 --to snap2: only snap2
    let exact_out = env
        .command()
        .args([
            "list",
            "--remote",
            &env.remote_str(),
            "--from",
            "snap2",
            "--to",
            "snap2",
        ])
        .assert()
        .success();
    let exact_str = String::from_utf8(exact_out.get_output().stdout.clone()).unwrap();
    assert!(!exact_str.contains("snap1"));
    assert!(exact_str.contains("snap2"));
    assert!(!exact_str.contains("snap3"));
    assert!(exact_str.contains("Total: 1 snapshots"));

    // 5. Test visible aliases --start and --end
    let alias_out = env
        .command()
        .args([
            "list",
            "--remote",
            &env.remote_str(),
            "--start",
            "snap1",
            "--end",
            "snap2",
        ])
        .assert()
        .success();
    let alias_str = String::from_utf8(alias_out.get_output().stdout.clone()).unwrap();
    assert!(alias_str.contains("snap1"));
    assert!(alias_str.contains("snap2"));
    assert!(!alias_str.contains("snap3"));
    assert!(alias_str.contains("Total: 2 snapshots"));

    // 6. Inverted bounds: --from snap3 --to snap1 -> empty
    let inv_out = env
        .command()
        .args([
            "list",
            "--remote",
            &env.remote_str(),
            "--from",
            "snap3",
            "--to",
            "snap1",
        ])
        .assert()
        .success();
    let inv_str = String::from_utf8(inv_out.get_output().stdout.clone()).unwrap();
    assert!(inv_str.contains("(no snapshots yet)"));
    assert!(inv_str.contains("Total: 0 snapshots"));

    // 7. JSON output with filters
    let json_out = env
        .command()
        .args([
            "list",
            "--remote",
            &env.remote_str(),
            "--from",
            "snap2",
            "--to",
            "snap2",
            "--json",
        ])
        .assert()
        .success();
    let json_str = String::from_utf8(json_out.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "snap2");

    // 8. Invalid bound returns error
    env.command()
        .args([
            "list",
            "--remote",
            &env.remote_str(),
            "--from",
            "non-existent-snap",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid timestamp or snapshot name"));
}

#[test]
fn test_list_catalog_fast_path_and_self_healing() {
    let env = TestEnv::new();

    // Initialize repo
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

    // Verify .meta/snapshots.jsonl was created and contains both snapshots
    let catalog_path = env.remote_path.join(".meta/snapshots.jsonl");
    assert!(catalog_path.exists(), "snapshots.jsonl catalog should exist after sync");
    let content = std::fs::read_to_string(&catalog_path).unwrap();
    assert!(content.contains("snap1"));
    assert!(content.contains("snap2"));

    // List reads quickly from catalog
    env.command()
        .args(["list", "--remote", &env.remote_str()])
        .assert()
        .success()
        .stdout(predicate::str::contains("snap1"))
        .stdout(predicate::str::contains("snap2"))
        .stdout(predicate::str::contains("Total: 2 snapshots"));

    // Simulate self-healing: remove catalog file
    std::fs::remove_file(&catalog_path).unwrap();
    assert!(!catalog_path.exists());

    // Run list: it should rebuild the catalog from manifests and succeed
    env.command()
        .args(["list", "--remote", &env.remote_str()])
        .assert()
        .success()
        .stdout(predicate::str::contains("snap1"))
        .stdout(predicate::str::contains("snap2"))
        .stdout(predicate::str::contains("Total: 2 snapshots"));

    // Verify catalog was self-healed
    assert!(catalog_path.exists(), "snapshots.jsonl catalog should be recreated by self-healing");
    let healed_content = std::fs::read_to_string(&catalog_path).unwrap();
    assert!(healed_content.contains("snap1"));
    assert!(healed_content.contains("snap2"));
}


