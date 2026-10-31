mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn test_lock_prevents_concurrent_runs() {
    let env = TestEnv::new();
    env.write_source_file("file.txt", b"content");

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // Manually create a lock file simulating another process
    let lock_json = serde_json::json!({
        "holder": "another-machine.local",
        "pid": 99999,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "timeout_minutes": 600000
    });
    let lock_path = env.remote_path.join(".meta/lock");
    std::fs::write(&lock_path, serde_json::to_string(&lock_json).unwrap()).unwrap();

    // Sync should fail because of lock
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("backup locked by 'another-machine.local'"));

    // Sync with --force should succeed
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--force",
        ])
        .assert()
        .success();
}

#[test]
fn test_lock_error_respects_timezone() {
    let env = TestEnv::new();
    env.write_source_file("file.txt", b"content");

    let lock_json = serde_json::json!({
        "holder": "shabeer-mac.local",
        "pid": 99999,
        "started_at": "2026-10-29T06:30:05.775308Z",
        "timeout_minutes": 600000
    });
    let meta_dir = env.remote_path.join(".meta");
    std::fs::create_dir_all(&meta_dir).unwrap();
    std::fs::write(meta_dir.join("lock"), serde_json::to_string(&lock_json).unwrap()).unwrap();

    // With UTC timezone
    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("2026-10-29T06:30:05.775308+00:00"));

    // With local timezone
    let dt = chrono::DateTime::parse_from_rfc3339("2026-10-29T06:30:05.775308Z").unwrap();
    let local_expected = dt.with_timezone(&chrono::Local).to_rfc3339();

    env.command()
        .args([
            "sync",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--timezone",
            "local",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(local_expected));
}
