# incr-backup — Comprehensive Test Cases

> **Project:** incr-backup (rsnapshot-like incremental backup for rclone Google Drive remotes)
> **Version:** 0.1.0-draft
> **Last Updated:** 2026-10-28

---

## Table of Contents

- [1. `init` Command](#1-init-command)
- [2. `sync` Command — Core Operation](#2-sync-command--core-operation)
- [3. `sync` — Retention & Rotation](#3-sync--retention--rotation)
- [4. `sync` — Cascade Promotion](#4-sync--cascade-promotion)
- [5. `list` Command](#5-list-command)
- [6. `status` Command](#6-status-command)
- [7. `restore` Command](#7-restore-command)
- [8. `diff` Command](#8-diff-command)
- [9. `verify` Command](#9-verify-command)
- [10. Locking & Concurrency](#10-locking--concurrency)
- [11. Journal & Resumability](#11-journal--resumability)
- [12. Error Handling & Edge Cases](#12-error-handling--edge-cases)
- [13. Configuration & Environment](#13-configuration--environment)
- [14. Filesystem Edge Cases](#14-filesystem-edge-cases)
- [15. Network & Remote Failures](#15-network--remote-failures)
- [16. Multi-Source & Namespace Isolation](#16-multi-source--namespace-isolation)
- [17. `remove` Command — History Purging](#17-remove-command--history-purging)
- [18. `config` Command — Local & Remote Metadata Sync](#18-config-command--local--remote-metadata-sync)

---

## Legend

| Category      | Description                                                |
|---------------|------------------------------------------------------------|
| **unit**      | Tests a single function/module in isolation                |
| **integration** | Tests interaction between two or more components         |
| **e2e**       | Full end-to-end test against a real or emulated remote     |

---

## 1. `init` Command

### TC-INIT-001 — Basic initialization with defaults

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-INIT-001 |
| **Test Name**    | Initialize a new backup namespace on an empty remote |
| **Preconditions** | rclone remote `gdrive:` is configured and authenticated. Target path `gdrive:backups/myhost` does not exist. |
| **Steps**        | 1. Run `incr-backup init --remote gdrive:backups/myhost --source /home/user/docs` |
| **Expected Result** | - Exit code 0. <br>- Remote directory `gdrive:backups/myhost/current/` is created (empty). <br>- `gdrive:backups/myhost/.meta/config.json` is created with source path, remote, and default retention policy (e.g., `hourly: 6, daily: 7, weekly: 4, monthly: 12`). <br>- `gdrive:backups/myhost/.meta/journal.json` is created (empty/clean state). <br>- No `diffs/` directory created yet. <br>- Local config file is written (e.g., `~/.config/incr-backup/<namespace>.toml`). |
| **Category**     | e2e |

---

### TC-INIT-002 — Init with custom retention policy

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-INIT-002 |
| **Test Name**    | Initialize with custom retention counts |
| **Preconditions** | Remote is configured and target path does not exist. |
| **Steps**        | 1. Run `incr-backup init --remote gdrive:backups/myhost --source /home/user/docs --hourly 12 --daily 14 --weekly 8 --monthly 24` |
| **Expected Result** | - `config.json` contains `hourly: 12, daily: 14, weekly: 8, monthly: 24`. <br>- All other behavior identical to TC-INIT-001. |
| **Category**     | e2e |

---

### TC-INIT-003 — Init with `--adopt` on existing rclone-synced directory

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-INIT-003 |
| **Test Name**    | Adopt an existing rclone-synced remote directory |
| **Preconditions** | `gdrive:backups/myhost` already contains files previously synced with plain `rclone sync` (flat file structure, no `.meta/`, no `current/`, no `diffs/`). |
| **Steps**        | 1. Run `incr-backup init --remote gdrive:backups/myhost --source /home/user/docs --adopt` |
| **Expected Result** | - Tool emits warning notice: `[WARN] Adopting existing remote contents into 'current/'...` <br>- All existing files are moved (server-side) into `current/` subdirectory. Zero bytes re-uploaded. <br>- Missing directories (`diffs/`, `.meta/`, `.meta/manifests/`) are created. <br>- An initial manifest is created at `.meta/manifests/current.json` listing every file with path, size, md5, mtime. <br>- `config.json` is created. <br>- Exit code 0 with summary: "Adopted N files, total size X." |
| **Category**     | e2e |

---

### TC-INIT-004 — Init without `--adopt` on non-empty remote fails with suggestion

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-INIT-004 |
| **Test Name**    | Init fails and suggests --adopt if remote contains unmanaged files |
| **Preconditions** | `gdrive:backups/myhost` contains unmanaged files and lacks required backup structure (`current/`, `.meta/`, `diffs/`). |
| **Steps**        | 1. Run `incr-backup init --remote gdrive:backups/myhost --source /home/user/docs` |
| **Expected Result** | - Exit code non-zero (1). <br>- Error message: "Remote 'gdrive:backups/myhost' contains existing files but is not an initialized backup namespace (missing required directories)." <br>- Suggestion/hint: "Hint: To adopt existing files into 'current/', re-run with '--adopt'." <br>- No files moved, no directories created, no config written. |
| **Category**     | e2e |

---

### TC-INIT-005 — Adopt on partially initialized remote overwrites config and adopts

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-INIT-005 |
| **Test Name**    | Adopt on partial structure moves files, creates missing dirs, and overwrites config |
| **Preconditions** | `gdrive:backups/myhost` has partial structure (e.g. `.meta/` exists from an interrupted init) plus unmanaged files at root. |
| **Steps**        | 1. Run `incr-backup init --remote gdrive:backups/myhost --source /home/user/docs --adopt` |
| **Expected Result** | - Warning emitted: `[WARN] Adopting existing remote contents into 'current/'...` <br>- All non-backup root contents moved into `current/`. <br>- Missing directories (`diffs/`, `current/`) created. <br>- `.meta/config.json` overwritten with new configuration options. <br>- Manifest indexed and uploaded. <br>- Exit code 0. |
| **Category**     | e2e |

---

### TC-INIT-006 — Init on already-initialized path is a NO-OP

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-INIT-006 |
| **Test Name**    | Double-init is a successful NO-OP |
| **Preconditions** | `current/`, `.meta/`, and `diffs/` already exist on `gdrive:backups/myhost`. |
| **Steps**        | 1. Run `incr-backup init --remote gdrive:backups/myhost --source /home/user/docs` (or with `--adopt`) |
| **Expected Result** | - Exit code 0. <br>- Info message: `[INFO] Remote already initialized at gdrive:backups/myhost; nothing to do.` <br>- Zero writes to remote; existing metadata untouched. <br>- `--adopt` flag is ignored if passed. <br>- If stray root items exist, emits warning: `[WARN] Unmanaged root files/directories detected in '<remote>'... Please manually delete or move them out.` |
| **Category**     | e2e |

---

### TC-INIT-007 — Init with invalid remote

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-INIT-007 |
| **Test Name**    | Init with unconfigured rclone remote |
| **Preconditions** | rclone remote `nonexistent:` is not in rclone config. |
| **Steps**        | 1. Run `incr-backup init --remote nonexistent:backups/myhost --source /home/user/docs` |
| **Expected Result** | - Exit code non-zero. <br>- Error message includes rclone's "Failed to create file system" or similar. <br>- No partial state left on remote. |
| **Category**     | e2e |

---

### TC-INIT-008 — Init with expired auth token

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-INIT-008 |
| **Test Name**    | Init when rclone OAuth token has expired |
| **Preconditions** | rclone remote is configured but token is expired and cannot auto-refresh (e.g., refresh token revoked). |
| **Steps**        | 1. Run `incr-backup init --remote gdrive:backups/myhost --source /home/user/docs` |
| **Expected Result** | - Exit code non-zero. <br>- Error message indicates authentication failure. <br>- No partial state left on remote. |
| **Category**     | e2e |

---

### TC-INIT-009 — Init with source directory that doesn't exist

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-INIT-009 |
| **Test Name**    | Init with non-existent local source path |
| **Preconditions** | `/home/user/nonexistent_dir` does not exist. |
| **Steps**        | 1. Run `incr-backup init --remote gdrive:backups/myhost --source /home/user/nonexistent_dir` |
| **Expected Result** | - Exit code non-zero. <br>- Error: "Source directory `/home/user/nonexistent_dir` does not exist." |
| **Category**     | unit |

---

### TC-INIT-010 — Adopt with empty remote directory

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-INIT-010 |
| **Test Name**    | Adopt on empty remote silently ignores --adopt flag |
| **Preconditions** | `gdrive:backups/myhost` exists but contains zero files/dirs. |
| **Steps**        | 1. Run `incr-backup init --remote gdrive:backups/myhost --source /home/user/docs --adopt` |
| **Expected Result** | - Exit code 0. <br>- `--adopt` is silently ignored (no adopt warning or move operation). <br>- `current/`, `diffs/`, `.meta/`, and initial manifest created. <br>- Summary: "✓ Initialized empty backup at gdrive:backups/myhost". |
| **Category**     | e2e |

---

### TC-INIT-011 — Init from configuration file without CLI flags

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-INIT-011 |
| **Test Name**    | Init uses source, remote, and retention from config file when flags are omitted |
| **Preconditions** | `incr-backup.toml` exists with valid `[source]`, `[remote]`, and `[retention]`. Remote target is empty. |
| **Steps**        | 1. Run `incr-backup init` |
| **Expected Result** | - Exit code 0. <br>- `source` and `remote` read from `incr-backup.toml`. <br>- Retention from config file uploaded to remote `.meta/config.json`. <br>- Directory structure (`current/`, `diffs/`, `.meta/`) created on remote. |
| **Category**     | integration |

---

### TC-INIT-012 — Init with configuration file and CLI flag overrides

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-INIT-012 |
| **Test Name**    | CLI flags override config file values during init |
| **Preconditions** | `incr-backup.toml` exists with source and remote paths. Alternate remote target exists and is empty. |
| **Steps**        | 1. Run `incr-backup init --remote <other-remote>` |
| **Expected Result** | - Exit code 0. <br>- Alternate remote path is initialized rather than the path in `incr-backup.toml`. <br>- Source path is taken from `incr-backup.toml`. |
| **Category**     | integration |

---

### TC-INIT-013 — Init fails when source path is missing

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-INIT-013 |
| **Test Name**    | Init fails when source is not provided in config or CLI |
| **Preconditions** | Configuration file has `[remote]` but no `[source]`, and `--source` is not passed on CLI. |
| **Steps**        | 1. Run `incr-backup init` |
| **Expected Result** | - Exit code 1. <br>- Error message: "source path is required. Set [source].path in config, --source flag, or INCR_BACKUP_SOURCE env var". |
| **Category**     | unit |

---

### TC-INIT-014 — Init fails when remote path is missing

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-INIT-014 |
| **Test Name**    | Init fails when remote is not provided in config or CLI |
| **Preconditions** | Configuration file has `[source]` but no `[remote]`, and `--remote` is not passed on CLI. |
| **Steps**        | 1. Run `incr-backup init` |
| **Expected Result** | - Exit code 1. <br>- Error message: "remote path is required. Set [remote].path in config, --remote flag, or INCR_BACKUP_REMOTE env var". |
| **Category**     | unit |

---

## 2. `sync` Command — Core Operation

### TC-SYNC-001 — First sync after init (no prior snapshots)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-001 |
| **Test Name**    | Initial sync populates `current/` |
| **Preconditions** | `init` completed. Source has 10 files. No prior syncs. |
| **Steps**        | 1. Run `incr-backup sync --level hourly` |
| **Expected Result** | - Lock acquired in `.meta/lock`. <br>- Journal written with operation start. <br>- `rclone sync` runs from source → `current/` (no `--backup-dir` needed since no rotation target yet, OR `--backup-dir` pointing to `diffs/hourly.0/<timestamp>`). <br>- Manifest generated and uploaded to `.meta/manifests/`. <br>- `diffs/hourly.0/` snapshot directory created with timestamp marker. <br>- Journal cleared. Lock released. <br>- Exit code 0. |
| **Category**     | e2e |

---

### TC-SYNC-002 — Sync with changed files

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-002 |
| **Test Name**    | Sync detects and backs up changed files |
| **Preconditions** | One prior sync completed. 3 files modified in source since last sync. |
| **Steps**        | 1. Modify 3 files in source directory. <br>2. Run `incr-backup sync --level hourly` |
| **Expected Result** | - Hourly rotation occurs (hourly.0 → hourly.1, etc.). <br>- `rclone sync --backup-dir diffs/hourly.0/<timestamp>` runs. <br>- The 3 old versions of changed files appear in `diffs/hourly.0/<timestamp>/`. <br>- `current/` has the new versions. <br>- New manifest reflects updated md5/size/mtime for the 3 files. |
| **Category**     | e2e |

---

### TC-SYNC-003 — Sync with deleted files

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-003 |
| **Test Name**    | Deleted files are captured in diffs |
| **Preconditions** | One prior sync completed. 2 files deleted from source. |
| **Steps**        | 1. Delete 2 files from source. <br>2. Run `incr-backup sync --level hourly` |
| **Expected Result** | - The 2 deleted files appear in `diffs/hourly.0/<timestamp>/` (their last known versions). <br>- `current/` no longer contains the 2 files. <br>- New manifest does not list the 2 deleted files. |
| **Category**     | e2e |

---

### TC-SYNC-004 — Sync with added files

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-004 |
| **Test Name**    | Newly added files appear in current/ |
| **Preconditions** | One prior sync completed. 5 new files added to source. |
| **Steps**        | 1. Add 5 new files to source. <br>2. Run `incr-backup sync --level hourly` |
| **Expected Result** | - 5 new files uploaded to `current/`. <br>- `diffs/hourly.0/` has no entries for the new files (they didn't exist before). <br>- New manifest includes all 5 new files with correct hashes. |
| **Category**     | e2e |

---

### TC-SYNC-005 — Sync with mixed changes (add + modify + delete)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-005 |
| **Test Name**    | Sync handles mixed change types simultaneously |
| **Preconditions** | Prior sync completed. Source has 2 added, 3 modified, 1 deleted file. |
| **Steps**        | 1. Make mixed changes. <br>2. Run `incr-backup sync --level hourly` |
| **Expected Result** | - Added files in `current/` only. <br>- Modified files: old versions in `diffs/hourly.0/`, new versions in `current/`. <br>- Deleted file: old version in `diffs/hourly.0/`, absent from `current/`. <br>- Manifest accurate for all changes. |
| **Category**     | e2e |

---

### TC-SYNC-006 — Sync with no changes (idempotent)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-006 |
| **Test Name**    | Sync when source is unchanged |
| **Preconditions** | Prior sync completed. No changes in source. |
| **Steps**        | 1. Run `incr-backup sync --level hourly` |
| **Expected Result** | - Rotation still occurs (hourly slots shift). <br>- `rclone sync --backup-dir` runs but produces no backup files (nothing changed). <br>- New manifest is identical in content to previous. <br>- `diffs/hourly.0/` exists but contains only timestamp/manifest metadata (no file content). <br>- Exit code 0. |
| **Category**     | e2e |

---

### TC-SYNC-007 — Sync with empty source directory

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-007 |
| **Test Name**    | Sync when source directory is empty |
| **Preconditions** | Prior sync with 10 files. Source directory now emptied (all files removed). |
| **Steps**        | 1. Remove all files from source. <br>2. Run `incr-backup sync --level hourly` |
| **Expected Result** | - All 10 files moved to `diffs/hourly.0/`. <br>- `current/` is empty. <br>- Manifest has zero entries. <br>- Warning emitted: "Source directory is empty. All files will be deleted from current/." <br>- Exit code 0 (or configurable to require `--allow-empty`). |
| **Category**     | e2e |

---

### TC-SYNC-008 — Sync acquires and releases lock

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-008 |
| **Test Name**    | Lock lifecycle during sync |
| **Preconditions** | No lock present on remote. |
| **Steps**        | 1. Run `incr-backup sync --level hourly`. <br>2. Monitor `.meta/lock` during execution. |
| **Expected Result** | - `.meta/lock` is created at the start of sync with PID, hostname, and timestamp. <br>- `.meta/lock` is deleted upon successful completion. <br>- If interrupted, lock remains (tested separately). |
| **Category**     | integration |

---

### TC-SYNC-009 — Sync generates valid manifest

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-009 |
| **Test Name**    | Manifest accuracy after sync |
| **Preconditions** | Source has known files with pre-computed md5 hashes. |
| **Steps**        | 1. Run `incr-backup sync --level hourly`. <br>2. Download and parse the generated manifest. <br>3. Compare each entry against `rclone lsjson --hash` output. |
| **Expected Result** | - Manifest is valid JSON. <br>- Every file in `current/` has a manifest entry. <br>- Every manifest entry has `path`, `size`, `md5`, `mtime`. <br>- All fields match `rclone lsjson --hash` output exactly. |
| **Category**     | integration |

---

### TC-SYNC-010 — Dry-run mode

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-010 |
| **Test Name**    | Sync with --dry-run makes no changes |
| **Preconditions** | Initialized namespace with prior sync. Source has changes. |
| **Steps**        | 1. Run `incr-backup sync --level hourly --dry-run` |
| **Expected Result** | - Tool outputs what it *would* do (rotation plan, files to sync). <br>- No lock acquired. <br>- No files moved, uploaded, or deleted on remote. <br>- No manifest generated. <br>- Exit code 0. |
| **Category**     | e2e |

---

## 3. `sync` — Retention & Rotation

### TC-ROT-001 — Hourly rotation within retention limit

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ROT-001 |
| **Test Name**    | Hourly snapshots rotate correctly within limits |
| **Preconditions** | Retention: `hourly: 6`. Currently have hourly.0 through hourly.3 (4 snapshots). |
| **Steps**        | 1. Run `incr-backup sync --level hourly` |
| **Expected Result** | - hourly.3 → hourly.4 <br>- hourly.2 → hourly.3 <br>- hourly.1 → hourly.2 <br>- hourly.0 → hourly.1 <br>- New sync creates hourly.0. <br>- Total: 5 hourly snapshots. |
| **Category**     | integration |

---

### TC-ROT-002 — Hourly rotation at retention limit (oldest deleted)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ROT-002 |
| **Test Name**    | Oldest hourly deleted when retention limit reached |
| **Preconditions** | Retention: `hourly: 6`. Currently have hourly.0 through hourly.5 (6 snapshots = full). |
| **Steps**        | 1. Run `incr-backup sync --level hourly` |
| **Expected Result** | - hourly.5 is DELETED (oldest, exceeds retention). <br>- hourly.4 → hourly.5 <br>- … <br>- hourly.0 → hourly.1 <br>- New hourly.0 created. <br>- Still 6 hourly snapshots total. |
| **Category**     | integration |

---

### TC-ROT-003 — Re-sync within minimum interval (update in place)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ROT-003 |
| **Test Name**    | Re-sync within same hour updates hourly.0 in place |
| **Preconditions** | hourly.0 was created 15 minutes ago. Default minimum interval = 1 hour. |
| **Steps**        | 1. Make changes to source. <br>2. Run `incr-backup sync --level hourly` |
| **Expected Result** | - No rotation occurs (hourly.0 is too recent). <br>- `rclone sync --backup-dir diffs/hourly.0/<timestamp>` re-runs, updating hourly.0's diff content in place. <br>- hourly.0's manifest is regenerated / replaced. <br>- Log message: "hourly.0 is less than 1h old; updating in place without rotation." |
| **Category**     | integration |

---

### TC-ROT-004 — Force rotate within minimum interval

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ROT-004 |
| **Test Name**    | --force-rotate overrides minimum interval |
| **Preconditions** | hourly.0 was created 15 minutes ago. |
| **Steps**        | 1. Run `incr-backup sync --level hourly --force-rotate` |
| **Expected Result** | - Rotation occurs despite hourly.0 being recent. <br>- hourly.0 → hourly.1, new hourly.0 created. <br>- Normal sync proceeds. |
| **Category**     | integration |

---

### TC-ROT-005 — Multiple rapid re-syncs without rotation

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ROT-005 |
| **Test Name**    | Three syncs within one hour all update in place |
| **Preconditions** | hourly.0 was just created. |
| **Steps**        | 1. Modify source, run `incr-backup sync --level hourly`. <br>2. Modify source again, run `incr-backup sync --level hourly`. <br>3. Modify source again, run `incr-backup sync --level hourly`. |
| **Expected Result** | - All three syncs update hourly.0 in place. <br>- No additional hourly.N slots consumed. <br>- `current/` reflects the state after the third sync. <br>- hourly.0 diff contains OLD versions of ALL files changed across all 3 syncs (accumulative within the slot). |
| **Category**     | e2e |

---

### TC-ROT-006 — Sync after minimum interval elapses

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ROT-006 |
| **Test Name**    | Normal rotation resumes after interval elapses |
| **Preconditions** | hourly.0 was created 2 hours ago. Minimum interval = 1 hour. |
| **Steps**        | 1. Run `incr-backup sync --level hourly` |
| **Expected Result** | - Normal rotation occurs (hourly.0 → hourly.1, etc.). <br>- New hourly.0 created with fresh timestamp. |
| **Category**     | integration |

---

## 4. `sync` — Cascade Promotion

### TC-CASC-001 — Daily promotion from hourly

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CASC-001 |
| **Test Name**    | `sync --level daily` promotes oldest hourly to daily.0 |
| **Preconditions** | hourly.0 through hourly.5 exist. daily.0 through daily.2 exist. |
| **Steps**        | 1. Run `incr-backup sync --level daily` |
| **Expected Result** | - daily.2 → daily.3. <br>- daily.1 → daily.2. <br>- daily.0 → daily.1. <br>- hourly.5 (oldest) promoted to daily.0. <br>- Hourly rotation proceeds (hourly.4 → hourly.5, … hourly.0 → hourly.1). <br>- New hourly.0 created with fresh sync. <br>- Weekly/monthly slots are NOT touched. |
| **Category**     | integration |

---

### TC-CASC-002 — Daily promotion deletes oldest daily at retention limit

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CASC-002 |
| **Test Name**    | Oldest daily is deleted when daily retention is full |
| **Preconditions** | Retention: `daily: 7`. daily.0 through daily.6 exist (full). hourly.0 through hourly.5 exist. |
| **Steps**        | 1. Run `incr-backup sync --level daily` |
| **Expected Result** | - daily.6 is DELETED. <br>- daily.5 → daily.6, … daily.0 → daily.1. <br>- hourly.5 → daily.0 (promoted). <br>- Hourly rotation + sync proceeds. |
| **Category**     | integration |

---

### TC-CASC-003 — Weekly promotion cascades through daily and hourly

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CASC-003 |
| **Test Name**    | `sync --level weekly` cascades promotions |
| **Preconditions** | weekly.0, weekly.1 exist. daily.0–daily.6 exist. hourly.0–hourly.5 exist. |
| **Steps**        | 1. Run `incr-backup sync --level weekly` |
| **Expected Result** | - weekly.1 → weekly.2. <br>- weekly.0 → weekly.1. <br>- daily.6 (oldest daily) → weekly.0 (promoted). <br>- daily.5 → daily.6, … daily.0 → daily.1. <br>- hourly.5 → daily.0 (promoted). <br>- Hourly rotation + sync. <br>- Monthly NOT touched. |
| **Category**     | integration |

---

### TC-CASC-004 — Monthly promotion cascades through all levels

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CASC-004 |
| **Test Name**    | `sync --level monthly` full cascade |
| **Preconditions** | All levels have snapshots. |
| **Steps**        | 1. Run `incr-backup sync --level monthly` |
| **Expected Result** | - Oldest weekly → monthly.0 (with rotation of monthlies). <br>- Oldest daily → weekly.0 (with rotation of weeklies). <br>- Oldest hourly → daily.0 (with rotation of dailies). <br>- Hourly rotation + sync. |
| **Category**     | integration |

---

### TC-CASC-005 — Daily promotion when no hourlies exist

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CASC-005 |
| **Test Name**    | Daily sync with zero hourly snapshots |
| **Preconditions** | No hourly snapshots exist. Only `current/` from init. |
| **Steps**        | 1. Run `incr-backup sync --level daily` |
| **Expected Result** | - No promotion into daily (nothing to promote). <br>- Proceeds with hourly rotation and sync normally. <br>- Creates hourly.0. <br>- Warning or info: "No hourly snapshots available for promotion to daily." |
| **Category**     | integration |

---

### TC-CASC-006 — Weekly promotion when no dailies exist

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CASC-006 |
| **Test Name**    | Weekly sync with zero daily snapshots |
| **Preconditions** | Hourly snapshots exist, but no daily snapshots. |
| **Steps**        | 1. Run `incr-backup sync --level weekly` |
| **Expected Result** | - No promotion into weekly (no dailies to promote). <br>- Daily promotion from hourly proceeds if hourlies exist. <br>- Hourly rotation + sync. <br>- Warning: "No daily snapshots available for promotion to weekly." |
| **Category**     | integration |

---

## 5. `list` Command

### TC-LIST-001 — List all snapshots

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-LIST-001 |
| **Test Name**    | List displays all retained snapshots |
| **Preconditions** | Multiple snapshots at various levels: hourly.0–2, daily.0–1, weekly.0, monthly.0. |
| **Steps**        | 1. Run `incr-backup list` |
| **Expected Result** | - Output shows all snapshots grouped by level. <br>- Each entry shows: slot name, timestamp, number of files, total size. <br>- Order: most recent first within each level. <br>- `current` is listed as a special entry at the top. |
| **Category**     | e2e |

---

### TC-LIST-002 — List when no snapshots exist

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-LIST-002 |
| **Test Name**    | List with freshly initialized namespace (no syncs) |
| **Preconditions** | `init` completed, no `sync` run yet. |
| **Steps**        | 1. Run `incr-backup list` |
| **Expected Result** | - Output shows only `current` (empty or from adopt). <br>- Message: "No snapshots. Run `incr-backup sync` to create one." |
| **Category**     | e2e |

---

### TC-LIST-003 — List with --json output

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-LIST-003 |
| **Test Name**    | JSON-formatted list output |
| **Preconditions** | Snapshots exist at multiple levels. |
| **Steps**        | 1. Run `incr-backup list --json` |
| **Expected Result** | - Valid JSON array output. <br>- Each object has `level`, `slot`, `timestamp`, `file_count`, `total_size`. <br>- Parseable by `jq`. |
| **Category**     | e2e |

---

## 6. `status` Command

### TC-STAT-001 — Status shows space usage

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-STAT-001 |
| **Test Name**    | Status displays remote space usage breakdown |
| **Preconditions** | Multiple syncs completed with data in `current/` and `diffs/`. |
| **Steps**        | 1. Run `incr-backup status` |
| **Expected Result** | - Output shows: <br>  - Total remote usage. <br>  - `current/` size. <br>  - `diffs/` size (broken down by level). <br>  - `.meta/` size. <br>  - Retention policy summary. <br>  - Last sync time. <br>  - Lock status (locked/unlocked). |
| **Category**     | e2e |

---

### TC-STAT-002 — Status detects stale lock

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-STAT-002 |
| **Test Name**    | Status warns about stale lock |
| **Preconditions** | A lock file exists that is older than the timeout (default 1 hour). |
| **Steps**        | 1. Run `incr-backup status` |
| **Expected Result** | - Output includes: "⚠ WARNING: Stale lock detected (age: Xh Ym). May indicate a crashed operation. Use `sync --force` to break." <br>- Suggests checking journal for interrupted operation. |
| **Category**     | e2e |

---

### TC-STAT-003 — Health check with manifest integrity

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-STAT-003 |
| **Test Name**    | Status performs quick health check |
| **Preconditions** | Initialized namespace with syncs. |
| **Steps**        | 1. Run `incr-backup status --health` |
| **Expected Result** | - Verifies `config.json` is readable and valid. <br>- Verifies latest manifest exists and is valid JSON. <br>- Checks that `current/` file count matches manifest entry count. <br>- Reports: "Health: OK" or lists discrepancies. |
| **Category**     | e2e |

---

## 7. `restore` Command

### TC-REST-001 — Restore current snapshot

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REST-001 |
| **Test Name**    | Restore `current` (trivial copy) |
| **Preconditions** | Multiple syncs completed. Local target dir `/tmp/restore` is empty. |
| **Steps**        | 1. Run `incr-backup restore current --to /tmp/restore` |
| **Expected Result** | - Effectively `rclone copy gdrive:backups/myhost/current/ /tmp/restore`. <br>- All files from `current/` appear in `/tmp/restore`. <br>- No diff resolution needed. <br>- Exit code 0. |
| **Category**     | e2e |

---

### TC-REST-002 — Restore a historical hourly snapshot

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REST-002 |
| **Test Name**    | Restore hourly.2 from diffs |
| **Preconditions** | hourly.0 through hourly.4 exist. `/tmp/restore` is empty. |
| **Steps**        | 1. Run `incr-backup restore hourly.2 --to /tmp/restore` |
| **Expected Result** | - Tool reads hourly.2's manifest to get the file list. <br>- For each file: looks in the earliest diff directory AFTER hourly.2 (i.e., hourly.1, hourly.0) for old versions, or uses `current/` if unchanged. <br>- All files downloaded to `/tmp/restore` matching hourly.2's state. <br>- md5 hashes of restored files match manifest entries. |
| **Category**     | e2e |

---

### TC-REST-003 — Restore oldest monthly snapshot

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REST-003 |
| **Test Name**    | Restore the oldest available snapshot |
| **Preconditions** | monthly.11 is the oldest snapshot. |
| **Steps**        | 1. Run `incr-backup restore monthly.11 --to /tmp/restore` |
| **Expected Result** | - File resolution searches through all diff directories from monthly.11 forward to current. <br>- Complete restoration of monthly.11's state. <br>- All hashes validated. |
| **Category**     | e2e |

---

### TC-REST-004 — Restore with --path filter (single file)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REST-004 |
| **Test Name**    | Restore a single file from a snapshot |
| **Preconditions** | hourly.1 exists with manifest containing `docs/report.pdf`. |
| **Steps**        | 1. Run `incr-backup restore hourly.1 --to /tmp/restore --path docs/report.pdf` |
| **Expected Result** | - Only `docs/report.pdf` is restored to `/tmp/restore/docs/report.pdf`. <br>- No other files downloaded. <br>- Hash matches manifest. |
| **Category**     | e2e |

---

### TC-REST-005 — Restore with --path filter (subdirectory)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REST-005 |
| **Test Name**    | Restore an entire subdirectory from a snapshot |
| **Preconditions** | hourly.1 manifest contains files under `photos/2026/`. |
| **Steps**        | 1. Run `incr-backup restore hourly.1 --to /tmp/restore --path photos/2026/` |
| **Expected Result** | - All files under `photos/2026/` in that snapshot are restored. <br>- Directory structure preserved. <br>- Files outside `photos/2026/` are not downloaded. |
| **Category**     | e2e |

---

### TC-REST-006 — Restore to non-empty directory

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REST-006 |
| **Test Name**    | Restore refuses to overwrite existing files without --overwrite |
| **Preconditions** | `/tmp/restore` already contains files. |
| **Steps**        | 1. Run `incr-backup restore hourly.0 --to /tmp/restore` |
| **Expected Result** | - Exit code non-zero. <br>- Error: "Target directory `/tmp/restore` is not empty. Use `--overwrite` to replace existing files." |
| **Category**     | e2e |

---

### TC-REST-007 — Restore non-existent snapshot

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REST-007 |
| **Test Name**    | Restore a snapshot that doesn't exist |
| **Preconditions** | Only hourly.0–hourly.2 exist. |
| **Steps**        | 1. Run `incr-backup restore hourly.5 --to /tmp/restore` |
| **Expected Result** | - Exit code non-zero. <br>- Error: "Snapshot `hourly.5` does not exist. Run `incr-backup list` to see available snapshots." |
| **Category**     | e2e |

---

### TC-REST-008 — Restore file resolution: file in diff vs current

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REST-008 |
| **Test Name**    | File resolution correctly chooses diff or current source |
| **Preconditions** | - hourly.2 manifest lists `file_A.txt` (md5 = `aaa`). <br>- `file_A.txt` was modified after hourly.2, so its old version (md5 = `aaa`) is in `diffs/hourly.1/`. <br>- `current/file_A.txt` has md5 = `bbb` (newer). <br>- hourly.2 manifest also lists `file_B.txt` (md5 = `ccc`), which hasn't changed — it's in `current/` with md5 = `ccc`. |
| **Steps**        | 1. Run `incr-backup restore hourly.2 --to /tmp/restore` |
| **Expected Result** | - `file_A.txt` is sourced from `diffs/hourly.1/` (md5 `aaa`). <br>- `file_B.txt` is sourced from `current/` (md5 `ccc`). <br>- Both files match hourly.2 manifest hashes. |
| **Category**     | integration |

---

## 8. `diff` Command

### TC-DIFF-001 — Diff between two adjacent hourly snapshots

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-DIFF-001 |
| **Test Name**    | Diff shows changes between hourly.0 and hourly.1 |
| **Preconditions** | hourly.0 and hourly.1 exist with different manifests. |
| **Steps**        | 1. Run `incr-backup diff hourly.1 hourly.0` |
| **Expected Result** | - Output lists: <br>  - **Added** files (in hourly.0 but not hourly.1). <br>  - **Deleted** files (in hourly.1 but not hourly.0). <br>  - **Modified** files (same path, different md5/size). <br>- Counts displayed: "3 added, 1 deleted, 2 modified." |
| **Category**     | e2e |

---

### TC-DIFF-002 — Diff between current and itself

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-DIFF-002 |
| **Test Name**    | Diff of a snapshot with itself is empty |
| **Preconditions** | hourly.0 exists. |
| **Steps**        | 1. Run `incr-backup diff hourly.0 hourly.0` |
| **Expected Result** | - Output: "No differences." <br>- Zero added, deleted, or modified files. |
| **Category**     | unit |

---

### TC-DIFF-003 — Diff between different levels

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-DIFF-003 |
| **Test Name**    | Diff between a daily and a weekly snapshot |
| **Preconditions** | daily.0 and weekly.0 both exist with manifests. |
| **Steps**        | 1. Run `incr-backup diff weekly.0 daily.0` |
| **Expected Result** | - Correctly compares manifests regardless of level. <br>- Shows all added/deleted/modified files between the two points in time. |
| **Category**     | e2e |

---

### TC-DIFF-004 — Diff with current

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-DIFF-004 |
| **Test Name**    | Diff a historical snapshot against current |
| **Preconditions** | hourly.2 and current (latest manifest) exist. |
| **Steps**        | 1. Run `incr-backup diff hourly.2 current` |
| **Expected Result** | - Shows all changes between hourly.2 and the current state. <br>- Useful for seeing "what changed since that snapshot." |
| **Category**     | e2e |

---

### TC-DIFF-005 — Diff with --json output

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-DIFF-005 |
| **Test Name**    | JSON-formatted diff output |
| **Preconditions** | Two snapshots exist. |
| **Steps**        | 1. Run `incr-backup diff hourly.1 hourly.0 --json` |
| **Expected Result** | - Valid JSON with `added`, `deleted`, `modified` arrays. <br>- Each entry has `path`, `size_a`, `size_b`, `md5_a`, `md5_b` (where applicable). |
| **Category**     | e2e |

---

### TC-DIFF-006 — Diff with non-existent snapshot

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-DIFF-006 |
| **Test Name**    | Diff references a snapshot that doesn't exist |
| **Preconditions** | hourly.0 exists, weekly.5 does not. |
| **Steps**        | 1. Run `incr-backup diff hourly.0 weekly.5` |
| **Expected Result** | - Exit code non-zero. <br>- Error: "Snapshot `weekly.5` does not exist." |
| **Category**     | e2e |

---

## 9. `verify` Command

### TC-VERI-001 — Verify current snapshot (all files match)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-VERI-001 |
| **Test Name**    | Verify current with all files intact |
| **Preconditions** | Latest sync completed successfully. All files on remote are intact. |
| **Steps**        | 1. Run `incr-backup verify current` |
| **Expected Result** | - For each file in the latest manifest, checks existence on remote and verifies md5 hash. <br>- Output: "Verified N files. All OK." <br>- Exit code 0. |
| **Category**     | e2e |

---

### TC-VERI-002 — Verify detects missing file

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-VERI-002 |
| **Test Name**    | Verify catches a file deleted from remote outside of tool |
| **Preconditions** | Manually delete a file from `current/` on remote (outside of incr-backup). |
| **Steps**        | 1. Run `incr-backup verify current` |
| **Expected Result** | - Error reported: "MISSING: path/to/deleted_file.txt" <br>- Exit code non-zero. <br>- Summary: "Verified N files. 1 error(s): 1 missing." |
| **Category**     | e2e |

---

### TC-VERI-003 — Verify detects corrupted file (hash mismatch)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-VERI-003 |
| **Test Name**    | Verify catches file with wrong hash |
| **Preconditions** | Modify a file on remote directly (changing its content/hash) without going through incr-backup. |
| **Steps**        | 1. Run `incr-backup verify current` |
| **Expected Result** | - Error: "HASH MISMATCH: path/to/file.txt (expected: abc123, got: def456)" <br>- Exit code non-zero. |
| **Category**     | e2e |

---

### TC-VERI-004 — Verify a historical snapshot

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-VERI-004 |
| **Test Name**    | Verify hourly.2 checks files across diffs and current |
| **Preconditions** | hourly.2 exists. Its manifest references files in diffs/ and current/. |
| **Steps**        | 1. Run `incr-backup verify hourly.2` |
| **Expected Result** | - Resolves file locations (same logic as restore). <br>- Checks existence and hash for each file. <br>- Reports results. |
| **Category**     | e2e |

---

### TC-VERI-005 — Verify all snapshots

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-VERI-005 |
| **Test Name**    | Verify without specifying a snapshot checks everything |
| **Preconditions** | Multiple snapshots at various levels. |
| **Steps**        | 1. Run `incr-backup verify` |
| **Expected Result** | - Iterates over all snapshots and current. <br>- Reports per-snapshot results. <br>- Summary: "Checked M snapshots, N total file checks. X error(s)." |
| **Category**     | e2e |

---

## 10. Locking & Concurrency

### TC-LOCK-001 — Concurrent sync rejected

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-LOCK-001 |
| **Test Name**    | Second sync fails while first is running |
| **Preconditions** | First `incr-backup sync` is currently in progress (lock held). |
| **Steps**        | 1. In a second terminal, run `incr-backup sync --level hourly` |
| **Expected Result** | - Exit code non-zero. <br>- Error: "Another operation is in progress (locked by PID XXXX on host YYYY at TIME). Use `--force` to break the lock if you believe this is stale." <br>- No data corruption. |
| **Category**     | e2e |

---

### TC-LOCK-002 — Break stale lock with --force

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-LOCK-002 |
| **Test Name**    | Force-break a stale lock |
| **Preconditions** | Lock file exists but is older than timeout (1 hour). The process that created it has crashed. |
| **Steps**        | 1. Run `incr-backup sync --level hourly --force` |
| **Expected Result** | - Warning: "Breaking stale lock (age: Xh Ym, created by PID XXXX on host YYYY)." <br>- Old lock removed, new lock acquired. <br>- Checks journal for interrupted operation and attempts resume/cleanup. <br>- Sync proceeds normally. |
| **Category**     | e2e |

---

### TC-LOCK-003 — Lock within timeout not breakable without --force

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-LOCK-003 |
| **Test Name**    | Recent lock cannot be broken without --force |
| **Preconditions** | Lock file exists and is 10 minutes old (within timeout). |
| **Steps**        | 1. Run `incr-backup sync --level hourly` (without --force) |
| **Expected Result** | - Exit code non-zero. <br>- Error includes lock age and suggests waiting or using `--force`. |
| **Category**     | integration |

---

### TC-LOCK-004 — Lock contains identifying information

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-LOCK-004 |
| **Test Name**    | Lock file has PID, hostname, and timestamp |
| **Preconditions** | None. |
| **Steps**        | 1. Start a sync. <br>2. Read `.meta/lock` from remote before sync completes. |
| **Expected Result** | - Lock file is valid JSON with fields: `pid`, `hostname`, `timestamp`, `operation`. <br>- Values match the running process. |
| **Category**     | integration |

---

### TC-LOCK-005 — Custom lock timeout

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-LOCK-005 |
| **Test Name**    | Configurable lock timeout is respected |
| **Preconditions** | Config sets `lock_timeout_minutes: 30`. Lock is 45 minutes old. |
| **Steps**        | 1. Run `incr-backup sync --level hourly --force` |
| **Expected Result** | - Lock is considered stale (45 min > 30 min timeout). <br>- Warning and break proceeds as in TC-LOCK-002. |
| **Category**     | integration |

---

## 11. Journal & Resumability

### TC-JOUR-001 — Journal tracks operation steps

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-JOUR-001 |
| **Test Name**    | Journal records each phase of sync |
| **Preconditions** | None. |
| **Steps**        | 1. Run `incr-backup sync --level hourly`. <br>2. Inspect `.meta/journal.json` during execution (or from logs). |
| **Expected Result** | - Journal entries include: `lock_acquired`, `rotation_started`, `rotation_complete`, `sync_started`, `sync_complete`, `manifest_generated`, `manifest_uploaded`, `lock_released`. <br>- After successful completion, journal is in clean/empty state. |
| **Category**     | integration |

---

### TC-JOUR-002 — Resume after interruption during rotation

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-JOUR-002 |
| **Test Name**    | Crash during rotation phase, resume on next run |
| **Preconditions** | Simulate crash after `rotation_started` but before `rotation_complete` (e.g., kill process). |
| **Steps**        | 1. Force-start a new sync with `--force` (breaking stale lock). |
| **Expected Result** | - Tool reads journal, sees incomplete rotation. <br>- Either rolls back the partial rotation or completes it. <br>- Proceeds with full sync. <br>- No duplicate or lost snapshots. |
| **Category**     | e2e |

---

### TC-JOUR-003 — Resume after interruption during rclone sync

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-JOUR-003 |
| **Test Name**    | Crash during rclone sync phase |
| **Preconditions** | Simulate crash during `rclone sync` (mid-transfer). Journal shows `sync_started`. |
| **Steps**        | 1. Run `incr-backup sync --level hourly --force` |
| **Expected Result** | - Tool reads journal, sees sync was interrupted. <br>- Re-runs `rclone sync` from scratch (rclone handles its own idempotency). <br>- Manifest generated from final state. <br>- No data loss. |
| **Category**     | e2e |

---

### TC-JOUR-004 — Resume after interruption during manifest upload

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-JOUR-004 |
| **Test Name**    | Crash after manifest generated but before upload |
| **Preconditions** | Journal shows `manifest_generated` but not `manifest_uploaded`. |
| **Steps**        | 1. Run `incr-backup sync --level hourly --force` |
| **Expected Result** | - Tool re-generates manifest (or re-uploads the generated one). <br>- Does not re-run rotation or sync. <br>- Completes cleanly. |
| **Category**     | e2e |

---

### TC-JOUR-005 — Clean journal after successful operation

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-JOUR-005 |
| **Test Name**    | Journal is clean after successful sync |
| **Preconditions** | None. |
| **Steps**        | 1. Run `incr-backup sync --level hourly`. <br>2. Read `.meta/journal.json`. |
| **Expected Result** | - Journal contains no in-progress operations. <br>- State is `idle` or file is empty/minimal. |
| **Category**     | integration |

---

## 12. Error Handling & Edge Cases

### TC-ERR-001 — Source directory does not exist at sync time

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ERR-001 |
| **Test Name**    | Sync when source directory has been removed |
| **Preconditions** | Initialized namespace. Source directory `/home/user/docs` no longer exists. |
| **Steps**        | 1. Run `incr-backup sync --level hourly` |
| **Expected Result** | - Exit code non-zero. <br>- Error: "Source directory `/home/user/docs` does not exist." <br>- No lock left behind. No partial state. |
| **Category**     | e2e |

---

### TC-ERR-002 — Manifest corruption (invalid JSON)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ERR-002 |
| **Test Name**    | Tool handles corrupted manifest gracefully |
| **Preconditions** | Manually corrupt a manifest file on remote (write invalid JSON). |
| **Steps**        | 1. Run `incr-backup verify` <br>2. Run `incr-backup restore hourly.0 --to /tmp/restore` (if hourly.0's manifest is corrupt) |
| **Expected Result** | - `verify`: Reports "Manifest for hourly.0 is corrupted: invalid JSON at byte X." <br>- `restore`: Fails with clear error rather than panic. <br>- Suggests: "Run `incr-backup sync` to regenerate the current manifest." |
| **Category**     | integration |

---

### TC-ERR-003 — Config corruption

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ERR-003 |
| **Test Name**    | Corrupted config.json on remote |
| **Preconditions** | `.meta/config.json` contains invalid JSON. |
| **Steps**        | 1. Run any command (sync, list, status, etc.) |
| **Expected Result** | - Exit code non-zero. <br>- Error: "Failed to read configuration: invalid JSON in .meta/config.json." <br>- Suggests re-running `init --force`. |
| **Category**     | integration |

---

### TC-ERR-004 — rclone not found in PATH

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ERR-004 |
| **Test Name**    | rclone binary not available |
| **Preconditions** | rclone is not installed or not in PATH. |
| **Steps**        | 1. Run any `incr-backup` command |
| **Expected Result** | - Exit code non-zero. <br>- Error: "rclone not found. Please install rclone and ensure it is in your PATH." |
| **Category**     | unit |

---

### TC-ERR-005 — rclone command fails with non-zero exit

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ERR-005 |
| **Test Name**    | rclone subprocess failure is handled |
| **Preconditions** | Simulate rclone failure (e.g., auth error during sync). |
| **Steps**        | 1. Run `incr-backup sync --level hourly` |
| **Expected Result** | - incr-backup captures rclone's stderr. <br>- Error: "rclone sync failed (exit code X): <rclone stderr>". <br>- Journal records the failure. Lock is released. <br>- No manifest generated (sync didn't complete). |
| **Category**     | integration |

---

### TC-ERR-006 — Remote quota exceeded (disk full)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ERR-006 |
| **Test Name**    | Sync when Google Drive storage is full |
| **Preconditions** | Google Drive account is at storage quota. |
| **Steps**        | 1. Add new files to source. <br>2. Run `incr-backup sync --level hourly` |
| **Expected Result** | - rclone fails with quota error. <br>- incr-backup reports: "Remote storage quota exceeded. Free up space or upgrade storage." <br>- Lock released. Journal reflects failure. |
| **Category**     | e2e |

---

### TC-ERR-007 — Local disk full during restore

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ERR-007 |
| **Test Name**    | Restore when local disk is full |
| **Preconditions** | Local filesystem where restore target resides is at capacity. |
| **Steps**        | 1. Run `incr-backup restore current --to /tmp/restore` |
| **Expected Result** | - rclone copy fails with "no space left on device" or similar. <br>- incr-backup reports the error clearly. <br>- Partially restored files may remain (user's responsibility to clean up). |
| **Category**     | e2e |

---

### TC-ERR-008 — Clock skew between runs

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ERR-008 |
| **Test Name**    | System clock jumps backward between syncs |
| **Preconditions** | Last sync timestamp is T. System clock is set to T - 2 hours. |
| **Steps**        | 1. Run `incr-backup sync --level hourly` |
| **Expected Result** | - Warning: "System clock appears to have moved backward. Last sync was at T, current time is T-2h." <br>- Sync proceeds (using current system time for the new snapshot). <br>- Minimum interval check handles this gracefully (does not skip rotation incorrectly). |
| **Category**     | integration |

---

## 13. Configuration & Environment

### TC-CONF-001 — Config override via environment variable

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CONF-001 |
| **Test Name**    | Override retention via env var |
| **Preconditions** | Config has `hourly: 6`. |
| **Steps**        | 1. Run `INCR_BACKUP_HOURLY=12 incr-backup sync --level hourly` |
| **Expected Result** | - Hourly retention of 12 is used for this run. <br>- Config file on remote is NOT modified (env override is transient). |
| **Category**     | integration |

---

### TC-CONF-002 — Override remote path via env var

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CONF-002 |
| **Test Name**    | Override remote via INCR_BACKUP_REMOTE |
| **Preconditions** | Initialized namespace. |
| **Steps**        | 1. Run `INCR_BACKUP_REMOTE=gdrive:other/path incr-backup list` |
| **Expected Result** | - Tool uses `gdrive:other/path` instead of configured remote. <br>- Lists snapshots from that location (or errors if it doesn't exist). |
| **Category**     | integration |

---

### TC-CONF-003 — Override rclone binary path

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CONF-003 |
| **Test Name**    | Custom rclone binary path |
| **Preconditions** | rclone installed at `/opt/rclone/rclone`. |
| **Steps**        | 1. Run `INCR_BACKUP_RCLONE=/opt/rclone/rclone incr-backup sync --level hourly` |
| **Expected Result** | - Tool uses the specified rclone binary. <br>- Sync succeeds if the binary is valid. |
| **Category**     | integration |

---

### TC-CONF-004 — Config file location respects XDG

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CONF-004 |
| **Test Name**    | Local config respects XDG_CONFIG_HOME |
| **Preconditions** | `XDG_CONFIG_HOME=/custom/config` is set. |
| **Steps**        | 1. Run `incr-backup init --remote gdrive:backups/myhost --source /home/user/docs` |
| **Expected Result** | - Local config written to `/custom/config/incr-backup/` instead of `~/.config/incr-backup/`. |
| **Category**     | unit |

---

### TC-CONF-005 — Override lock timeout via config

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CONF-005 |
| **Test Name**    | Custom lock timeout in config |
| **Preconditions** | Config has `lock_timeout_minutes: 15`. |
| **Steps**        | 1. Create a lock that is 20 minutes old. <br>2. Run `incr-backup sync --level hourly --force` |
| **Expected Result** | - Lock is considered stale at 20 min (> 15 min configured timeout). <br>- Lock broken and sync proceeds. |
| **Category**     | integration |

---

### TC-CONF-006 — Override minimum sync interval

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CONF-006 |
| **Test Name**    | Custom minimum interval between rotations |
| **Preconditions** | Config has `min_interval_minutes: 30`. hourly.0 is 35 minutes old. |
| **Steps**        | 1. Run `incr-backup sync --level hourly` |
| **Expected Result** | - Rotation occurs (35 min > 30 min minimum). <br>- With default (60 min), this would have been an in-place update. |
| **Category**     | integration |

---

### TC-CONF-007 — Timezone-aware snapshot naming via CLI and config

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CONF-007 |
| **Test Name**    | Timezone indicator in snapshot directory names (UTC vs Local) |
| **Preconditions** | Initialized backup repository. |
| **Steps**        | 1. Run `incr-backup sync --level hourly --timezone local` <br>2. Run `incr-backup sync --level hourly --tz utc --force-rotate` |
| **Expected Result** | - Step 1 creates snapshot ending with numeric offset `+HHMM` or `-HHMM` without colons `:` <br>- Step 2 creates snapshot ending with `Z` (UTC designator). |
| **Category**     | integration |

---

### TC-CONF-008 — Robust TOML timezone parsing across sections and unquoted values

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CONF-008 |
| **Test Name**    | TOML configuration timezone parsing flexibility |
| **Preconditions** | Valid source directory and remote path. |
| **Steps**        | 1. Test `timezone = "local"` at root level. <br>2. Test `timezone = "local"` after `[remote]` or `[retention]` table headers. <br>3. Test `[snapshot]` section with `timezone = "local"`. <br>4. Test unquoted `timezone = local` and `timezone=local`. |
| **Expected Result** | - In all cases, `Config::load` parses `timezone` as `SnapshotTimezone::Local` without failing TOML syntax validation or defaulting to UTC. |
| **Category**     | unit, integration |

---

## 14. Filesystem Edge Cases

### TC-FS-001 — Files with spaces in names

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-001 |
| **Test Name**    | Sync handles filenames with spaces |
| **Preconditions** | Source contains `my document (final).docx`. |
| **Steps**        | 1. Run `incr-backup sync --level hourly` <br>2. Run `incr-backup verify current` |
| **Expected Result** | - File synced to `current/my document (final).docx`. <br>- Manifest entry path preserved exactly. <br>- Verify confirms hash match. |
| **Category**     | e2e |

---

### TC-FS-002 — Files with unicode characters

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-002 |
| **Test Name**    | Sync handles unicode filenames |
| **Preconditions** | Source contains `日本語ファイル.txt`, `café_résumé.pdf`, `файл_документ.doc`. |
| **Steps**        | 1. Run `incr-backup sync --level hourly` <br>2. Run `incr-backup verify current` <br>3. Run `incr-backup restore current --to /tmp/restore` |
| **Expected Result** | - All files synced with correct names. <br>- Manifest correctly encodes unicode paths (UTF-8). <br>- Restore produces correct filenames. |
| **Category**     | e2e |

---

### TC-FS-003 — Files with special characters (brackets, hashes, etc.)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-003 |
| **Test Name**    | Filenames with characters special to shells/GDrive |
| **Preconditions** | Source contains files named: `file#1.txt`, `report[2026].pdf`, `notes & ideas.md`, `100%.txt`. |
| **Steps**        | 1. Run `incr-backup sync --level hourly` |
| **Expected Result** | - All files synced correctly. <br>- rclone subprocess arguments properly escaped. <br>- Manifest paths are exact. |
| **Category**     | e2e |

---

### TC-FS-004 — Symlinks in source

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-004 |
| **Test Name**    | Symlinks handled according to rclone behavior |
| **Preconditions** | Source contains a symlink `link.txt → ../other/real.txt`. |
| **Steps**        | 1. Run `incr-backup sync --level hourly` |
| **Expected Result** | - Follows rclone's default behavior (typically follows symlinks and copies the target file). <br>- If rclone skips symlinks, incr-backup logs a warning. <br>- No crashes or panics. |
| **Category**     | e2e |

---

### TC-FS-005 — Permission-denied files in source

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-005 |
| **Test Name**    | Unreadable files in source |
| **Preconditions** | Source contains a file with `chmod 000` (no read permission). |
| **Steps**        | 1. Run `incr-backup sync --level hourly` |
| **Expected Result** | - rclone reports permission denied for that file. <br>- incr-backup logs the error but continues syncing other files (rclone's `--ignore-errors` or similar). <br>- Exit code indicates partial failure. <br>- Manifest does not include the unreadable file (or marks it as errored). |
| **Category**     | e2e |

---

### TC-FS-006 — Very large file (>5GB, chunked upload)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-006 |
| **Test Name**    | Sync a file larger than Google Drive's chunked upload threshold |
| **Preconditions** | Source contains a 6GB file. |
| **Steps**        | 1. Run `incr-backup sync --level hourly` |
| **Expected Result** | - rclone handles chunked upload automatically. <br>- File appears in `current/` with correct md5. <br>- Manifest records correct size (> 5GB). <br>- If interrupted mid-upload, journal enables resume. |
| **Category**     | e2e |

---

### TC-FS-007 — Empty file (0 bytes)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-007 |
| **Test Name**    | Sync handles zero-byte files |
| **Preconditions** | Source contains `empty.txt` (0 bytes). |
| **Steps**        | 1. Run `incr-backup sync --level hourly` |
| **Expected Result** | - File synced to `current/`. <br>- Manifest entry has `size: 0` and valid md5 (`d41d8cd98f00b204e9800998ecf8427e`). |
| **Category**     | e2e |

---

### TC-FS-008 — Deeply nested directory structure

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-008 |
| **Test Name**    | Sync handles deeply nested paths |
| **Preconditions** | Source has a file at depth 20: `a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q/r/s/t/file.txt`. |
| **Steps**        | 1. Run `incr-backup sync --level hourly` <br>2. Run `incr-backup restore current --to /tmp/restore` |
| **Expected Result** | - Full path preserved in `current/` and manifest. <br>- Restore recreates the full directory hierarchy. |
| **Category**     | e2e |

---

### TC-FS-009 — Very long filename

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-009 |
| **Test Name**    | File with name near filesystem/GDrive limits |
| **Preconditions** | Source has a file with a 200+ character filename. |
| **Steps**        | 1. Run `incr-backup sync --level hourly` |
| **Expected Result** | - If rclone can handle it, file synced normally. <br>- If GDrive rejects it, error reported clearly. <br>- No panic or data corruption. |
| **Category**     | e2e |

---

### TC-FS-010 — Thousands of small files

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-010 |
| **Test Name**    | Performance with many small files |
| **Preconditions** | Source contains 50,000 small files (1KB each). |
| **Steps**        | 1. Run `incr-backup sync --level hourly` <br>2. Run `incr-backup verify current` |
| **Expected Result** | - Sync completes successfully (may take time). <br>- Manifest has 50,000 entries and is valid. <br>- Verify completes without error. <br>- `rclone lsjson --recursive` handles the volume. |
| **Category**     | e2e |

---

### TC-FS-011 — Hidden files (dotfiles)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-011 |
| **Test Name**    | Sync includes hidden files |
| **Preconditions** | Source contains `.gitignore`, `.env`, `.ssh/config`. |
| **Steps**        | 1. Run `incr-backup sync --level hourly` |
| **Expected Result** | - Hidden files and directories synced to `current/`. <br>- Manifest includes dotfile entries. |
| **Category**     | e2e |

---

## 15. Network & Remote Failures

### TC-NET-001 — Network interruption during rotation

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-NET-001 |
| **Test Name**    | Network drops during snapshot rotation phase |
| **Preconditions** | Simulate network failure (e.g., disconnect WiFi) during rotation. |
| **Steps**        | 1. Start `incr-backup sync --level hourly`. <br>2. Kill network during rotation. <br>3. Restore network. <br>4. Run `incr-backup sync --level hourly --force`. |
| **Expected Result** | - First run fails with network error. Lock and journal left on remote. <br>- Second run (with --force) reads journal, detects incomplete rotation. <br>- Cleanup/resume succeeds. <br>- Final state is consistent. |
| **Category**     | e2e |

---

### TC-NET-002 — Network interruption during rclone sync

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-NET-002 |
| **Test Name**    | Network drops during file transfer |
| **Preconditions** | Large sync in progress. |
| **Steps**        | 1. Start sync. <br>2. Kill network mid-transfer. <br>3. Restore network. <br>4. Run sync again with --force. |
| **Expected Result** | - Partial files may exist on remote (rclone may leave temp files). <br>- Journal shows `sync_started`. <br>- Re-sync re-runs rclone which handles its own resume/cleanup. <br>- Final state correct. |
| **Category**     | e2e |

---

### TC-NET-003 — Network interruption during manifest upload

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-NET-003 |
| **Test Name**    | Network drops during manifest upload |
| **Preconditions** | Sync completed but manifest upload in progress. |
| **Steps**        | 1. Simulate failure during manifest upload. <br>2. Resume with `--force`. |
| **Expected Result** | - Journal shows `manifest_generated` but not `manifest_uploaded`. <br>- Re-run regenerates or re-uploads manifest. <br>- `current/` state is correct from completed sync. |
| **Category**     | e2e |

---

### TC-NET-004 — Intermittent network (retries)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-NET-004 |
| **Test Name**    | Transient network errors are retried |
| **Preconditions** | Network is flaky (packet loss). |
| **Steps**        | 1. Run `incr-backup sync --level hourly` with `--retries 5` (or rclone's default retries). |
| **Expected Result** | - rclone retries failed transfers automatically. <br>- incr-backup does not abort on first rclone retry. <br>- Sync eventually completes if network stabilizes. |
| **Category**     | e2e |

---

### TC-NET-005 — GDrive rate limiting (HTTP 429)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-NET-005 |
| **Test Name**    | Google Drive API rate limits are handled |
| **Preconditions** | High volume of API calls triggers rate limiting. |
| **Steps**        | 1. Sync a large number of files rapidly. |
| **Expected Result** | - rclone's built-in rate limit handling (backoff/retry) kicks in. <br>- incr-backup does not treat rate-limited responses as fatal. <br>- Sync completes (slowly). |
| **Category**     | e2e |

---

## 16. Multi-Source & Namespace Isolation

### TC-NS-001 — Two sources on same remote (independent namespaces)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-NS-001 |
| **Test Name**    | Independent backups coexist on same remote |
| **Preconditions** | Two namespaces initialized: `gdrive:backups/host_a` and `gdrive:backups/host_b`. |
| **Steps**        | 1. Run `incr-backup sync --level hourly` for host_a. <br>2. Run `incr-backup sync --level hourly` for host_b. |
| **Expected Result** | - Each namespace has independent `current/`, `diffs/`, `.meta/`. <br>- Operations on host_a do not affect host_b and vice versa. <br>- Locks are per-namespace. <br>- Manifests are independent. |
| **Category**     | e2e |

---

### TC-NS-002 — Namespace collision detection

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-NS-002 |
| **Test Name**    | Init prevents overlapping namespaces |
| **Preconditions** | `gdrive:backups/host` already initialized. |
| **Steps**        | 1. Run `incr-backup init --remote gdrive:backups/host --source /other/dir` |
| **Expected Result** | - Error: "Namespace already initialized." <br>- No data corruption. |
| **Category**     | e2e |

---

### TC-NS-003 — Concurrent syncs on different namespaces

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-NS-003 |
| **Test Name**    | Simultaneous syncs on independent namespaces |
| **Preconditions** | Two independent namespaces. |
| **Steps**        | 1. Run sync for namespace A and namespace B simultaneously. |
| **Expected Result** | - Both syncs complete independently. <br>- Locks do not conflict (different lock paths). <br>- No data crossover. |
| **Category**     | e2e |

---

### TC-NS-004 — List only shows specified namespace

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-NS-004 |
| **Test Name**    | List command is namespace-scoped |
| **Preconditions** | Two namespaces with different snapshot counts. |
| **Steps**        | 1. Run `incr-backup list` for namespace A. |
| **Expected Result** | - Only namespace A's snapshots displayed. <br>- No leakage from namespace B. |
| **Category**     | e2e |

---

## 17. `remove` Command — History Purging

### TC-REM-001 — Purge single file from all snapshots and manifests

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REM-001 |
| **Test Name**    | Purge single file across current and all past diff snapshots and manifests |
| **Preconditions** | Initial backup taken with `secret.txt` and `file1.txt`. Second backup taken where `secret.txt` was modified (creating a historical diff copy in `diffs/hourly.0__.../`). |
| **Steps**        | 1. Run `incr-backup remove secret.txt` |
| **Expected Result** | - Exit code 0. <br>- `current/secret.txt` is deleted. <br>- `secret.txt` is deleted from all directories under `diffs/`. <br>- Every manifest in `.meta/manifests/` is rewritten without `secret.txt`, with updated `file_count` and `total_size`. <br>- `file1.txt` is completely unaffected. <br>- Subsequent `incr-backup verify` passes all checks. |
| **Category**     | e2e |

---

### TC-REM-002 — Purge directory tree from all snapshots

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REM-002 |
| **Test Name**    | Purge entire directory and nested subdirectories from all snapshots |
| **Preconditions** | Backup containing `sensitive/passwords.txt`, `sensitive/sub/key.pem`, and `docs/report.txt`. |
| **Steps**        | 1. Run `incr-backup remove sensitive/` |
| **Expected Result** | - Exit code 0. <br>- `sensitive/` directory is purged from `current/`. <br>- `sensitive/` is purged from all `diffs/*` snapshot directories. <br>- Empty directory structures cleaned up via `rclone rmdirs --leave-root`. <br>- Manifests have zero entries under `sensitive/`. <br>- `docs/report.txt` is intact. |
| **Category**     | e2e |

---

### TC-REM-003 — Dry-run preview of remove operation

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REM-003 |
| **Test Name**    | Verify dry-run mode leaves remote data and manifests untouched |
| **Preconditions** | Backup containing `temp.log`. |
| **Steps**        | 1. Run `incr-backup remove temp.log --dry-run` |
| **Expected Result** | - Exit code 0 with `[DRY-RUN]` output showing which manifests and files would be affected. <br>- Remote storage (`current/temp.log`) and manifests remain completely unmodified. <br>- No advisory lock is created or acquired. |
| **Category**     | integration |

---

### TC-REM-004 — Remove multiple targets with JSON output

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REM-004 |
| **Test Name**    | Remove multiple files simultaneously and verify JSON structure |
| **Preconditions** | Backup containing `leak1.env`, `leak2.pem`, and `doc1.txt`. |
| **Steps**        | 1. Run `incr-backup remove leak1.env leak2.pem --json` |
| **Expected Result** | - Exit code 0. <br>- Valid JSON output parsed with `unique_files_removed: 2`, `snapshots_affected`, and `details`. <br>- Both leaked files deleted across all snapshots and manifests. |
| **Category**     | integration |

---

### TC-REM-005 — Nonexistent target path handling

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REM-005 |
| **Test Name**    | Graceful handling when target file does not exist in any snapshot |
| **Preconditions** | Backup exists without `nonexistent.txt`. |
| **Steps**        | 1. Run `incr-backup remove nonexistent.txt` |
| **Expected Result** | - Exit code 0. <br>- Informative message: "No matching files found across snapshots." <br>- No files or manifests modified. |
| **Category**     | integration |

---

### TC-REM-006 — Root directory removal protection

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REM-006 |
| **Test Name**    | Reject removal of root directory or empty target path |
| **Preconditions** | Backup exists. |
| **Steps**        | 1. Run `incr-backup remove /` or `incr-backup remove .` |
| **Expected Result** | - Exit code non-zero. <br>- Error message rejecting removal of root directory or empty path. <br>- No remote changes executed. |
| **Category**     | unit / integration |

---

## 18. `config` Command — Local & Remote Metadata Sync

### TC-CFG-001 — `config diff` detects differences between local config and remote metadata

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CFG-001 |
| **Test Name**    | Diff local TOML config against remote `.meta/config.json` |
| **Preconditions** | Remote initialized with default retention (6h/7d/4w/6m). Local config edited to minimal (3h/3d/2w/3m). |
| **Steps**        | 1. Run `incr-backup -c my-config.toml config diff` |
| **Expected Result** | - Output displays `Retention: (DIFFER)`. <br>- Highlights local values vs remote values. <br>- Reports `Found 1 difference(s)`. |
| **Category**     | integration |

---

### TC-CFG-002 — `status` and `verify` display warning notices on retention divergence

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CFG-002 |
| **Test Name**    | Retention mismatch warning emitted in status and verify |
| **Preconditions** | Remote `.meta/config.json` has default retention. Local config has minimal retention. |
| **Steps**        | 1. Run `incr-backup -c my-config.toml status`. <br>2. Run `incr-backup -c my-config.toml verify`. |
| **Expected Result** | - `status` output displays remote retention and warning: `Local retention differs from remote metadata. Run incr-backup config push to sync`. <br>- `verify` Check 1 emits warning status `⚠ retention differs from local (...)`. |
| **Category**     | integration |

---

### TC-CFG-003 — `config push` updates `.meta/config.json` under advisory cloud lock

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CFG-003 |
| **Test Name**    | Push local configuration to remote metadata |
| **Preconditions** | Local configuration has updated retention or exclude patterns. |
| **Steps**        | 1. Run `incr-backup -c my-config.toml config push`. <br>2. Run `incr-backup -c my-config.toml config diff`. |
| **Expected Result** | - Remote lock acquired and released. <br>- Remote `.meta/config.json` updated with local settings. <br>- Subsequent `config diff` reports `Local configuration and remote metadata are identical`. |
| **Category**     | integration |

---

### TC-CFG-004 — `config pull` writes remote metadata into local file with overwrite guard

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CFG-004 |
| **Test Name**    | Pull remote configuration into a local TOML file |
| **Preconditions** | Remote `.meta/config.json` exists. Local destination path already exists. |
| **Steps**        | 1. Run `incr-backup config pull --remote <REMOTE> --to existing.toml` without `--overwrite`. <br>2. Re-run with `--overwrite`. |
| **Expected Result** | - Step 1 fails with error requiring `--overwrite`. <br>- Step 2 overwrites local file with valid TOML generated from remote metadata. |
| **Category**     | integration |

---

### TC-CFG-005 — Configuration template validation

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CFG-005 |
| **Test Name**    | Validate syntax and parsing of `config/incr-backup-example.toml` |
| **Preconditions** | File `config/incr-backup-example.toml` exists. |
| **Steps**        | 1. Run unit test `test_example_config_file_parses` loading `config/incr-backup-example.toml`. |
| **Expected Result** | - Successfully parses with `Config::load`. <br>- All fields, presets, comments, and globs verified. |
| **Category**     | unit |

---

## Appendix A: Test Case Summary

| Section | ID Range | Count |
|---------|----------|-------|
| 1. `init` Command | TC-INIT-001 – TC-INIT-014 | 14 |
| 2. `sync` — Core | TC-SYNC-001 – TC-SYNC-010 | 10 |
| 3. `sync` — Rotation | TC-ROT-001 – TC-ROT-006 | 6 |
| 4. `sync` — Cascade | TC-CASC-001 – TC-CASC-006 | 6 |
| 5. `list` Command | TC-LIST-001 – TC-LIST-003 | 3 |
| 6. `status` Command | TC-STAT-001 – TC-STAT-003 | 3 |
| 7. `restore` Command | TC-REST-001 – TC-REST-008 | 8 |
| 8. `diff` Command | TC-DIFF-001 – TC-DIFF-006 | 6 |
| 9. `verify` Command | TC-VERI-001 – TC-VERI-005 | 5 |
| 10. Locking & Concurrency | TC-LOCK-001 – TC-LOCK-005 | 5 |
| 11. Journal & Resumability | TC-JOUR-001 – TC-JOUR-005 | 5 |
| 12. Error Handling | TC-ERR-001 – TC-ERR-008 | 8 |
| 13. Configuration & Env | TC-CONF-001 – TC-CONF-008 | 8 |
| 14. Filesystem Edge Cases | TC-FS-001 – TC-FS-011 | 11 |
| 15. Network & Remote | TC-NET-001 – TC-NET-005 | 5 |
| 16. Multi-Source & Namespace | TC-NS-001 – TC-NS-004 | 4 |
| 17. `remove` Command | TC-REM-001 – TC-REM-006 | 6 |
| 18. `config` Command | TC-CFG-001 – TC-CFG-005 | 5 |
| **Total** | | **114** |

---

## Appendix B: Test Infrastructure Notes

### Recommended Test Environment

- **Unit tests**: Use Rust's built-in `#[test]` framework. Mock rclone subprocess calls using a trait abstraction.
- **Integration tests**: Use a local rclone remote (e.g., `rclone config create test local`) or rclone's built-in `:memory:` backend for fast, hermetic tests.
- **E2E tests**: Use a dedicated Google Drive service account with a test folder. Clean up after each test run.

### Test Fixtures

- **Small fixture set**: 10 files across 3 directories, ~100KB total.
- **Medium fixture set**: 1,000 files, mixed sizes (1B–10MB), ~500MB total.
- **Large fixture set**: 50,000 files, ~50GB total (for TC-FS-010, TC-FS-006).
- **Special characters set**: Files with spaces, unicode, brackets, hashes, empty files, dotfiles.

### CI Considerations

- Unit and integration tests should run on every commit.
- E2E tests against real GDrive can run nightly or on release branches.
- Use `rclone serve` or a local backend for most integration tests to avoid GDrive API quotas.
