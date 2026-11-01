mod common;

use common::TestEnv;
use predicates::prelude::*;

#[test]
fn test_restore_current() {
    let env = TestEnv::new();
    env.write_source_file("file1.txt", b"Content 1");
    env.write_source_file("subdir/file2.txt", b"Content 2");

    // Init & sync
    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // Restore to target directory
    let restore_dir = env.temp_dir.path().join("restored_current");
    env.command()
        .args([
            "restore",
            "current",
            "--to",
            &restore_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Restored"));

    assert!(restore_dir.join("file1.txt").exists());
    assert_eq!(
        std::fs::read_to_string(restore_dir.join("file1.txt")).unwrap(),
        "Content 1"
    );
    assert!(restore_dir.join("subdir/file2.txt").exists());
}

#[test]
fn test_restore_point_in_time() {
    let env = TestEnv::new();
    env.write_source_file("document.txt", b"Version 1");
    env.write_source_file("static.txt", b"Never changes");

    // Init
    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // Sync v1
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

    // Modify document.txt
    env.write_source_file("document.txt", b"Version 2");

    // Sync v2
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

    // Now restore snap1 (which contains Version 1 of document.txt)
    let restore_dir = env.temp_dir.path().join("restored_v1");
    env.command()
        .args([
            "restore",
            "snap1",
            "--to",
            &restore_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Restored"));

    // Verify document.txt is Version 1 and static.txt was also restored
    assert!(restore_dir.join("document.txt").exists());
    assert_eq!(
        std::fs::read_to_string(restore_dir.join("document.txt")).unwrap(),
        "Version 1"
    );
    assert!(restore_dir.join("static.txt").exists());
    assert_eq!(
        std::fs::read_to_string(restore_dir.join("static.txt")).unwrap(),
        "Never changes"
    );
}

#[test]
fn test_restore_dest_not_empty_requires_overwrite() {
    let env = TestEnv::new();
    env.write_source_file("doc.txt", b"data");

    // Init & sync
    env.command()
        .args(["init", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    env.command()
        .args(["sync", "--source", &env.source_str(), "--remote", &env.remote_str()])
        .assert()
        .success();

    // Create a non-empty restore destination
    let restore_dir = env.temp_dir.path().join("non_empty");
    std::fs::create_dir_all(&restore_dir).unwrap();
    std::fs::write(restore_dir.join("existing.txt"), "preexisting").unwrap();

    // Restore without --overwrite fails
    env.command()
        .args([
            "restore",
            "current",
            "--to",
            &restore_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not empty"));

    // Restore with --overwrite succeeds
    env.command()
        .args([
            "restore",
            "current",
            "--to",
            &restore_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--overwrite",
        ])
        .assert()
        .success();

    assert!(restore_dir.join("doc.txt").exists());
}

#[test]
fn test_restore_adopted_file_after_delete_and_sync() {
    let env = TestEnv::new();

    // 1. Existing files in remote
    env.write_remote_file("file1.txt", b"Content 1");
    env.write_remote_file("file2.txt", b"Content 2");

    // Local source directory matches existing remote files
    env.write_source_file("file1.txt", b"Content 1");
    env.write_source_file("file2.txt", b"Content 2");

    // Init with --adopt
    env.command()
        .args([
            "init",
            "--adopt",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success();

    // 2. Delete a file in source
    env.delete_source_file("file2.txt");

    // 3. Sync is done
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

    // Sync created a timestamp diff directory with the deleted file correctly
    let diffs_path = env.remote_path.join("diffs");
    let diff_entries = std::fs::read_dir(&diffs_path).expect("read diffs dir");
    let diff_dirs: Vec<String> = diff_entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();

    assert_eq!(diff_dirs.len(), 1, "expected 1 diff directory");
    let diff_ts = &diff_dirs[0];
    assert!(
        diffs_path.join(diff_ts).join("file2.txt").exists(),
        "diff directory should contain deleted file2.txt"
    );

    // 4. Restore using diff timestamp
    let restore_dir = env.temp_dir.path().join("restored_from_diff_ts");
    env.command()
        .args([
            "restore",
            diff_ts,
            "--to",
            &restore_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success();

    // Deleted file should be restored!
    assert!(
        restore_dir.join("file2.txt").exists(),
        "deleted file2.txt should be restored when restoring using diff timestamp"
    );
    assert_eq!(
        std::fs::read_to_string(restore_dir.join("file2.txt")).unwrap(),
        "Content 2"
    );
    assert!(restore_dir.join("file1.txt").exists());
    assert_eq!(
        std::fs::read_to_string(restore_dir.join("file1.txt")).unwrap(),
        "Content 1"
    );
}

#[test]
fn test_restore_adopted_file_after_update_and_sync() {
    let env = TestEnv::new();

    // 1. Existing files in remote
    env.write_remote_file("file1.txt", b"Initial Version 1");
    env.write_remote_file("file2.txt", b"Unchanged Content");

    // Local source directory matches existing remote files
    env.write_source_file("file1.txt", b"Initial Version 1");
    env.write_source_file("file2.txt", b"Unchanged Content");

    // Init with --adopt
    env.command()
        .args([
            "init",
            "--adopt",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success();

    // 2. Update a file in source
    env.write_source_file("file1.txt", b"Updated Version 2");

    // 3. Sync is done
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

    // Sync created a timestamp diff directory with the updated file's previous version
    let diffs_path = env.remote_path.join("diffs");
    let diff_entries = std::fs::read_dir(&diffs_path).expect("read diffs dir");
    let diff_dirs: Vec<String> = diff_entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();

    assert_eq!(diff_dirs.len(), 1, "expected 1 diff directory");
    let diff_ts = &diff_dirs[0];
    assert!(
        diffs_path.join(diff_ts).join("file1.txt").exists(),
        "diff directory should contain old version of file1.txt"
    );

    // 4. Restore using diff timestamp
    let restore_dir = env.temp_dir.path().join("restored_from_diff_update");
    env.command()
        .args([
            "restore",
            diff_ts,
            "--to",
            &restore_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();

    // Updated file should be restored to its pre-update version (Initial Version 1)
    assert!(restore_dir.join("file1.txt").exists());
    assert_eq!(
        std::fs::read_to_string(restore_dir.join("file1.txt")).unwrap(),
        "Initial Version 1",
        "file1.txt should be restored to its pre-update version when restoring using diff timestamp"
    );

    // Unchanged file should also be restored
    assert!(restore_dir.join("file2.txt").exists());
    assert_eq!(
        std::fs::read_to_string(restore_dir.join("file2.txt")).unwrap(),
        "Unchanged Content"
    );
}

#[test]
fn test_restore_adopted_file_after_add_and_sync() {
    let env = TestEnv::new();

    // 1. Existing file in remote
    env.write_remote_file("existing.txt", b"Existing Content");
    env.write_source_file("existing.txt", b"Existing Content");

    // Init with --adopt
    env.command()
        .args([
            "init",
            "--adopt",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success();

    // 2. Add a new file in source
    env.write_source_file("added.txt", b"Added Content");

    // 3. Sync is done
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

    // 4. Restore snapshot
    let restore_dir = env.temp_dir.path().join("restored_from_add");
    env.command()
        .args([
            "restore",
            "current",
            "--to",
            &restore_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();

    assert!(restore_dir.join("existing.txt").exists());
    assert_eq!(
        std::fs::read_to_string(restore_dir.join("existing.txt")).unwrap(),
        "Existing Content"
    );
    assert!(restore_dir.join("added.txt").exists());
    assert_eq!(
        std::fs::read_to_string(restore_dir.join("added.txt")).unwrap(),
        "Added Content"
    );
}

#[test]
fn test_restore_adopted_lifecycle_add_update_delete() {
    let env = TestEnv::new();

    // 1. Existing files in remote
    env.write_remote_file("unchanged.txt", b"Unchanged Content");
    env.write_remote_file("updated.txt", b"Original Version");
    env.write_remote_file("deleted.txt", b"To Be Deleted Content");

    env.write_source_file("unchanged.txt", b"Unchanged Content");
    env.write_source_file("updated.txt", b"Original Version");
    env.write_source_file("deleted.txt", b"To Be Deleted Content");

    // Init with --adopt
    env.command()
        .args([
            "init",
            "--adopt",
            "--source",
            &env.source_str(),
            "--remote",
            &env.remote_str(),
        ])
        .assert()
        .success();

    // 2. Update, delete, and add files in source
    env.write_source_file("updated.txt", b"New Version Content");
    env.delete_source_file("deleted.txt");
    env.write_source_file("added.txt", b"Newly Added Content");

    // 3. Sync
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

    // Diff directory contains old updated.txt and deleted.txt
    let diffs_path = env.remote_path.join("diffs");
    let diff_entries = std::fs::read_dir(&diffs_path).expect("read diffs dir");
    let diff_dirs: Vec<String> = diff_entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();

    assert_eq!(diff_dirs.len(), 1, "expected 1 diff directory");
    let diff_ts = &diff_dirs[0];

    // 4. Restore using diff timestamp
    let restore_dir = env.temp_dir.path().join("restored_combined");
    env.command()
        .args([
            "restore",
            diff_ts,
            "--to",
            &restore_dir.to_string_lossy(),
            "--remote",
            &env.remote_str(),
            "--verify",
        ])
        .assert()
        .success();

    // Verify all files restored properly with verify passing
    assert!(restore_dir.join("unchanged.txt").exists());
    assert_eq!(
        std::fs::read_to_string(restore_dir.join("unchanged.txt")).unwrap(),
        "Unchanged Content"
    );

    assert!(restore_dir.join("updated.txt").exists());
    assert_eq!(
        std::fs::read_to_string(restore_dir.join("updated.txt")).unwrap(),
        "Original Version",
        "updated file should be restored to pre-update version"
    );

    assert!(
        restore_dir.join("deleted.txt").exists(),
        "deleted file should be restored from diff"
    );
    assert_eq!(
        std::fs::read_to_string(restore_dir.join("deleted.txt")).unwrap(),
        "To Be Deleted Content"
    );

    assert!(
        restore_dir.join("added.txt").exists(),
        "added file should be present in restored snapshot"
    );
    assert_eq!(
        std::fs::read_to_string(restore_dir.join("added.txt")).unwrap(),
        "Newly Added Content"
    );
}



