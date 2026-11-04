mod common;

use common::TestEnv;
use predicates::prelude::*;
use std::fs;

#[test]
fn test_compact_cli_timezone_validation() {
    let env = TestEnv::new();

    // 1. With UTC timezone configured, passing non-UTC offset must fail with exact error
    env.command()
        .args([
            "compact",
            "--from",
            "2026-10-28T00:00:00+05:30",
            "--to",
            "2026-10-28T23:59:59+05:30",
            "--name",
            "daily-2026-10-28",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Timezone offset in --from (+05:30) does not match configured backup timezone (+00:00)",
        ));

    // 2. With Local timezone configured, passing UTC Z must fail with exact error
    let local_offset = format!("{}", chrono::Local::now().offset());
    env.command()
        .args([
            "compact",
            "--from",
            "2026-10-28T00:00:00Z",
            "--to",
            "2026-10-28T23:59:59Z",
            "--name",
            "daily-2026-10-28",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "local",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(format!(
            "Timezone offset in --from (+00:00) does not match configured backup timezone ({})",
            local_offset
        )));
}

#[test]
fn test_compact_no_snapshots_in_range() {
    let env = TestEnv::new();

    env.command()
        .args([
            "init",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .success();

    // Spec Section 9.2:
    // Gracefully exits with error: "No snapshots found in range [from, to]". Exit code 1. Zero changes made.
    env.command()
        .args([
            "compact",
            "--from",
            "2025-01-01T00:00:00Z",
            "--to",
            "2025-01-02T00:00:00Z",
            "--name",
            "empty-range",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "No snapshots found in range [2025-01-01T00:00:00Z, 2025-01-02T00:00:00Z]",
        ));
}

#[test]
fn test_compact_single_snapshot() {
    let env = TestEnv::new();
    env.write_source_file("hello.txt", b"Hello World");

    env.command()
        .args([
            "init",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .success();

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
        .success();

    // Now compact across full time range including today
    let from = "2020-01-01T00:00:00Z";
    let to = "2030-01-01T00:00:00Z";

    env.command()
        .args([
            "compact",
            "--from",
            from,
            "--to",
            to,
            "--name",
            "renamed-single",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .success();

    // Check that manifest renamed-single.json exists
    assert!(env.remote_path.join(".meta/manifests/renamed-single.json").exists());

    // Restore renamed-single to verify content
    let restore_single = env.temp_dir.path().join("restore_single");
    env.command()
        .args([
            "restore",
            "renamed-single",
            "--to",
            &restore_single.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();
    assert_eq!(fs::read(restore_single.join("hello.txt")).unwrap(), b"Hello World");
}

#[test]
fn test_compact_concrete_walkthrough_and_point_in_time_restore() {
    let env = TestEnv::new();

    // Setup the exact scenario from design_snapshot_consolidation.md Sections 2, 3, and 4:
    // Files: report.docx, budget.xlsx, logo.png
    // Versions:
    //   Day 1, 23:00 (day-2026-10-27): report=VA, budget=V1, logo=VL
    //   Day 2, 10:00 (snap-10:00): budget=V2, report=VA, logo=VL (diff snap-10:00 has budget V1)
    //   Day 2, 14:00 (snap-14:00): report=VB, budget=V2, logo=VL (diff snap-14:00 has report VA)
    //   Day 2, 20:00 (snap-20:00): report=VC, budget=V2, logo=VL (diff snap-20:00 has report VB)
    //   Day 3, 12:00 (snap-12:00): report=VD, budget=V2, logo=VL (diff snap-12:00 has report VC)
    //   Current: report=VD, budget=V2, logo=VL

    let v_a = b"Version_A_content";
    let v_b = b"Version_B_content";
    let v_c = b"Version_C_content";
    let v_d = b"Version_D_content";
    let v_1 = b"Version_1_content";
    let v_2 = b"Version_2_content";
    let v_l = b"Version_Logo_content";

    fn compute_md5(b: &[u8]) -> String {
        use md5::Digest;
        format!("{:x}", md5::Md5::digest(b))
    }

    let md5_a = compute_md5(v_a);
    let md5_b = compute_md5(v_b);
    let md5_c = compute_md5(v_c);
    let md5_d = compute_md5(v_d);
    let md5_1 = compute_md5(v_1);
    let md5_2 = compute_md5(v_2);
    let md5_l = compute_md5(v_l);

    let remote = &env.remote_path;
    let manifests_dir = remote.join(".meta/manifests");
    let diffs_dir = remote.join("diffs");
    let current_dir = remote.join("current");

    fs::create_dir_all(&manifests_dir).unwrap();
    fs::create_dir_all(&diffs_dir).unwrap();
    fs::create_dir_all(&current_dir).unwrap();

    // Helper to create manifest
    let write_manifest = |name: &str, files: Vec<(&str, u64, &str)>, iso_time: &str| {
        let file_entries: Vec<serde_json::Value> = files
            .into_iter()
            .map(|(p, sz, hash)| {
                serde_json::json!({
                    "path": p,
                    "size": sz,
                    "md5": hash,
                    "mtime": iso_time,
                })
            })
            .collect();

        let json = serde_json::json!({
            "snapshot_name": name,
            "created_at": iso_time,
            "files": file_entries,
            "total_size": 1000,
            "file_count": 3
        });
        fs::write(
            manifests_dir.join(format!("{name}.json")),
            serde_json::to_string_pretty(&json).unwrap(),
        )
        .unwrap();
    };

    // 1. Current files on remote
    fs::write(current_dir.join("report.docx"), v_d).unwrap();
    fs::write(current_dir.join("budget.xlsx"), v_2).unwrap();
    fs::write(current_dir.join("logo.png"), v_l).unwrap();

    // 2. Day 1: day-2026-10-27 at 23:00 UTC
    write_manifest(
        "day-2026-10-27",
        vec![
            ("report.docx", v_a.len() as u64, &md5_a),
            ("budget.xlsx", v_1.len() as u64, &md5_1),
            ("logo.png", v_l.len() as u64, &md5_l),
        ],
        "2026-10-27T23:00:00Z",
    );

    // 3. Day 2: snap-10:00 (diff has budget.xlsx V1)
    let diff_10 = diffs_dir.join("snap-10:00");
    fs::create_dir_all(&diff_10).unwrap();
    fs::write(diff_10.join("budget.xlsx"), v_1).unwrap();
    write_manifest(
        "snap-10:00",
        vec![
            ("report.docx", v_a.len() as u64, &md5_a),
            ("budget.xlsx", v_2.len() as u64, &md5_2),
            ("logo.png", v_l.len() as u64, &md5_l),
        ],
        "2026-10-28T10:00:00Z",
    );

    // 4. Day 2: snap-14:00 (diff has report.docx VA)
    let diff_14 = diffs_dir.join("snap-14:00");
    fs::create_dir_all(&diff_14).unwrap();
    fs::write(diff_14.join("report.docx"), v_a).unwrap();
    write_manifest(
        "snap-14:00",
        vec![
            ("report.docx", v_b.len() as u64, &md5_b),
            ("budget.xlsx", v_2.len() as u64, &md5_2),
            ("logo.png", v_l.len() as u64, &md5_l),
        ],
        "2026-10-28T14:00:00Z",
    );

    // 5. Day 2: snap-20:00 (diff has report.docx VB)
    let diff_20 = diffs_dir.join("snap-20:00");
    fs::create_dir_all(&diff_20).unwrap();
    fs::write(diff_20.join("report.docx"), v_b).unwrap();
    write_manifest(
        "snap-20:00",
        vec![
            ("report.docx", v_c.len() as u64, &md5_c),
            ("budget.xlsx", v_2.len() as u64, &md5_2),
            ("logo.png", v_l.len() as u64, &md5_l),
        ],
        "2026-10-28T20:00:00Z",
    );

    // 6. Day 3: snap-12:00 (diff has report.docx VC)
    let diff_12 = diffs_dir.join("snap-12:00");
    fs::create_dir_all(&diff_12).unwrap();
    fs::write(diff_12.join("report.docx"), v_c).unwrap();
    write_manifest(
        "snap-12:00",
        vec![
            ("report.docx", v_d.len() as u64, &md5_d),
            ("budget.xlsx", v_2.len() as u64, &md5_2),
            ("logo.png", v_l.len() as u64, &md5_l),
        ],
        "2026-10-29T12:00:00Z",
    );

    // Test Dry-run first with --summary
    env.command()
        .args([
            "compact",
            "--from",
            "2026-10-28T00:00:00Z",
            "--to",
            "2026-10-28T23:59:59Z",
            "--name",
            "daily-2026-10-28",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
            "--dry-run",
            "--summary",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Compaction Plan Summary:"))
        .stdout(predicate::str::contains("Snapshots in range:   3"))
        .stdout(predicate::str::contains("Diff files kept:      2"))
        .stdout(predicate::str::contains("Diff files discarded: 1"));

    // Dry-run must not modify files
    assert!(diff_10.exists());
    assert!(diff_14.exists());
    assert!(diff_20.exists());
    assert!(!diffs_dir.join("daily-2026-10-28").exists());

    // Test Dry-run with --full-list
    env.command()
        .args([
            "compact",
            "--from",
            "2026-10-28T00:00:00Z",
            "--to",
            "2026-10-28T23:59:59Z",
            "--name",
            "daily-2026-10-28",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
            "--dry-run",
            "--full-list",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Detailed Compaction Plan for 'daily-2026-10-28'"))
        .stdout(predicate::str::contains("+ budget.xlsx"))
        .stdout(predicate::str::contains("+ report.docx"))
        .stdout(predicate::str::contains("- report.docx"));

    // Execute real compaction
    env.command()
        .args([
            "compact",
            "--from",
            "2026-10-28T00:00:00Z",
            "--to",
            "2026-10-28T23:59:59Z",
            "--name",
            "daily-2026-10-28",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Compaction complete: daily-2026-10-28"));

    // Verify post-compaction directory layout:
    // - Created: diffs/daily-2026-10-28/ containing report.docx (VA) and budget.xlsx (V1)
    // - Created: .meta/manifests/daily-2026-10-28.json
    // - Purged: diffs/snap-10:00, diffs/snap-14:00, diffs/snap-20:00 and their manifests
    let compacted_diff = diffs_dir.join("daily-2026-10-28");
    assert!(compacted_diff.exists());
    assert_eq!(
        fs::read(compacted_diff.join("report.docx")).unwrap(),
        v_a,
        "Compacted diff must contain VA from snap-14:00"
    );
    assert_eq!(
        fs::read(compacted_diff.join("budget.xlsx")).unwrap(),
        v_1,
        "Compacted diff must contain V1 from snap-10:00"
    );

    // Intermediate diff VB must NOT exist anywhere in daily-2026-10-28
    assert!(!diff_10.exists());
    assert!(!diff_14.exists());
    assert!(!diff_20.exists());
    assert!(!manifests_dir.join("snap-10:00.json").exists());
    assert!(!manifests_dir.join("snap-14:00.json").exists());
    assert!(!manifests_dir.join("snap-20:00.json").exists());

    // ─── Verification of Scenario A: Restore daily-2026-10-28 ───────────────
    // Point in time: Day 2, 20:00 -> expected: report=VC, budget=V2, logo=VL
    let restore_day2 = env.temp_dir.path().join("restore_day2");
    env.command()
        .args([
            "restore",
            "daily-2026-10-28",
            "--to",
            &restore_day2.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();

    assert_eq!(
        fs::read(restore_day2.join("report.docx")).unwrap(),
        v_c,
        "Restoring Day 2 must yield VC"
    );
    assert_eq!(
        fs::read(restore_day2.join("budget.xlsx")).unwrap(),
        v_2,
        "Restoring Day 2 must yield V2"
    );
    assert_eq!(
        fs::read(restore_day2.join("logo.png")).unwrap(),
        v_l,
        "Restoring Day 2 must yield VL"
    );

    // ─── Verification of Scenario B: Restore day-2026-10-27 ─────────────────
    // Point in time: Day 1, 23:00 -> expected: report=VA, budget=V1, logo=VL
    let restore_day1 = env.temp_dir.path().join("restore_day1");
    env.command()
        .args([
            "restore",
            "day-2026-10-27",
            "--to",
            &restore_day1.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();

    assert_eq!(
        fs::read(restore_day1.join("report.docx")).unwrap(),
        v_a,
        "Restoring Day 1 must yield VA"
    );
    assert_eq!(
        fs::read(restore_day1.join("budget.xlsx")).unwrap(),
        v_1,
        "Restoring Day 1 must yield V1"
    );
    assert_eq!(
        fs::read(restore_day1.join("logo.png")).unwrap(),
        v_l,
        "Restoring Day 1 must yield VL"
    );

    // ─── Verification of Restore snap-12:00 (Day 3) ──────────────────────────
    let restore_day3 = env.temp_dir.path().join("restore_day3");
    env.command()
        .args([
            "restore",
            "snap-12:00",
            "--to",
            &restore_day3.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();

    assert_eq!(
        fs::read(restore_day3.join("report.docx")).unwrap(),
        v_d,
        "Restoring Day 3 must yield VD"
    );
    assert_eq!(
        fs::read(restore_day3.join("budget.xlsx")).unwrap(),
        v_2,
        "Restoring Day 3 must yield V2"
    );
    assert_eq!(
        fs::read(restore_day3.join("logo.png")).unwrap(),
        v_l,
        "Restoring Day 3 must yield VL"
    );
}

#[test]
fn test_compact_intra_range_created_and_deleted() {
    let env = TestEnv::new();

    // S_prev at 08:00 has only keep.txt
    let t_prev = "2026-10-28T08:00:00Z";
    let remote = &env.remote_path;
    let manifests_dir = remote.join(".meta/manifests");
    let diffs_dir = remote.join("diffs");
    let current_dir = remote.join("current");

    fs::create_dir_all(&manifests_dir).unwrap();
    fs::create_dir_all(&diffs_dir).unwrap();
    fs::create_dir_all(&current_dir).unwrap();

    fs::write(current_dir.join("keep.txt"), b"keep").unwrap();

    let write_manifest = |name: &str, files: Vec<(&str, &str)>, iso_time: &str| {
        let file_entries: Vec<serde_json::Value> = files
            .into_iter()
            .map(|(p, hash)| {
                serde_json::json!({
                    "path": p,
                    "size": 10,
                    "md5": hash,
                    "mtime": iso_time,
                })
            })
            .collect();

        let json = serde_json::json!({
            "snapshot_name": name,
            "created_at": iso_time,
            "files": file_entries,
            "total_size": 100,
            "file_count": file_entries.len()
        });
        fs::write(
            manifests_dir.join(format!("{name}.json")),
            serde_json::to_string(&json).unwrap(),
        )
        .unwrap();
    };

    write_manifest("s-08:00", vec![("keep.txt", "h_keep")], t_prev);

    // Intra-range snapshot 1 (10:00): created temp.txt
    let diff_10 = diffs_dir.join("s-10:00");
    fs::create_dir_all(&diff_10).unwrap();
    write_manifest(
        "s-10:00",
        vec![("keep.txt", "h_keep"), ("temp.txt", "h_temp")],
        "2026-10-28T10:00:00Z",
    );

    // Intra-range snapshot 2 (14:00): temp.txt deleted, moved to diff
    let diff_14 = diffs_dir.join("s-14:00");
    fs::create_dir_all(&diff_14).unwrap();
    fs::write(diff_14.join("temp.txt"), b"temp content").unwrap();
    write_manifest("s-14:00", vec![("keep.txt", "h_keep")], "2026-10-28T14:00:00Z");

    // Compact [09:00 - 15:00]
    env.command()
        .args([
            "compact",
            "--from",
            "2026-10-28T09:00:00Z",
            "--to",
            "2026-10-28T15:00:00Z",
            "--name",
            "daily-intra-test",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .success();

    // temp.txt must NOT exist in consolidated diff
    let compacted_diff = diffs_dir.join("daily-intra-test");
    if compacted_diff.exists() {
        assert!(!compacted_diff.join("temp.txt").exists());
    }

    // Point-in-time restore of daily-intra-test (anchor)
    let restore_anchor = env.temp_dir.path().join("restore_anchor");
    env.command()
        .args([
            "restore",
            "daily-intra-test",
            "--to",
            &restore_anchor.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();
    assert_eq!(fs::read(restore_anchor.join("keep.txt")).unwrap(), b"keep");
    assert!(!restore_anchor.join("temp.txt").exists());

    // Point-in-time restore of s-08:00 (S_prev)
    let restore_prev = env.temp_dir.path().join("restore_prev");
    env.command()
        .args([
            "restore",
            "s-08:00",
            "--to",
            &restore_prev.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();
    assert_eq!(fs::read(restore_prev.join("keep.txt")).unwrap(), b"keep");
    assert!(!restore_prev.join("temp.txt").exists());
}

#[test]
fn test_compact_uses_snapshot_jsonl() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str(), "--timezone", "utc"])
        .assert()
        .success();

    env.write_source_file("doc.txt", b"version 1");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--timezone", "utc", "--name", "snap1"])
        .assert()
        .success();

    env.write_source_file("doc.txt", b"version 2");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--timezone", "utc", "--name", "snap2"])
        .assert()
        .success();

    env.write_source_file("doc.txt", b"version 3");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--timezone", "utc", "--name", "snap3"])
        .assert()
        .success();

    // Rename snapshots.jsonl to snapshot.jsonl (singular fallback)
    let catalog_path = env.remote_path.join(".meta/snapshots.jsonl");
    let alt_catalog_path = env.remote_path.join(".meta/snapshot.jsonl");
    if catalog_path.exists() {
        fs::rename(&catalog_path, &alt_catalog_path).unwrap();
    }
    assert!(!catalog_path.exists(), "snapshots.jsonl must not exist");
    assert!(alt_catalog_path.exists(), "snapshot.jsonl must exist");

    // Run compact across range covering snap1 and snap2
    env.command()
        .args([
            "compact",
            "--from",
            "2020-01-01T00:00:00Z",
            "--to",
            "2030-01-01T00:00:00Z",
            "--name",
            "daily-all",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Compaction complete: daily-all"));

    // Verify snapshot.jsonl is kept up to date
    assert!(alt_catalog_path.exists(), "snapshot.jsonl must still exist");
    let alt_content = fs::read_to_string(&alt_catalog_path).unwrap();
    assert!(alt_content.contains("daily-all"), "daily-all must be in snapshot.jsonl");
    assert!(!alt_content.contains("snap1"), "snap1 must be removed from snapshot.jsonl");
    assert!(!alt_content.contains("snap2"), "snap2 must be removed from snapshot.jsonl");
    assert!(!alt_content.contains("snap3"), "snap3 must be removed from snapshot.jsonl");

    // Point-in-time restore of consolidated snapshot using snapshot.jsonl
    let restore_cat = env.temp_dir.path().join("restore_cat");
    env.command()
        .args([
            "restore",
            "daily-all",
            "--to",
            &restore_cat.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();
    assert_eq!(fs::read(restore_cat.join("doc.txt")).unwrap(), b"version 3");
}

#[test]
fn test_compact_validates_timestamps_before_lock() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str(), "--timezone", "utc"])
        .assert()
        .success();

    // Create a foreign lock on the remote to ensure any lock attempt will fail
    let lock_json = serde_json::json!({
        "holder": "foreign-host.local",
        "pid": 12345,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "timeout_minutes": 600000
    });
    let meta_dir = env.remote_path.join(".meta");
    fs::create_dir_all(&meta_dir).unwrap();
    fs::write(meta_dir.join("lock"), serde_json::to_string(&lock_json).unwrap()).unwrap();

    // 1. Invalid timestamp format must fail fast BEFORE acquiring lock
    env.command()
        .args([
            "compact",
            "--from",
            "not-a-valid-timestamp",
            "--to",
            "2026-10-28T23:59:59Z",
            "--name",
            "daily-test",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid timestamp in --from"))
        .stderr(predicate::str::contains("backup locked by 'foreign-host.local'").not());

    // 2. Timezone mismatch must fail fast BEFORE acquiring lock
    env.command()
        .args([
            "compact",
            "--from",
            "2026-10-28T00:00:00+05:30",
            "--to",
            "2026-10-28T23:59:59+05:30",
            "--name",
            "daily-test",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Timezone offset in --from (+05:30) does not match configured backup timezone (+00:00)"))
        .stderr(predicate::str::contains("backup locked by 'foreign-host.local'").not());

    // 3. from > to must fail fast BEFORE acquiring lock
    env.command()
        .args([
            "compact",
            "--from",
            "2026-10-29T00:00:00Z",
            "--to",
            "2026-10-28T00:00:00Z",
            "--name",
            "daily-test",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--from timestamp must be before or equal to --to timestamp"))
        .stderr(predicate::str::contains("backup locked by 'foreign-host.local'").not());

    // 4. Conversely, with valid timestamps, it proceeds to lock and fails on the lock
    env.command()
        .args([
            "compact",
            "--from",
            "2026-10-28T00:00:00Z",
            "--to",
            "2026-10-28T23:59:59Z",
            "--name",
            "daily-test",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("backup locked by 'foreign-host.local'"));
}

#[test]
fn test_compact_recovers_from_interrupted_staging_journal() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str(), "--timezone", "utc"])
        .assert()
        .success();

    env.write_source_file("doc.txt", b"v0");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--timezone", "utc", "--name", "snap0"])
        .assert()
        .success();

    env.write_source_file("doc.txt", b"v1");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--timezone", "utc", "--name", "snap1"])
        .assert()
        .success();

    env.write_source_file("doc.txt", b"v2");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--timezone", "utc", "--name", "snap2"])
        .assert()
        .success();

    // Verify diff in snap1 exists (holding v0)
    let diff_file = env.remote_path.join("diffs/snap1/doc.txt");
    assert!(diff_file.exists());

    // Read timestamps from catalog
    let catalog_content = fs::read_to_string(env.remote_path.join(".meta/snapshots.jsonl")).unwrap();
    let summaries: Vec<serde_json::Value> = catalog_content
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let t_snap1 = summaries.iter().find(|s| s["name"] == "snap1").unwrap()["created_at"].as_str().unwrap();
    let t_snap2 = summaries.iter().find(|s| s["name"] == "snap2").unwrap()["created_at"].as_str().unwrap();

    // Simulate an ungraceful crash (kill -9) midway through Stage 1 of a previous compaction:
    // doc.txt from snap1 was moved to .meta/staging/daily-rec/doc.txt and journal was written
    let staging_dir = env.remote_path.join(".meta/staging/daily-rec");
    fs::create_dir_all(&staging_dir).unwrap();
    let staged_file = staging_dir.join("doc.txt");
    fs::rename(&diff_file, &staged_file).unwrap();
    assert!(!diff_file.exists());
    assert!(staged_file.exists());

    let journal_file = env.remote_path.join(".meta/staging/daily-rec.journal.json");
    let remote_base = env.remote_str();
    let journal_json = serde_json::json!([
        [
            format!("{remote_base}/diffs/snap1/doc.txt"),
            format!("{remote_base}/.meta/staging/daily-rec/doc.txt")
        ]
    ]);
    fs::write(&journal_file, serde_json::to_string(&journal_json).unwrap()).unwrap();

    // Now run compact: it must detect the journal, recover the staged file back to snap1, and finish compaction successfully!
    env.command()
        .args([
            "compact",
            "--from",
            t_snap1,
            "--to",
            t_snap2,
            "--name",
            "daily-rec",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Compaction complete: daily-rec"));

    // Verify that daily-rec now has the compacted diff
    assert!(env.remote_path.join("diffs/daily-rec/doc.txt").exists());
    assert!(!journal_file.exists(), "journal file must be cleaned up on commit");

    // Point-in-time restore of snap0 (S_prev)
    let restore_snap0 = env.temp_dir.path().join("restore_snap0_journal");
    env.command()
        .args([
            "restore",
            "snap0",
            "--to",
            &restore_snap0.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();
    assert_eq!(fs::read(restore_snap0.join("doc.txt")).unwrap(), b"v0");

    // Point-in-time restore of daily-rec (anchor)
    let restore_daily = env.temp_dir.path().join("restore_daily_journal");
    env.command()
        .args([
            "restore",
            "daily-rec",
            "--to",
            &restore_daily.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();
    assert_eq!(fs::read(restore_daily.join("doc.txt")).unwrap(), b"v2");
}

#[test]
fn test_compact_batch_moves_and_purge_integrity() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str(), "--timezone", "utc"])
        .assert()
        .success();

    // Snapshot 0: baseline with multiple files and nested directories
    env.write_source_file("f1.txt", b"f1_v0");
    env.write_source_file("f2.txt", b"f2_v0");
    env.write_source_file("sub/f3.txt", b"f3_v0");
    env.write_source_file("sub/f4.txt", b"f4_v0");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "snap0"])
        .assert()
        .success();

    // Snapshot 1: modify f1, f3
    env.write_source_file("f1.txt", b"f1_v1");
    env.write_source_file("sub/f3.txt", b"f3_v1");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "snap1"])
        .assert()
        .success();

    // Snapshot 2: modify f2, f4
    env.write_source_file("f2.txt", b"f2_v2");
    env.write_source_file("sub/f4.txt", b"f4_v2");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "snap2"])
        .assert()
        .success();

    // Read timestamps from catalog
    let catalog_content = fs::read_to_string(env.remote_path.join(".meta/snapshots.jsonl")).unwrap();
    let summaries: Vec<serde_json::Value> = catalog_content
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let t_snap1 = summaries.iter().find(|s| s["name"] == "snap1").unwrap()["created_at"].as_str().unwrap();
    let t_snap2 = summaries.iter().find(|s| s["name"] == "snap2").unwrap()["created_at"].as_str().unwrap();

    // Run compact across snap1 and snap2
    env.command()
        .args([
            "compact",
            "--from",
            t_snap1,
            "--to",
            t_snap2,
            "--name",
            "consolidated-batch",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Compaction complete: consolidated-batch"));

    // Check that source snapshot diff directories were purged
    assert!(!env.remote_path.join("diffs/snap1").exists(), "diffs/snap1 must be purged");
    assert!(!env.remote_path.join("diffs/snap2").exists(), "diffs/snap2 must be purged");

    // Check that consolidated diff contains kept versions needed by snap0
    let consolidated_diff = env.remote_path.join("diffs/consolidated-batch");
    assert!(consolidated_diff.exists(), "consolidated diff dir must exist");
    assert_eq!(fs::read(consolidated_diff.join("f1.txt")).unwrap(), b"f1_v0");
    assert_eq!(fs::read(consolidated_diff.join("f2.txt")).unwrap(), b"f2_v0");
    assert_eq!(fs::read(consolidated_diff.join("sub/f3.txt")).unwrap(), b"f3_v0");
    assert_eq!(fs::read(consolidated_diff.join("sub/f4.txt")).unwrap(), b"f4_v0");

    // Point-in-time restore of snap0 must succeed and match exact baseline contents
    let restore_dir = env.temp_dir.path().join("restore_snap0");
    env.command()
        .args([
            "restore",
            "snap0",
            "--to",
            &restore_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success();

    assert_eq!(fs::read(restore_dir.join("f1.txt")).unwrap(), b"f1_v0");
    assert_eq!(fs::read(restore_dir.join("f2.txt")).unwrap(), b"f2_v0");
    assert_eq!(fs::read(restore_dir.join("sub/f3.txt")).unwrap(), b"f3_v0");
    assert_eq!(fs::read(restore_dir.join("sub/f4.txt")).unwrap(), b"f4_v0");

    // Point-in-time restore of consolidated-batch
    let restore_batch = env.temp_dir.path().join("restore_batch");
    env.command()
        .args([
            "restore",
            "consolidated-batch",
            "--to",
            &restore_batch.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();

    assert_eq!(fs::read(restore_batch.join("f1.txt")).unwrap(), b"f1_v1");
    assert_eq!(fs::read(restore_batch.join("f2.txt")).unwrap(), b"f2_v2");
    assert_eq!(fs::read(restore_batch.join("sub/f3.txt")).unwrap(), b"f3_v1");
    assert_eq!(fs::read(restore_batch.join("sub/f4.txt")).unwrap(), b"f4_v2");
}

#[test]
fn test_compact_multi_file_churn_and_full_restore() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str(), "--timezone", "utc"])
        .assert()
        .success();

    // Baseline (S_prev): Day 0 with all 8 files at v0
    env.write_source_file("f_mod1", b"f_mod1_v0");
    env.write_source_file("f_mod2", b"f_mod2_v0");
    env.write_source_file("f_mod3", b"f_mod3_v0");
    env.write_source_file("f_single1", b"f_single1_v0");
    env.write_source_file("f_single2", b"f_single2_v0");
    env.write_source_file("f_del", b"f_del_v0");
    env.write_source_file("f_static", b"f_static_v0");
    env.write_source_file("f_post", b"f_post_v0");

    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "sprev"])
        .assert()
        .success();

    // Snapshot S1 (10:00):
    // - f_mod1, f_mod2, f_mod3 updated to v1
    // - f_single1 updated to v1
    env.write_source_file("f_mod1", b"f_mod1_v1");
    env.write_source_file("f_mod2", b"f_mod2_v1");
    env.write_source_file("f_mod3", b"f_mod3_v1");
    env.write_source_file("f_single1", b"f_single1_v1");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s1"])
        .assert()
        .success();

    // Snapshot S2 (14:00):
    // - f_mod1, f_mod2, f_mod3 updated to v2
    // - f_single2 updated to v1
    // - f_del deleted
    env.write_source_file("f_mod1", b"f_mod1_v2");
    env.write_source_file("f_mod2", b"f_mod2_v2");
    env.write_source_file("f_mod3", b"f_mod3_v2");
    env.write_source_file("f_single2", b"f_single2_v1");
    fs::remove_file(env.source_path.join("f_del")).unwrap();
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s2"])
        .assert()
        .success();

    // Snapshot S3 (Anchor, 20:00):
    // - f_mod1, f_mod2, f_mod3 updated to v3
    env.write_source_file("f_mod1", b"f_mod1_v3");
    env.write_source_file("f_mod2", b"f_mod2_v3");
    env.write_source_file("f_mod3", b"f_mod3_v3");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s3"])
        .assert()
        .success();

    // Snapshot S_post (Day 2):
    // - f_post updated to v1
    env.write_source_file("f_post", b"f_post_v1");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "spost"])
        .assert()
        .success();

    // Read timestamps from catalog
    let catalog_content = fs::read_to_string(env.remote_path.join(".meta/snapshots.jsonl")).unwrap();
    let summaries: Vec<serde_json::Value> = catalog_content
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let t_s1 = summaries.iter().find(|s| s["name"] == "s1").unwrap()["created_at"].as_str().unwrap();
    let t_s3 = summaries.iter().find(|s| s["name"] == "s3").unwrap()["created_at"].as_str().unwrap();

    // Compact range [s1 .. s3] -> consolidated-churn
    env.command()
        .args([
            "compact",
            "--from",
            t_s1,
            "--to",
            t_s3,
            "--name",
            "consolidated-churn",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
            "--summary",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Compaction complete: consolidated-churn"))
        .stdout(predicate::str::contains("Diff files kept:      6"))
        .stdout(predicate::str::contains("Diff files discarded: 6"));

    // Check kept diffs in consolidated-churn directory:
    // Exactly 6 files, all holding v0
    let consolidated_diff = env.remote_path.join("diffs/consolidated-churn");
    assert!(consolidated_diff.exists());
    assert_eq!(fs::read(consolidated_diff.join("f_mod1")).unwrap(), b"f_mod1_v0");
    assert_eq!(fs::read(consolidated_diff.join("f_mod2")).unwrap(), b"f_mod2_v0");
    assert_eq!(fs::read(consolidated_diff.join("f_mod3")).unwrap(), b"f_mod3_v0");
    assert_eq!(fs::read(consolidated_diff.join("f_single1")).unwrap(), b"f_single1_v0");
    assert_eq!(fs::read(consolidated_diff.join("f_single2")).unwrap(), b"f_single2_v0");
    assert_eq!(fs::read(consolidated_diff.join("f_del")).unwrap(), b"f_del_v0");

    // Point-in-time restore 1: Restore S_prev
    // Expected: all 8 files present at v0
    let restore_sprev = env.temp_dir.path().join("restore_sprev");
    env.command()
        .args([
            "restore",
            "sprev",
            "--to",
            &restore_sprev.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();

    assert_eq!(fs::read(restore_sprev.join("f_mod1")).unwrap(), b"f_mod1_v0");
    assert_eq!(fs::read(restore_sprev.join("f_mod2")).unwrap(), b"f_mod2_v0");
    assert_eq!(fs::read(restore_sprev.join("f_mod3")).unwrap(), b"f_mod3_v0");
    assert_eq!(fs::read(restore_sprev.join("f_single1")).unwrap(), b"f_single1_v0");
    assert_eq!(fs::read(restore_sprev.join("f_single2")).unwrap(), b"f_single2_v0");
    assert_eq!(fs::read(restore_sprev.join("f_del")).unwrap(), b"f_del_v0");
    assert_eq!(fs::read(restore_sprev.join("f_static")).unwrap(), b"f_static_v0");
    assert_eq!(fs::read(restore_sprev.join("f_post")).unwrap(), b"f_post_v0");

    // Point-in-time restore 2: Restore consolidated-churn (Anchor S3)
    // Expected: f_mod* at v3, f_single* at v1, f_static at v0, f_post at v0, f_del ABSENT
    let restore_anchor = env.temp_dir.path().join("restore_anchor");
    env.command()
        .args([
            "restore",
            "consolidated-churn",
            "--to",
            &restore_anchor.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();

    assert_eq!(fs::read(restore_anchor.join("f_mod1")).unwrap(), b"f_mod1_v3");
    assert_eq!(fs::read(restore_anchor.join("f_mod2")).unwrap(), b"f_mod2_v3");
    assert_eq!(fs::read(restore_anchor.join("f_mod3")).unwrap(), b"f_mod3_v3");
    assert_eq!(fs::read(restore_anchor.join("f_single1")).unwrap(), b"f_single1_v1");
    assert_eq!(fs::read(restore_anchor.join("f_single2")).unwrap(), b"f_single2_v1");
    assert_eq!(fs::read(restore_anchor.join("f_static")).unwrap(), b"f_static_v0");
    assert_eq!(fs::read(restore_anchor.join("f_post")).unwrap(), b"f_post_v0");
    assert!(!restore_anchor.join("f_del").exists(), "f_del was deleted in s2");

    // Point-in-time restore 3: Restore S_post
    // Expected: f_post at v1, f_mod* at v3, f_single* at v1, f_static at v0, f_del ABSENT
    let restore_spost = env.temp_dir.path().join("restore_spost");
    env.command()
        .args([
            "restore",
            "spost",
            "--to",
            &restore_spost.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();

    assert_eq!(fs::read(restore_spost.join("f_post")).unwrap(), b"f_post_v1");
    assert_eq!(fs::read(restore_spost.join("f_mod1")).unwrap(), b"f_mod1_v3");
    assert_eq!(fs::read(restore_spost.join("f_mod2")).unwrap(), b"f_mod2_v3");
    assert_eq!(fs::read(restore_spost.join("f_mod3")).unwrap(), b"f_mod3_v3");
    assert_eq!(fs::read(restore_spost.join("f_single1")).unwrap(), b"f_single1_v1");
    assert_eq!(fs::read(restore_spost.join("f_single2")).unwrap(), b"f_single2_v1");
    assert_eq!(fs::read(restore_spost.join("f_static")).unwrap(), b"f_static_v0");
    assert!(!restore_spost.join("f_del").exists());
}

#[test]
fn test_compact_beginning_of_history() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str(), "--timezone", "utc"])
        .assert()
        .success();

    // S1: file1 v1, file2 v1
    env.write_source_file("file1.txt", b"file1_v1");
    env.write_source_file("file2.txt", b"file2_v1");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s1"])
        .assert()
        .success();

    // S2: file1 v2, file2 deleted, file3 v1
    env.write_source_file("file1.txt", b"file1_v2");
    fs::remove_file(env.source_path.join("file2.txt")).unwrap();
    env.write_source_file("file3.txt", b"file3_v1");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s2"])
        .assert()
        .success();

    // S3: file1 v3, file3 v2
    env.write_source_file("file1.txt", b"file1_v3");
    env.write_source_file("file3.txt", b"file3_v2");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s3"])
        .assert()
        .success();

    // Read timestamps from catalog
    let catalog_content = fs::read_to_string(env.remote_path.join(".meta/snapshots.jsonl")).unwrap();
    let summaries: Vec<serde_json::Value> = catalog_content
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let t_s1 = summaries.iter().find(|s| s["name"] == "s1").unwrap()["created_at"].as_str().unwrap();
    let t_s3 = summaries.iter().find(|s| s["name"] == "s3").unwrap()["created_at"].as_str().unwrap();

    // Compact range starting at beginning of history (S_prev is None)
    env.command()
        .args([
            "compact",
            "--from",
            t_s1,
            "--to",
            t_s3,
            "--name",
            "consolidated-start",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
            "--summary",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Preceding snapshot:   (none — beginning of history)"))
        .stdout(predicate::str::contains("Diff files kept:      0 (0 B)"));

    // diffs/consolidated-start must exist and be empty
    let diff_dir = env.remote_path.join("diffs/consolidated-start");
    assert!(diff_dir.exists(), "Diff directory must exist to preserve 1:1 invariant");
    let diff_entries: Vec<_> = fs::read_dir(&diff_dir).unwrap().collect();
    assert!(diff_entries.is_empty(), "Diff dir must be empty when prev is None");

    // Point-in-time restore of consolidated-start
    let restore_dir = env.temp_dir.path().join("restore_start");
    env.command()
        .args([
            "restore",
            "consolidated-start",
            "--to",
            &restore_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();

    assert_eq!(fs::read(restore_dir.join("file1.txt")).unwrap(), b"file1_v3");
    assert_eq!(fs::read(restore_dir.join("file3.txt")).unwrap(), b"file3_v2");
    assert!(!restore_dir.join("file2.txt").exists());
}

#[test]
fn test_compact_reincarnation_and_circular_edits() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str(), "--timezone", "utc"])
        .assert()
        .success();

    // S_prev: reinc.txt = V0, circ.txt = VA
    env.write_source_file("reinc.txt", b"reinc_V0");
    env.write_source_file("circ.txt", b"circ_VA");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "sprev"])
        .assert()
        .success();

    // S1: reinc.txt deleted (moved to diff holding V0), circ.txt -> VB
    fs::remove_file(env.source_path.join("reinc.txt")).unwrap();
    env.write_source_file("circ.txt", b"circ_VB");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s1"])
        .assert()
        .success();

    // S2: reinc.txt reincarnated with V_new, circ.txt -> VC
    env.write_source_file("reinc.txt", b"reinc_Vnew");
    env.write_source_file("circ.txt", b"circ_VC");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s2"])
        .assert()
        .success();

    // S3 (Anchor): reinc.txt -> V_final, circ.txt reverted to VA
    env.write_source_file("reinc.txt", b"reinc_Vfinal");
    env.write_source_file("circ.txt", b"circ_VA");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s3"])
        .assert()
        .success();

    // Read timestamps from catalog
    let catalog_content = fs::read_to_string(env.remote_path.join(".meta/snapshots.jsonl")).unwrap();
    let summaries: Vec<serde_json::Value> = catalog_content
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let t_s1 = summaries.iter().find(|s| s["name"] == "s1").unwrap()["created_at"].as_str().unwrap();
    let t_s3 = summaries.iter().find(|s| s["name"] == "s3").unwrap()["created_at"].as_str().unwrap();

    // Compact [s1 .. s3] -> consolidated-edge
    env.command()
        .args([
            "compact",
            "--from",
            t_s1,
            "--to",
            t_s3,
            "--name",
            "consolidated-edge",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
            "--summary",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Compaction complete: consolidated-edge"));

    // Earliest diff rule check:
    // In consolidated-edge diff:
    // - reinc.txt must have V0 from s1 (needed for S_prev)
    // - circ.txt must have VA from s1 (needed for S_prev)
    let edge_diff = env.remote_path.join("diffs/consolidated-edge");
    assert_eq!(fs::read(edge_diff.join("reinc.txt")).unwrap(), b"reinc_V0");
    assert_eq!(fs::read(edge_diff.join("circ.txt")).unwrap(), b"circ_VA");

    // Point-in-time restore S_prev:
    let restore_sprev = env.temp_dir.path().join("restore_edge_sprev");
    env.command()
        .args([
            "restore",
            "sprev",
            "--to",
            &restore_sprev.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();

    assert_eq!(fs::read(restore_sprev.join("reinc.txt")).unwrap(), b"reinc_V0");
    assert_eq!(fs::read(restore_sprev.join("circ.txt")).unwrap(), b"circ_VA");

    // Point-in-time restore Anchor (consolidated-edge):
    let restore_anchor = env.temp_dir.path().join("restore_edge_anchor");
    env.command()
        .args([
            "restore",
            "consolidated-edge",
            "--to",
            &restore_anchor.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();

    assert_eq!(fs::read(restore_anchor.join("reinc.txt")).unwrap(), b"reinc_Vfinal");
    assert_eq!(fs::read(restore_anchor.join("circ.txt")).unwrap(), b"circ_VA");
}

#[test]
fn test_compact_recursive_hierarchy() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str(), "--timezone", "utc"])
        .assert()
        .success();

    // Baseline S0
    env.write_source_file("f1.txt", b"f1_base");
    env.write_source_file("f2.txt", b"f2_base");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s0"])
        .assert()
        .success();

    // Hourly H1: f1 -> h1
    env.write_source_file("f1.txt", b"f1_h1");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "h1"])
        .assert()
        .success();

    // Hourly H2: f1 -> h2
    env.write_source_file("f1.txt", b"f1_h2");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "h2"])
        .assert()
        .success();

    // Compact H1..H2 -> daily-1
    let catalog = fs::read_to_string(env.remote_path.join(".meta/snapshots.jsonl")).unwrap();
    let get_ts = |name: &str| {
        let summaries: Vec<serde_json::Value> = catalog
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        summaries.iter().find(|s| s["name"] == name).unwrap()["created_at"].as_str().unwrap().to_string()
    };
    let t_h1 = get_ts("h1");
    let t_h2 = get_ts("h2");

    env.command()
        .args([
            "compact",
            "--from",
            &t_h1,
            "--to",
            &t_h2,
            "--name",
            "daily-1",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .success();

    // Hourly H3: f2 -> h3
    env.write_source_file("f2.txt", b"f2_h3");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "h3"])
        .assert()
        .success();

    // Hourly H4: f2 -> h4
    env.write_source_file("f2.txt", b"f2_h4");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "h4"])
        .assert()
        .success();

    // Compact H3..H4 -> daily-2
    let catalog2 = fs::read_to_string(env.remote_path.join(".meta/snapshots.jsonl")).unwrap();
    let get_ts2 = |name: &str| {
        let summaries: Vec<serde_json::Value> = catalog2
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        summaries.iter().find(|s| s["name"] == name).unwrap()["created_at"].as_str().unwrap().to_string()
    };
    let t_h3 = get_ts2("h3");
    let t_h4 = get_ts2("h4");

    env.command()
        .args([
            "compact",
            "--from",
            &t_h3,
            "--to",
            &t_h4,
            "--name",
            "daily-2",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .success();

    // Tier 2: Compact daily-1..daily-2 -> weekly-1
    let catalog3 = fs::read_to_string(env.remote_path.join(".meta/snapshots.jsonl")).unwrap();
    let get_ts3 = |name: &str| {
        let summaries: Vec<serde_json::Value> = catalog3
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        summaries.iter().find(|s| s["name"] == name).unwrap()["created_at"].as_str().unwrap().to_string()
    };
    let t_daily1 = get_ts3("daily-1");
    let t_daily2 = get_ts3("daily-2");

    env.command()
        .args([
            "compact",
            "--from",
            &t_daily1,
            "--to",
            &t_daily2,
            "--name",
            "weekly-1",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .success();

    // Verify restore of baseline S0 across the recursive compact hierarchy
    let restore_s0 = env.temp_dir.path().join("restore_rec_s0");
    env.command()
        .args([
            "restore",
            "s0",
            "--to",
            &restore_s0.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();

    assert_eq!(fs::read(restore_s0.join("f1.txt")).unwrap(), b"f1_base");
    assert_eq!(fs::read(restore_s0.join("f2.txt")).unwrap(), b"f2_base");

    // Verify restore of consolidated weekly-1
    let restore_weekly = env.temp_dir.path().join("restore_rec_weekly");
    env.command()
        .args([
            "restore",
            "weekly-1",
            "--to",
            &restore_weekly.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();

    assert_eq!(fs::read(restore_weekly.join("f1.txt")).unwrap(), b"f1_h2");
    assert_eq!(fs::read(restore_weekly.join("f2.txt")).unwrap(), b"f2_h4");
}

#[test]
fn test_compact_verification_failure_rolls_back() {
    use incr_backup::compact::{build_compact_plan, execute_compact_plan, CompactOptions};
    use incr_backup::config::{Config, SnapshotTimezone};
    use incr_backup::rclone::Rclone;

    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str(), "--timezone", "utc"])
        .assert()
        .success();

    env.write_source_file("doc.txt", b"v0");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s0"])
        .assert()
        .success();

    env.write_source_file("doc.txt", b"v1");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s1"])
        .assert()
        .success();

    env.write_source_file("doc.txt", b"v2");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s2"])
        .assert()
        .success();

    let catalog_content = fs::read_to_string(env.remote_path.join(".meta/snapshots.jsonl")).unwrap();
    let summaries: Vec<serde_json::Value> = catalog_content
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let t_s1 = summaries.iter().find(|s| s["name"] == "s1").unwrap()["created_at"].as_str().unwrap();
    let t_s2 = summaries.iter().find(|s| s["name"] == "s2").unwrap()["created_at"].as_str().unwrap();

    let config = Config {
        source_path: env.source_str(),
        remote_path: env.remote_str(),
        rclone: Default::default(),
        exclude: Default::default(),
        verbose: 0,
        dry_run: false,
        timezone: SnapshotTimezone::Utc,
    };
    let rclone = Rclone::new(&config);

    let opts = CompactOptions {
        from: t_s1.to_string(),
        to: t_s2.to_string(),
        name: "fail-stage2".to_string(),
        dry_run: false,
        force_lock: false,
        summary: false,
        full_list: false,
    };

    let mut plan = build_compact_plan(&rclone, &env.remote_str(), &config, &opts).unwrap();
    assert_eq!(plan.files_to_keep.len(), 1);

    // Corrupt the expected MD5 in the plan so Stage 2 hash verification deliberately fails
    plan.files_to_keep[0].md5 = "tampered_bad_hash".to_string();

    let result = execute_compact_plan(&rclone, &env.remote_str(), &plan, std::time::Instant::now());
    assert!(result.is_err(), "Must return error on hash mismatch");
    let err_str = format!("{}", result.unwrap_err());
    assert!(
        err_str.contains("Hash verification failed for staged file"),
        "Unexpected error: {err_str}"
    );

    // Invariant: rollback_staged_files must have restored the diff back to s1
    assert!(
        env.remote_path.join("diffs/s1/doc.txt").exists(),
        "diffs/s1/doc.txt must be restored by rollback"
    );
    assert_eq!(fs::read(env.remote_path.join("diffs/s1/doc.txt")).unwrap(), b"v0");

    // Invariant: staging must be wiped
    assert!(!env.remote_path.join(".meta/staging/fail-stage2").exists());
    assert!(!env.remote_path.join(".meta/staging/fail-stage2.journal.json").exists());
    assert!(!env.remote_path.join(".meta/staging/fail-stage2.manifest.json").exists());

    // Invariant: original snapshots are 100% intact and can be restored
    let restore_s0 = env.temp_dir.path().join("restore_s0_after_fail");
    env.command()
        .args([
            "restore",
            "s0",
            "--to",
            &restore_s0.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();
    assert_eq!(fs::read(restore_s0.join("doc.txt")).unwrap(), b"v0");

    let restore_s2 = env.temp_dir.path().join("restore_s2_after_fail");
    env.command()
        .args([
            "restore",
            "s2",
            "--to",
            &restore_s2.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();
    assert_eq!(fs::read(restore_s2.join("doc.txt")).unwrap(), b"v2");
}

#[test]
fn test_compact_force_breaks_stale_lock() {
    let env = TestEnv::new();

    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str(), "--timezone", "utc"])
        .assert()
        .success();

    env.write_source_file("f.txt", b"v1");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s1"])
        .assert()
        .success();

    env.write_source_file("f.txt", b"v2");
    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str(), "--name", "s2"])
        .assert()
        .success();

    // Create a foreign lock on remote
    let lock_json = serde_json::json!({
        "holder": "foreign-agent.example.com",
        "pid": 99999,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "timeout_minutes": 600000
    });
    let meta_dir = env.remote_path.join(".meta");
    let lock_file = meta_dir.join("lock");
    fs::write(&lock_file, serde_json::to_string(&lock_json).unwrap()).unwrap();

    let from = "2020-01-01T00:00:00Z";
    let to = "2030-01-01T00:00:00Z";

    // 1. Without --force, compact must fail fast with lock held
    env.command()
        .args([
            "compact",
            "--from",
            from,
            "--to",
            to,
            "--name",
            "force-test",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("backup locked by 'foreign-agent.example.com'"));

    // 2. With --force, compact breaks the lock, completes, and cleanly releases lock
    env.command()
        .args([
            "compact",
            "--from",
            from,
            "--to",
            to,
            "--name",
            "force-test",
            "--remote",
            &env.remote_str(),
            "--timezone",
            "utc",
            "--force",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Compaction complete: force-test"));

    assert!(!lock_file.exists(), "Lock must be released upon successful compaction");
}
