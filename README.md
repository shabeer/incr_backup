# incr-backup

[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org)
[![Tests](https://img.shields.io/badge/tests-87%20passed-brightgreen.svg)]()
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)]()

> **Reverse-incremental snapshot backup orchestration for rclone cloud remotes (Google Drive, S3, etc.).**

`incr-backup` brings **rsnapshot-style** incremental snapshot backups to cloud storage without proprietary chunked containers. The latest state (`current/`) is **always directly browseable and downloadable** on Google Drive. Old versions and deleted files are preserved in timestamped snapshot directories using **zero-bandwidth server-side moves**.

---

## Key Features

- 📂 **Directly Browseable on Cloud Storage**: `current/` is an exact mirror of your source directory. Open Google Drive on your phone, tablet, or web browser and access your latest files directly.
- ⚡ **Zero-Bandwidth Historical Snapshots**: Changed or deleted files are moved server-side by rclone into `diffs/<snapshot>/`. Old versions take zero upload bandwidth.
- 🔄 **rsnapshot-Style Retention**: Standard snapshot tiers: `hourly` → `daily` → `weekly` → `monthly`. Promotions between tiers are instant server-side folder renames.
- 🤝 **Adopt Existing Remotes (`--adopt`)**: Already synced 200 GB to Google Drive? `init --adopt` reorganizes existing files into `current/` via server-side moves in minutes with **zero re-uploading**.
- 🛡️ **Crash Recovery via Write-Ahead Journal (WAL)**: Interrupted syncs resume automatically from the last completed step. Every operation is strictly idempotent.
- 🔒 **Advisory Cloud Locking**: Prevents concurrent backup runs across machines or overlapping cron schedules, with automatic stale-lock expiration.
- 🔍 **Manifest-Based Zero-Bandwidth Diff**: Compare any two snapshots (`diff hourly.0 daily.0`) in milliseconds without downloading file contents.
- 🎯 **Targeted & Verified Restore**: Restore entire snapshots or specific subpaths (`--path`), with optional cryptographic verification (`--verify`).
- 🧹 **Purge from All Snapshots (`remove` / `rm`)**: Completely purge leaked credentials, sensitive directories, or GDPR-restricted files from `current/`, all historical diffs, and all manifests in one command.
- ⚡ **Blazing Fast Local Parallel Scanner**: Multithreaded file traversal and MD5 checksum computation powered by `rayon` and `walkdir`.
- 📦 **Single Standalone Binary**: No Python, JVM, or runtime dependencies. Just copy the 3 MB binary and run.

---

## Requirements

- **`rclone`** (v1.50+) configured and accessible in your `$PATH`.
- Supported cloud backends: **Google Drive** (primary, supports server-side move), local filesystem, SFTP, S3, OneDrive, Dropbox, etc.

---

## Installation

### From Source

```bash
git clone https://github.com/shabeer/shabeer_incr_backup.git
cd shabeer_incr_backup
cargo build --release
sudo cp target/release/incr-backup /usr/local/bin/
```

Verify installation:
```bash
incr-backup --version
```

---

## Quick Start in 5 Minutes

### 1. Initialize a Remote

```bash
# Initialize a new backup
incr-backup init --source /home/user/documents --remote gdrive:backups/myhost

# OR adopt an existing 200 GB rclone directory with ZERO re-uploading:
incr-backup init --source /home/user/documents --remote gdrive:backups/myhost --adopt
```

### 2. Run Syncs

```bash
# Run hourly sync (standard cron run)
incr-backup sync --level hourly

# Force a new snapshot even if less than 30 minutes elapsed:
incr-backup sync --level hourly --force-rotate

# Run daily sync (rotates dailies, promotes oldest hourly to daily.0, runs sync):
incr-backup sync --level daily
```

### 3. Inspect Status and Snapshots

```bash
# Overview of backup storage, locks, and configuration
incr-backup status

# List all retained snapshots
incr-backup list
```

### 4. Diff Snapshots

```bash
# Compare what changed between two snapshots (zero bandwidth)
incr-backup diff hourly.0 hourly.1
incr-backup diff hourly.0 daily.0 --stat
```

### 5. Restore Files

```bash
# Restore latest state (current mirror)
incr-backup restore current --to /tmp/restore-latest

# Restore exact state from 2 days ago
incr-backup restore daily.1 --to /tmp/restore-point-in-time --verify

# Restore only a specific file or folder
incr-backup restore daily.1 --to /tmp/restore-taxes --path documents/taxes/
```

### 6. Verify Backup Integrity

```bash
# Run 12 integrity checks and generate a space usage report
incr-backup verify --space --check-hashes
```

### 7. Purge Files or Folders Across All Snapshots (`remove` / `rm`)

```bash
# Remove leaked credentials or sensitive files from current/ and all past snapshots
incr-backup remove secret.env

# Remove an entire directory tree across all snapshots
incr-backup remove documents/confidential/

# Dry-run preview of what would be removed
incr-backup remove build/ --dry-run
```

---

## Configuration File (`incr-backup.toml`)

You can create an `incr-backup.toml` file so you don't need to pass `--source` and `--remote` every time:

```toml
[source]
path = "/home/user/documents"

[remote]
path = "gdrive:backups/myhost"

[retention]
hourly = 6
daily = 7
weekly = 4
monthly = 6
min_interval_minutes = 30  # Update in place if sync runs more often than this

[rclone]
path = "rclone"
extra_flags = ""

[exclude]
patterns = ["*.tmp", ".DS_Store", "node_modules/**", "target/**"]
```

### Configuration Priority
CLI Flags > Environment Variables (`INCR_BACKUP_SOURCE`, etc.) > TOML Configuration > Built-in Defaults.

---

## CLI Reference

```text
Usage: incr-backup [OPTIONS] <COMMAND>

Commands:
  init     Initialize a new backup configuration (--source, --remote, --adopt, --retention)
  sync     Run a sync operation (--level <hourly|daily|weekly|monthly>, --force-rotate, --force)
  list     List all snapshots (--json)
  status   Show backup status and storage summary
  restore  Restore a snapshot to a local directory (--to, --path, --verify, --overwrite)
  diff     Show differences between two snapshots (--stat, --json, --only-added, --only-modified)
  verify   Verify backup integrity (--space, --check-hashes, --deep)
  remove   Remove a given directory or file(s) from all snapshots (alias: rm) (--path, --force, --json)
  help     Print help information

Global Options:
  -c, --config <PATH>       Config file path [default: ./incr-backup.toml] [env: INCR_BACKUP_CONFIG]
  -s, --source <PATH>       Override source directory [env: INCR_BACKUP_SOURCE]
  -r, --remote <REMOTE>     Override rclone remote path [env: INCR_BACKUP_REMOTE]
  -v, --verbose             Increase verbosity (-v, -vv, -vvv)
  -n, --dry-run             Show what would happen without making changes
      --rclone-path <PATH>  Path to rclone binary [default: rclone]
      --rclone-flags <FLAGS> Extra flags passed to every rclone call
```

---

## Recommended Cron Setup

```cron
# Hourly: Every hour on the hour
0 * * * *   /usr/local/bin/incr-backup sync --level hourly -c /etc/incr-backup/config.toml >> /var/log/incr-backup.log 2>&1

# Daily: Every morning at 2:30 AM
30 2 * * *  /usr/local/bin/incr-backup sync --level daily  -c /etc/incr-backup/config.toml >> /var/log/incr-backup.log 2>&1

# Weekly: Every Sunday at 3:30 AM
30 3 * * 0  /usr/local/bin/incr-backup sync --level weekly -c /etc/incr-backup/config.toml >> /var/log/incr-backup.log 2>&1

# Monthly: 1st of every month at 4:30 AM
30 4 1 * *  /usr/local/bin/incr-backup sync --level monthly -c /etc/incr-backup/config.toml >> /var/log/incr-backup.log 2>&1
```

---

## Testing & Quality Assurance

The codebase includes comprehensive unit and integration tests against real `rclone` local operations:

```bash
# Run all unit tests and integration tests
cargo test

# Run tests with detailed log output
RUST_LOG=info cargo test -- --nocapture
```

Test coverage includes:
- **`test_init.rs`**: Basic init, adopt pre-existing files, duplicate protection, nonexistent source handling.
- **`test_sync.rs`**: Full sync pipeline, incremental diff tracking, dry-run mode, status inspection.
- **`test_restore.rs`**: Current restore, point-in-time restore from diffs, overwrite protection, checksum validation.
- **`test_diff.rs`**: Added, modified, deleted file diff detection, stat summaries, and JSON serialization.
- **`test_locking.rs`**: Mutual exclusion, stale lock detection, and force break overrides.
- **`test_verify.rs`**: Structural integrity, file counts, sizes, remote checksum verification, and space reports.
- **`test_remove.rs`**: Single file and directory tree purging across current mirror and all historical diffs, manifest rewrites, dry-run mode, JSON output, and root protection.

---

## Detailed Documentation

For a full step-by-step walkthrough of algorithms, crash recovery, space calculation formulas, and remote layout diagrams, see [**`walkthrough.md`**](file:///Users/shabeer/git/shabeer_incr_backup/walkthrough.md).

---

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.

