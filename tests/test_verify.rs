mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn test_verify_command() {
    let env = TestEnv::new();
    env.write_source_file("file1.txt", b"Content A");
    env.write_source_file("file2.txt", b"Content B");

    // Init & sync
    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--level", "hourly"])
        .assert()
        .success();

    // 1. Run basic verify
    env.command()
        .args([
            "verify",
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("All checks passed. ✓"))
        .stdout(predicate::str::contains("Config valid"))
        .stdout(predicate::str::contains("Manifest: current"));

    // 2. Run verify with --space
    env.command()
        .args([
            "verify",
            "--remote",
            &env.remote_str(),
            "--space",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Space usage:"))
        .stdout(predicate::str::contains("current/"))
        .stdout(predicate::str::contains("Total:"));

    // 3. Run verify with --check-hashes
    env.command()
        .args([
            "verify",
            "--remote",
            &env.remote_str(),
            "--check-hashes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Remote hashes match manifest"));
}

