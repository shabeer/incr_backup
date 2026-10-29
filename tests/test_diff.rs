mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn test_diff_command() {
    let env = TestEnv::new();
    env.write_source_file("unchanged.txt", b"never changes");
    env.write_source_file("modified.txt", b"Version 1");
    env.write_source_file("to_delete.txt", b"will be deleted");

    // Init & Sync 1 (creates snap1)
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

    // Modify files for Sync 2
    env.write_source_file("modified.txt", b"Version 2 - updated content");
    env.delete_source_file("to_delete.txt");
    env.write_source_file("added.txt", b"brand new file");

    // Sync 2 (creates snap2)
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

    // 1. Human readable diff between snap2 and snap1 (default UTC timezone with Z)
    env.command()
        .args([
            "diff",
            "snap2",
            "snap1",
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("added.txt"))
        .stdout(predicate::str::contains("modified.txt"))
        .stdout(predicate::str::contains("to_delete.txt"))
        .stdout(predicate::str::contains("Comparing: snap2 ("))
        .stdout(predicate::str::contains("Z) ↔ snap1 ("));

    // 1b. Human readable diff with --timezone local (shows local offset)
    env.command()
        .args([
            "diff",
            "snap2",
            "snap1",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "local",
        ])
        .assert()
        .success()
        .stdout(predicate::str::is_match(r"Comparing: snap2 \(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}[+-]\d{2}:\d{2}\) ↔ snap1 \(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}[+-]\d{2}:\d{2}\)").unwrap());

    // 2. Diff with --stat
    env.command()
        .args([
            "diff",
            "snap2",
            "snap1",
            "--remote",
            &env.remote_str(),
            "--stat",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 added, 1 deleted, 1 modified"));

    // 3. Diff with --json
    env.command()
        .args([
            "diff",
            "snap2",
            "snap1",
            "--remote",
            &env.remote_str(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"added\":"))
        .stdout(predicate::str::contains("\"deleted\":"))
        .stdout(predicate::str::contains("\"modified\":"));
}
