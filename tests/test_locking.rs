mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn test_lock_prevents_concurrent_sync() {
    let env = TestEnv::new();
    env.write_source_file("file.txt", b"content");

    // Init
    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // Create an artificial active lock file
    let lock_json = serde_json::json!({
        "holder": "another-machine.local",
        "pid": 99999,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "timeout_minutes": 60
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
            "--level",
            "hourly",
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
            "--level",
            "hourly",
            "--force",
        ])
        .assert()
        .success();
}

