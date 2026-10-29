# incr-backup

[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org)
[![Tests](https://img.shields.io/badge/tests-118%20passed-brightgreen.svg)]()
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)]()

> **Reverse-incremental snapshot backup orchestration for rclone cloud remotes (Google Drive, S3, etc.).**

`incr-backup` brings **rsnapshot-style** incremental snapshot backups to cloud storage without proprietary chunked containers. The latest state (`current/`) is **always directly browseable and downloadable** on Google Drive. Old versions and deleted files are preserved in timestamped snapshot directories using **zero-bandwidth server-side moves**.

---

## Key Features

- 📂 **Directly Browseable on Cloud Storage**: `current/` is an exact mirror of your source directory. Open Google Drive on your phone, tablet, or web browser and access your latest files directly.
- ⚡ **Zero-Bandwidth Historical Snapshots**: Changed or deleted files are moved server-side by rclone into `diffs/<snapshot>/`. Old versions take zero upload bandwidth.
- 🔄 **Append-Only, Discrete Snapshots**: Every sync creates an independent, immutable timestamped snapshot. No fixed retention slots, rotation overflow, tier promotions, or in-place overwrites. Compaction and pruning are decoupled and handled cleanly by `compact`.
- 🤝 **Adopt Existing Remotes (`--adopt`)**: Already synced 200 GB to Google Drive? `init --adopt` reorganizes existing files into `current/` via server-side moves in minutes with **zero re-uploading**.
- 🛡️ **Crash Recovery via Write-Ahead Journal (WAL)**: Interrupted syncs resume automatically from the last completed step. Every operation is strictly idempotent.
- 🔒 **Advisory Cloud Locking**: Prevents concurrent backup runs across machines or overlapping cron schedules, with automatic stale-lock expiration.
- 🔍 **Manifest-Based Zero-Bandwidth Diff**: Compare any two snapshots (`diff snap1 snap2`) in milliseconds without downloading file contents.
- 🎯 **Targeted & Verified Restore**: Restore entire snapshots or specific subpaths (`--path`), with optional cryptographic verification (`--verify`).
- 🗜️ **Dynamic Snapshot Consolidation (`compact`)**: Arbitrary snapshot frequencies and point-in-time recovery with zero data loss. Compact hourly snapshots into a single daily snapshot, or dailies into a monthly, safely discarding intermediate scratch revisions via a two-phase staging protocol.
- 🧹 **Purge from All Snapshots (`remove` / `rm`)**: Completely purge leaked credentials, sensitive directories, or GDPR-restricted files from `current/`, all historical diffs, and all manifests in one command.
- 🌐 **Timezone-Aware Snapshot Names**: Snapshots always embed an explicit, filesystem-safe timezone indicator: UTC (`Z`, e.g. `2026-10-29T04-28Z`) or local offset (`%z`, e.g. `2026-10-29T09-58+0530`), with zero colons (`:`) for universal cross-platform compatibility. Fully configurable via `--timezone` / `--tz`, env var, or TOML (`timezone = "local"` / `[snapshot]`).
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
git clone https://github.com/shabeer/incr_backup.git
cd incr_backup
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
# Initialize using settings from incr-backup.toml (or env vars):
incr-backup init

# OR initialize directly by specifying source and remote via CLI flags:
incr-backup init --source /home/user/documents --remote gdrive:backups/myhost

# OR adopt an existing 200 GB rclone directory with ZERO re-uploading:
incr-backup init --adopt
# (or: incr-backup init --source /home/user/documents --remote gdrive:backups/myhost --adopt)
```

### 2. Run Syncs

```bash
# Run incremental sync (creates an immutable, timestamped snapshot)
incr-backup sync

# Run sync with custom snapshot name:
incr-backup sync --name release-v1.0

# Run sync with local timezone timestamp (e.g., 2026-10-29T09-58+0530):
incr-backup sync --timezone local
# (or shorthand: --tz local)
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
incr-backup diff 2026-10-28T08-00Z 2026-10-28T07-00Z
incr-backup diff snap2 snap1 --stat
```

### 5. Restore Files

```bash
# Restore latest state (current mirror)
incr-backup restore current --to /tmp/restore-latest

# Restore exact state from an earlier snapshot
incr-backup restore 2026-10-27T00-00Z --to /tmp/restore-point-in-time --verify

# Restore only a specific file or folder
incr-backup restore daily-2026-10-28 --to /tmp/restore-taxes --path documents/taxes/
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

### 8. Manage Configuration Between Local and Remote (`config`)

```bash
# Compare local configuration vs remote metadata (.meta/config.json)
incr-backup config diff

# Push updated local configuration/excludes to remote metadata
incr-backup config push

# Pull configuration from remote into a local TOML file
# (refuses to overwrite existing files unless --overwrite is passed)
incr-backup config pull --to ./incr-backup.toml
```

### 9. Dynamic Snapshot Consolidation (`compact`)

Consolidate multiple snapshots in a user-specified date range into a single named snapshot using reverse-incremental dependency resolution and a two-phase safe commit protocol:

```bash
# Preview compaction plan with high-level summary
incr-backup compact \
    --from 2026-10-28T00:00:00+05:30 \
    --to 2026-10-28T23:59:59+05:30 \
    --name daily-2026-10-28 \
    --dry-run --summary

# Preview detailed file-by-file plan
incr-backup compact \
    --from 2026-10-28T00:00:00+05:30 \
    --to 2026-10-28T23:59:59+05:30 \
    --name daily-2026-10-28 \
    --dry-run --full-list

# Execute compaction (creates verified daily-2026-10-28 and purges source snapshots)
incr-backup compact \
    --from 2026-10-28T00:00:00+05:30 \
    --to 2026-10-28T23:59:59+05:30 \
    --name daily-2026-10-28
```


---

## Snapshot Lifecycle & Dynamic Consolidation

Unlike traditional tools that force rigid slot counts (e.g. 6 hourly, 7 daily) leading to rotation overflows or in-place overwrites, `incr-backup` uses **Dynamic Snapshot Consolidation**:

1. **Discrete Backups**: Every `sync` invocation creates an append-only snapshot directory in `diffs/<snapshot>/` with an immutable manifest in `.meta/manifests/<snapshot>.json`.
2. **Explicit Compaction (`compact`)**: Use `incr-backup compact --from <start> --to <end> --name <consolidated_name>` to merge intermediate snapshots across any date range. Intermediate scratch changes are safely discarded and earliest diff references are preserved with zero data loss.
3. **Decoupled Operation**: Fast syncs never block waiting for complex tier purges. Storage compaction runs on your schedule or via periodic cron maintenance.

---

## Configuration File (`incr-backup.toml`)

A fully documented template is available in [`config/incr-backup-example.toml`](./config/incr-backup-example.toml). You can create an `incr-backup.toml` file so you don't need to pass `--source` and `--remote` every time:

```toml
# Timezone used in snapshot timestamps: "utc" (default) or "local"
timezone = "utc"

[source]
path = "/home/user/documents"

[remote]
path = "gdrive:backups/myhost"

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
  init     Initialize a new backup configuration ([--source], [--remote], --adopt, --exclude)
  sync     Run a sync operation ([--name], --force)
  list     List all snapshots (--json)
  status   Show backup status and storage summary
  restore  Restore a snapshot to a local directory (--to, --path, --verify, --overwrite)
  diff     Show differences between two snapshots (--stat, --json, --only-added, --only-modified)
  verify   Verify backup integrity (--space, --check-hashes, --deep)
  compact  Consolidate snapshots in a date range into a single snapshot (--from, --to, --name, --summary, --full-list)
  remove   Remove a given directory or file(s) from all snapshots (alias: rm) (--path, --force, --json)
  config   Manage configuration between local file and remote metadata (push, pull, diff)
  help     Print help information

Global Options:
  -c, --config <PATH>       Config file path [default: ./incr-backup.toml] [env: INCR_BACKUP_CONFIG]
  -s, --source <PATH>       Override source directory [env: INCR_BACKUP_SOURCE]
  -r, --remote <REMOTE>     Override rclone remote path [env: INCR_BACKUP_REMOTE]
  -v, --verbose             Increase verbosity (-v, -vv, -vvv)
  -n, --dry-run             Show what would happen without making changes
      --timezone <utc|local> Timezone for snapshot timestamps (alias: --tz) [default: utc] [env: INCR_BACKUP_TIMEZONE]
      --rclone-path <PATH>  Path to rclone binary [default: rclone]
      --rclone-flags <FLAGS> Extra flags passed to every rclone call
```

---

## Recommended Cron Setup

```cron
# Sync: Run incremental backup every hour
0 * * * *   /usr/local/bin/incr-backup sync -c /etc/incr-backup/config.toml >> /var/log/incr-backup.log 2>&1

# Daily Consolidation: Every night at 2:30 AM, compact yesterday's hourly snapshots
30 2 * * *  /usr/local/bin/incr-backup compact --from "$(date -v-1d +\%Y-\%m-\%dT00:00:00\%z)" --to "$(date -v-1d +\%Y-\%m-\%dT23:59:59\%z)" --name "daily-$(date -v-1d +\%Y-\%m-\%d)" -c /etc/incr-backup/config.toml >> /var/log/incr-backup.log 2>&1
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
- **`test_init.rs`**: Basic init, config-based init, adopt pre-existing files, duplicate protection, nonexistent source handling.
- **`test_sync.rs`**: Full sync pipeline, incremental diff tracking, dry-run mode, status inspection, and timezone arguments (`--timezone local`, `--tz utc`).
- **`test_restore.rs`**: Current restore, point-in-time restore from diffs, overwrite protection, checksum validation.
- **`test_diff.rs`**: Added, modified, deleted file diff detection, stat summaries, and JSON serialization.
- **`test_locking.rs`**: Mutual exclusion, stale lock detection, and force break overrides.
- **`test_verify.rs`**: Structural integrity, file counts, sizes, remote checksum verification, and space reports.
- **`test_remove.rs`**: Single file and directory tree purging across current mirror and all historical diffs, manifest rewrites, dry-run mode, JSON output, and root protection.
- **`test_compact.rs`**: Dynamic snapshot consolidation, ISO-8601 timezone offset validation, single-snapshot aliasing, intra-range create/delete lifecycle, dry-run modes (`--summary`, `--full-list`), and mathematical point-in-time restore across pre-range, consolidated, and post-range snapshots.
- **`test_config.rs`**: Bidirectional configuration sync (`config push`, `config pull`), diff inspection (`config diff`), overwrite safety guards, and TOML timezone parsing (unquoted, sub-table placement, `[snapshot]`).

---

## Detailed Documentation

For a full step-by-step walkthrough of algorithms, crash recovery, space calculation formulas, and remote layout diagrams, see [**`walkthrough.md`**](walkthrough.md).

---

## License

Licensed under either of [Apache License, Version 2.0](https://www.apache.org/licenses/LICENSE-2.0) or [MIT License](licence.txt) at your option.

