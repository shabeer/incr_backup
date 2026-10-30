//! # Write-Ahead Journal Crash Recovery Tests
//!
//! Tests crash recovery and interruption handling using the write-ahead journal (`.meta/journal.json`).
//!
//! ## Covered Scenarios:
//! 1. Process killed abruptly during `rclone_sync` (SIGKILL / crash with lock held)
//! 2. Process killed after `rclone_sync` but before/during `generate_manifest`
//! 3. Process killed during `upload_manifest`
//! 4. Network disconnection / transport failure during sync, followed by network restore and resume
//! 5. Stale lock auto-break: resuming an interrupted journal breaks the dead process's lock without requiring `--force`
//! 6. Dry-run resume: `sync --dry-run` previews resume without mutating the journal
//! 7. Corrupt journal detection: malformed or truncated journal returns `CorruptJournal` error
//! 8. Live Google Drive journal recovery: full end-to-end recovery on real Google Drive remote

mod common;

use common::{LiveGdriveTestEnv, TestEnv};
use predicates::prelude::*;
use serde_json::json;
use std::fs;

// ─── 1. Process Killed During rclone_sync ────────────────────────────────────

#[test]
fn test_journal_recovery_process_killed_during_sync() {
    let env = TestEnv::new();
    env.write_source_file("file1.txt", b"Content 1");
    env.write_source_file("file2.txt", b"Content 2");

    // Init
    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // Stage an interrupted state on remote:
    // Process acquired lock, started rclone_sync, then was killed (SIGKILL)
    let meta_dir = env.remote_path.join(".meta");
    fs::create_dir_all(&meta_dir).unwrap();

    let lock_json = json!({
        "holder": "crashed-worker-host.local",
        "pid": 99991,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "timeout_minutes": 600000
    });
    fs::write(meta_dir.join("lock"), serde_json::to_string(&lock_json).unwrap()).unwrap();

    let snap_name = "2026-10-30T10-30Z";
    let backup_dir = format!("diffs/{snap_name}");

    let journal_json = json!({
        "operation": "sync",
        "snapshot_name": snap_name,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "backup_dir": backup_dir,
        "steps": [
            { "name": "acquire_lock", "status": "Done" },
            { "name": "rclone_sync", "status": "Running" },
            { "name": "generate_manifest", "status": "Pending" },
            { "name": "upload_manifest", "status": "Pending" },
            { "name": "cleanup_journal", "status": "Pending" },
            { "name": "release_lock", "status": "Pending" }
        ]
    });
    fs::write(meta_dir.join("journal.json"), serde_json::to_string_pretty(&journal_json).unwrap()).unwrap();

    // Now run sync again — it should detect the journal, break stale lock, resume rclone_sync,
    // generate manifest, upload manifest, and cleanup journal & lock.
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success()
        .stderr(predicate::str::contains("Found incomplete journal"))
        .stderr(predicate::str::contains("Resuming interrupted operation"));

    // Verify recovery succeeded:
    // 1. Files are in remote current/
    assert!(env.remote_path.join("current/file1.txt").exists());
    assert!(env.remote_path.join("current/file2.txt").exists());

    // 2. Manifest was generated and uploaded
    let manifest_path = meta_dir.join(format!("manifests/{snap_name}.json"));
    assert!(manifest_path.exists(), "Snapshot manifest must exist");
    assert!(meta_dir.join("manifests/current.json").exists(), "Current manifest must exist");

    // 3. Journal was deleted
    assert!(!meta_dir.join("journal.json").exists(), "Journal must be deleted on successful recovery");

    // 4. Lock was released
    assert!(!meta_dir.join("lock").exists(), "Lock must be released on successful recovery");
}

// ─── 2. Interrupted Before / During Manifest Generation ──────────────────────

#[test]
fn test_journal_recovery_interrupted_before_manifest_generation() {
    let env = TestEnv::new();
    env.write_source_file("report.pdf", b"%PDF-1.4 sample pdf content");

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // Pre-sync the file into remote current/ so rclone_sync was genuinely Done
    let remote_current = env.remote_path.join("current");
    fs::create_dir_all(&remote_current).unwrap();
    fs::write(remote_current.join("report.pdf"), b"%PDF-1.4 sample pdf content").unwrap();

    let meta_dir = env.remote_path.join(".meta");
    let snap_name = "2026-10-30T11-00Z";

    // Stage journal where rclone_sync is DONE, but generate_manifest is Running
    let journal_json = json!({
        "operation": "sync",
        "snapshot_name": snap_name,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "backup_dir": format!("diffs/{snap_name}"),
        "steps": [
            { "name": "acquire_lock", "status": "Done" },
            { "name": "rclone_sync", "status": "Done" },
            { "name": "generate_manifest", "status": "Running" },
            { "name": "upload_manifest", "status": "Pending" },
            { "name": "cleanup_journal", "status": "Pending" },
            { "name": "release_lock", "status": "Pending" }
        ]
    });
    fs::write(meta_dir.join("journal.json"), serde_json::to_string_pretty(&journal_json).unwrap()).unwrap();

    // Run sync to resume
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success()
        .stderr(predicate::str::contains("Resuming interrupted operation"));

    // Manifest should be generated and uploaded
    let manifest_path = meta_dir.join(format!("manifests/{snap_name}.json"));
    assert!(manifest_path.exists());
    let manifest_content = fs::read_to_string(&manifest_path).unwrap();
    assert!(manifest_content.contains("report.pdf"));

    // Journal and lock cleaned up
    assert!(!meta_dir.join("journal.json").exists());
    assert!(!meta_dir.join("lock").exists());
}

// ─── 3. Interrupted During Manifest Upload ──────────────────────────────────

#[test]
fn test_journal_recovery_interrupted_during_manifest_upload() {
    let env = TestEnv::new();
    env.write_source_file("archive.tar", b"dummy tar bytes 1234567890");

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    let remote_current = env.remote_path.join("current");
    fs::create_dir_all(&remote_current).unwrap();
    fs::write(remote_current.join("archive.tar"), b"dummy tar bytes 1234567890").unwrap();

    let meta_dir = env.remote_path.join(".meta");
    let snap_name = "2026-10-30T11-30Z";

    // Stage journal where rclone_sync and generate_manifest are Done, but upload_manifest was Running
    let journal_json = json!({
        "operation": "sync",
        "snapshot_name": snap_name,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "backup_dir": format!("diffs/{snap_name}"),
        "steps": [
            { "name": "acquire_lock", "status": "Done" },
            { "name": "rclone_sync", "status": "Done" },
            { "name": "generate_manifest", "status": "Done" },
            { "name": "upload_manifest", "status": "Running" },
            { "name": "cleanup_journal", "status": "Pending" },
            { "name": "release_lock", "status": "Pending" }
        ]
    });
    fs::write(meta_dir.join("journal.json"), serde_json::to_string_pretty(&journal_json).unwrap()).unwrap();

    // Run sync to resume
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // Manifests should exist
    assert!(meta_dir.join(format!("manifests/{snap_name}.json")).exists());
    assert!(meta_dir.join("manifests/current.json").exists());

    // Journal and lock cleaned up
    assert!(!meta_dir.join("journal.json").exists());
    assert!(!meta_dir.join("lock").exists());
}

// ─── 4. Network Disconnection & Reconnection Recovery ───────────────────────

#[test]
fn test_journal_recovery_network_disconnect() {
    let env = TestEnv::new();
    env.write_source_file("data_sync.bin", b"Critical transactional data");

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    let meta_dir = env.remote_path.join(".meta");
    let snap_name = "2026-10-30T12-00Z";

    // Simulate what happens when network drops during rclone transfer:
    // A journal was written, rclone started transferring, network connection was severed.
    // The journal remains on the remote at rclone_sync Running / Failed.
    let journal_json = json!({
        "operation": "sync",
        "snapshot_name": snap_name,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "backup_dir": format!("diffs/{snap_name}"),
        "steps": [
            { "name": "acquire_lock", "status": "Done" },
            { "name": "rclone_sync", "status": { "Failed": "Connection timed out (os error 60)" } },
            { "name": "generate_manifest", "status": "Pending" },
            { "name": "upload_manifest", "status": "Pending" },
            { "name": "cleanup_journal", "status": "Pending" },
            { "name": "release_lock", "status": "Pending" }
        ]
    });
    fs::write(meta_dir.join("journal.json"), serde_json::to_string_pretty(&journal_json).unwrap()).unwrap();

    // Network is restored -> next sync execution resumes from the failed rclone_sync step
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success()
        .stderr(predicate::str::contains("Resuming interrupted operation"));

    // Verify complete synchronization
    assert!(env.remote_path.join("current/data_sync.bin").exists());
    assert!(meta_dir.join(format!("manifests/{snap_name}.json")).exists());
    assert!(!meta_dir.join("journal.json").exists());
}

// ─── 5. Stale Lock Auto-Break during Resume ──────────────────────────────────

#[test]
fn test_journal_recovery_auto_breaks_stale_lock() {
    let env = TestEnv::new();
    env.write_source_file("file.txt", b"hello world");

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    let meta_dir = env.remote_path.join(".meta");

    // Stale lock left behind by killed process
    let lock_json = json!({
        "holder": "dead-process-host",
        "pid": 77777,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "timeout_minutes": 600000
    });
    fs::write(meta_dir.join("lock"), serde_json::to_string(&lock_json).unwrap()).unwrap();

    // Incomplete journal from that dead process
    let snap_name = "2026-10-30T13-00Z";
    let journal_json = json!({
        "operation": "sync",
        "snapshot_name": snap_name,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "backup_dir": format!("diffs/{snap_name}"),
        "steps": [
            { "name": "acquire_lock", "status": "Done" },
            { "name": "rclone_sync", "status": "Running" },
            { "name": "generate_manifest", "status": "Pending" },
            { "name": "upload_manifest", "status": "Pending" },
            { "name": "cleanup_journal", "status": "Pending" },
            { "name": "release_lock", "status": "Pending" }
        ]
    });
    fs::write(meta_dir.join("journal.json"), serde_json::to_string_pretty(&journal_json).unwrap()).unwrap();

    // Notice we do NOT pass --force! The journal recovery mechanism automatically
    // forces lock acquisition to recover from the crash!
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success()
        .stderr(predicate::str::contains("Resuming interrupted operation"));

    assert!(env.remote_path.join("current/file.txt").exists());
    assert!(!meta_dir.join("journal.json").exists());
    assert!(!meta_dir.join("lock").exists());
}

// ─── 6. Dry Run Resume ──────────────────────────────────────────────────────

#[test]
fn test_journal_recovery_dry_run() {
    let env = TestEnv::new();
    env.write_source_file("dry_file.txt", b"dry content");

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    let meta_dir = env.remote_path.join(".meta");
    let snap_name = "2026-10-30T14-00Z";

    let journal_json = json!({
        "operation": "sync",
        "snapshot_name": snap_name,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "backup_dir": format!("diffs/{snap_name}"),
        "steps": [
            { "name": "acquire_lock", "status": "Done" },
            { "name": "rclone_sync", "status": "Running" },
            { "name": "generate_manifest", "status": "Pending" },
            { "name": "upload_manifest", "status": "Pending" },
            { "name": "cleanup_journal", "status": "Pending" },
            { "name": "release_lock", "status": "Pending" }
        ]
    });
    fs::write(meta_dir.join("journal.json"), serde_json::to_string_pretty(&journal_json).unwrap()).unwrap();

    // Run sync in dry-run mode
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[DRY-RUN] Would sync"));

    // Remote journal must NOT be deleted or mutated
    assert!(
        meta_dir.join("journal.json").exists(),
        "Dry run must preserve the journal on remote"
    );
}

// ─── 7. Corrupt Journal Detection ───────────────────────────────────────────

#[test]
fn test_journal_recovery_corrupt_journal() {
    let env = TestEnv::new();
    env.write_source_file("file.txt", b"content");

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    let meta_dir = env.remote_path.join(".meta");

    // Write malformed/corrupt JSON into journal
    fs::write(meta_dir.join("journal.json"), b"{\"incomplete_json\": true, ").unwrap();

    // Sync should fail with CorruptJournal error
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("corrupt journal"));
}

// ─── 8. Live Google Drive Crash Recovery ────────────────────────────────────

#[test]
fn test_live_journal_recovery_on_google_drive() {
    let Some(env) = LiveGdriveTestEnv::new("gdrive_crash_rec") else {
        eprintln!("Skipping test_live_journal_recovery_on_google_drive: Google Drive remote unreachable");
        return;
    };

    // 1. Initialize on real Google Drive
    env.command().arg("init").assert().success();

    env.write_source_file("critical_db.sqlite", b"SQLITE_FORMAT_HEADER_DATA_2026");

    // 2. Stage interrupted crash state directly on Google Drive remote
    let snap_name = "2026-10-30T15-00+0530";
    let backup_dir = format!("diffs/{snap_name}");

    let journal_json = json!({
        "operation": "sync",
        "snapshot_name": snap_name,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "backup_dir": backup_dir,
        "steps": [
            { "name": "acquire_lock", "status": "Done" },
            { "name": "rclone_sync", "status": "Running" },
            { "name": "generate_manifest", "status": "Pending" },
            { "name": "upload_manifest", "status": "Pending" },
            { "name": "cleanup_journal", "status": "Pending" },
            { "name": "release_lock", "status": "Pending" }
        ]
    });

    // Upload staged journal to Google Drive via rclone rcat
    let journal_remote_path = format!("{}/.meta/journal.json", env.remote_str());
    let mut cmd = std::process::Command::new(&env.rclone_path);
    cmd.args(["rcat", &journal_remote_path]);
    for flag in &env.rclone_flags {
        cmd.arg(flag);
    }
    cmd.stdin(std::process::Stdio::piped());
    if let Ok(mut child) = cmd.spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(journal_json.to_string().as_bytes());
        }
        let _ = child.wait();
    }

    // Also upload a stale lock file directly on Google Drive
    let lock_json = json!({
        "holder": "killed-process-worker",
        "pid": 55555,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "timeout_minutes": 600000
    });
    let lock_remote_path = format!("{}/.meta/lock", env.remote_str());
    let mut cmd2 = std::process::Command::new(&env.rclone_path);
    cmd2.args(["rcat", &lock_remote_path]);
    for flag in &env.rclone_flags {
        cmd2.arg(flag);
    }
    cmd2.stdin(std::process::Stdio::piped());
    if let Ok(mut child) = cmd2.spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(lock_json.to_string().as_bytes());
        }
        let _ = child.wait();
    }

    // 3. Run sync against Google Drive remote
    // It must detect the journal on Google Drive, break the stale lock, resume sync,
    // upload manifests, and clean up journal and lock on Google Drive!
    env.command()
        .arg("sync")
        .assert()
        .success()
        .stderr(predicate::str::contains("Resuming interrupted operation"));

    // 4. Verify Google Drive state:
    // - critical_db.sqlite is in current/
    let cat_db = env.rclone_exec(&[
        "cat",
        &format!("{}/current/critical_db.sqlite", env.remote_str()),
    ]);
    assert!(cat_db.status.success(), "File must exist in current/ on Google Drive");

    // - Manifest exists on Google Drive
    let cat_manifest = env.rclone_exec(&[
        "cat",
        &format!("{}/.meta/manifests/{}.json", env.remote_str(), snap_name),
    ]);
    assert!(cat_manifest.status.success(), "Manifest must exist on Google Drive");

    // - Journal is DELETED from Google Drive
    let cat_journal = env.rclone_exec(&["cat", &journal_remote_path]);
    assert!(
        !cat_journal.status.success(),
        "Journal must be cleaned up from Google Drive"
    );

    // - Lock is RELEASED from Google Drive
    let cat_lock = env.rclone_exec(&["cat", &lock_remote_path]);
    assert!(
        !cat_lock.status.success(),
        "Lock must be released from Google Drive"
    );
}
