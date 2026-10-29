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
            "--level",
            "hourly",
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
}

#[test]
fn test_compact_concrete_walkthrough_and_point_in_time_restore() {
    let env = TestEnv::new();

    // Setup the exact scenario from DESIGN_SNAPSHOT_CONSOLIDATION.md Sections 2, 3, and 4:
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
}
