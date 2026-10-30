# incr-backup — Comprehensive Test Cases

> **Project:** incr-backup (rsnapshot-like incremental backup for rclone Google Drive remotes)
> **Version:** 0.1.0-draft
> **Last Updated:** 2026-10-28

---

## Table of Contents

- [1. `init` Command](#1-init-command)
- [2. `sync` Command — Core Operation](#2-sync-command--core-operation)
- [3. `sync` — Discrete Snapshots & Naming](#3-sync--discrete-snapshots--naming)
- [4. `sync` — Point-in-Time Integrity & Re-sync Behavior](#4-sync--point-in-time-integrity--re-sync-behavior)
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
- [19. `compact` Command — Dynamic Snapshot Consolidation](#19-compact-command--dynamic-snapshot-consolidation)

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
| **Expected Result** | - Exit code 0. <br>- Remote directory `gdrive:backups/myhost/current/` is created (empty). <br>- `gdrive:backups/myhost/.meta/config.json` is created with source path and remote info. <br>- `gdrive:backups/myhost/.meta/journal.json` is created (empty/clean state). <br>- No `diffs/` directory created yet. <br>- Local config file is written (e.g., `~/.config/incr-backup/<namespace>.toml`). |
| **Category**     | e2e |

---

### TC-INIT-002 — Init with configuration file

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-INIT-002 |
| **Test Name**    | Initialize from local config file |
| **Preconditions** | Remote is configured and target path does not exist. `backup.toml` specifies source, remote, and excludes. |
| **Steps**        | 1. Run `incr-backup init -c backup.toml` |
| **Expected Result** | - `config.json` uploaded to remote with source, remote, and exclude settings. <br>- All other behavior identical to TC-INIT-001. |
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
| **Test Name**    | Init uses source, remote, and options from config file when flags are omitted |
| **Preconditions** | `incr-backup.toml` exists with valid `[source]` and `[remote]`. Remote target is empty. |
| **Steps**        | 1. Run `incr-backup init` |
| **Expected Result** | - Exit code 0. <br>- `source` and `remote` read from `incr-backup.toml`. <br>- Config uploaded to remote `.meta/config.json`. <br>- Directory structure (`current/`, `diffs/`, `.meta/`) created on remote. |
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
| **Steps**        | 1. Run `incr-backup sync` |
| **Expected Result** | - Lock acquired in `.meta/lock`. <br>- Journal written with operation start. <br>- `rclone sync` runs from source → `current/`. <br>- Manifest generated and uploaded to `.meta/manifests/<snapshot_name>.json` and `.meta/manifests/current.json`. <br>- Journal cleared. Lock released. <br>- Exit code 0. |
| **Category**     | e2e |

---

### TC-SYNC-002 — Sync with changed files

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-002 |
| **Test Name**    | Sync detects and backs up changed files |
| **Preconditions** | One prior sync completed. 3 files modified in source since last sync. |
| **Steps**        | 1. Modify 3 files in source directory. <br>2. Run `incr-backup sync` |
| **Expected Result** | - `rclone sync --backup-dir diffs/<snapshot_name>` runs. <br>- The 3 old versions of changed files appear in `diffs/<snapshot_name>/`. <br>- `current/` has the new versions. <br>- New manifest reflects updated md5/size/mtime for the 3 files. |
| **Category**     | e2e |

---

### TC-SYNC-003 — Sync with deleted files

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-003 |
| **Test Name**    | Deleted files are captured in diffs |
| **Preconditions** | One prior sync completed. 2 files deleted from source. |
| **Steps**        | 1. Delete 2 files from source. <br>2. Run `incr-backup sync` |
| **Expected Result** | - The 2 deleted files appear in `diffs/<snapshot_name>/` (their last known versions). <br>- `current/` no longer contains the 2 files. <br>- New manifest does not list the 2 deleted files. |
| **Category**     | e2e |

---

### TC-SYNC-004 — Sync with added files

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-004 |
| **Test Name**    | Newly added files appear in current/ |
| **Preconditions** | One prior sync completed. 5 new files added to source. |
| **Steps**        | 1. Add 5 new files to source. <br>2. Run `incr-backup sync` |
| **Expected Result** | - 5 new files uploaded to `current/`. <br>- `diffs/<snapshot_name>/` has no entries for the new files (they didn't exist before). <br>- New manifest includes all 5 new files with correct hashes. |
| **Category**     | e2e |

---

### TC-SYNC-005 — Sync with mixed changes (add + modify + delete)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-005 |
| **Test Name**    | Sync handles mixed change types simultaneously |
| **Preconditions** | Prior sync completed. Source has 2 added, 3 modified, 1 deleted file. |
| **Steps**        | 1. Make mixed changes. <br>2. Run `incr-backup sync` |
| **Expected Result** | - Added files in `current/` only. <br>- Modified files: old versions in `diffs/<snapshot_name>/`, new versions in `current/`. <br>- Deleted file: old version in `diffs/<snapshot_name>/`, absent from `current/`. <br>- Manifest accurate for all changes. |
| **Category**     | e2e |

---

### TC-SYNC-006 — Sync with no changes (idempotent)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-006 |
| **Test Name**    | Sync when source is unchanged |
| **Preconditions** | Prior sync completed. No changes in source. |
| **Steps**        | 1. Run `incr-backup sync` |
| **Expected Result** | - `rclone sync --backup-dir` runs but produces no backup files (nothing changed). <br>- New manifest is identical in file content to previous. <br>- Exit code 0. |
| **Category**     | e2e |

---

### TC-SYNC-007 — Sync with empty source directory

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-007 |
| **Test Name**    | Sync when source directory is empty |
| **Preconditions** | Prior sync with 10 files. Source directory now emptied (all files removed). |
| **Steps**        | 1. Remove all files from source. <br>2. Run `incr-backup sync` |
| **Expected Result** | - All 10 files moved to `diffs/<snapshot_name>/`. <br>- `current/` is empty. <br>- Manifest has zero entries. <br>- Warning emitted: "Source directory is empty. All files will be deleted from current/." <br>- Exit code 0. |
| **Category**     | e2e |

---

### TC-SYNC-008 — Sync acquires and releases lock

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-008 |
| **Test Name**    | Lock lifecycle during sync |
| **Preconditions** | No lock present on remote. |
| **Steps**        | 1. Run `incr-backup sync`. <br>2. Monitor `.meta/lock` during execution. |
| **Expected Result** | - `.meta/lock` is created at the start of sync with PID, hostname, and timestamp. <br>- `.meta/lock` is deleted upon successful completion. <br>- If interrupted, lock remains (tested separately). |
| **Category**     | integration |

---

### TC-SYNC-009 — Sync generates valid manifest

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-009 |
| **Test Name**    | Manifest accuracy after sync |
| **Preconditions** | Source has known files with pre-computed md5 hashes. |
| **Steps**        | 1. Run `incr-backup sync`. <br>2. Download and parse the generated manifest. <br>3. Compare each entry against `rclone lsjson --hash` output. |
| **Expected Result** | - Manifest is valid JSON. <br>- Every file in `current/` has a manifest entry. <br>- Every manifest entry has `path`, `size`, `md5`, `mtime`. <br>- All fields match `rclone lsjson --hash` output exactly. |
| **Category**     | integration |

---

### TC-SYNC-010 — Dry-run mode

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SYNC-010 |
| **Test Name**    | Sync with --dry-run makes no changes |
| **Preconditions** | Initialized namespace with prior sync. Source has changes. |
| **Steps**        | 1. Run `incr-backup sync --dry-run` |
| **Expected Result** | - Tool outputs what it *would* do (dry run preview). <br>- No lock acquired. <br>- No files moved, uploaded, or deleted on remote. <br>- No manifest generated. <br>- Exit code 0. |
| **Category**     | e2e |

---

## 3. `sync` — Discrete Snapshots & Naming

### TC-SNAP-001 — Append-only discrete snapshot with ISO-8601 timestamp

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SNAP-001 |
| **Test Name**    | Each sync creates an independent timestamped snapshot |
| **Preconditions** | Initialized namespace. Source has modified files. |
| **Steps**        | 1. Run `incr-backup sync` |
| **Expected Result** | - Creates an append-only snapshot directory in `diffs/<timestamp>` (e.g. `2026-10-29T10-30Z`). <br>- Generates and uploads `.meta/manifests/<timestamp>.json` and `.meta/manifests/current.json`. <br>- No slot limits, tier rotation, or overwrites. |
| **Category**     | integration |

---

### TC-SNAP-002 — Sync with custom snapshot name (`--name`)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SNAP-002 |
| **Test Name**    | Sync with user-specified snapshot name |
| **Preconditions** | Initialized namespace. Source has changes. |
| **Steps**        | 1. Run `incr-backup sync --name pre-deploy` |
| **Expected Result** | - Diffs are stored in `diffs/pre-deploy/`. <br>- Manifest created at `.meta/manifests/pre-deploy.json`. <br>- Manifest records authoritative UTC creation timestamp in `created_at`. <br>- `incr-backup list` displays `pre-deploy` with formatted timestamp. <br>- Exit code 0. |
| **Category**     | integration |

---

### TC-SNAP-003 — Timezone formatting (UTC and local offset)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SNAP-003 |
| **Test Name**    | Timezone configuration formats filesystem-safe snapshot names |
| **Preconditions** | Initialized namespace. |
| **Steps**        | 1. Run `incr-backup sync --timezone utc` <br>2. Run `incr-backup sync --timezone local` |
| **Expected Result** | - UTC snapshot name ends with `Z` (e.g. `2026-10-29T10-30Z`). <br>- Local snapshot name contains formatted offset without colons (e.g. `2026-10-29T16-00+0530`). <br>- Both are valid filesystem path names on all platforms. |
| **Category**     | integration |

---

### TC-SNAP-004 — Rapid consecutive syncs create distinct immutable snapshots

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SNAP-004 |
| **Test Name**    | Rapid syncs do not overwrite previous snapshots |
| **Preconditions** | Initialized namespace. |
| **Steps**        | 1. Run `incr-backup sync --name snap1`. <br>2. Modify a file in source. <br>3. Run `incr-backup sync --name snap2`. |
| **Expected Result** | - Both `diffs/snap1/` and `diffs/snap2/` exist independently. <br>- Both `.meta/manifests/snap1.json` and `.meta/manifests/snap2.json` exist. <br>- No in-place overwriting occurs. |
| **Category**     | e2e |

---

### TC-SNAP-005 — Preserving diffs for changed and deleted files

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SNAP-005 |
| **Test Name**    | Intermediate file states are safely preserved across snapshots |
| **Preconditions** | Prior sync completed. |
| **Steps**        | 1. Modify `fileA.txt` and delete `fileB.txt`. <br>2. Run `incr-backup sync`. |
| **Expected Result** | - Pre-edit version of `fileA.txt` and deleted `fileB.txt` are preserved in the new diff directory. <br>- `current/` reflects the latest source state. |
| **Category**     | integration |

---

### TC-SNAP-006 — Dry-run preview displays snapshot target

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-SNAP-006 |
| **Test Name**    | Sync dry-run previews snapshot target without executing changes |
| **Preconditions** | Initialized namespace. |
| **Steps**        | 1. Run `incr-backup sync --dry-run` |
| **Expected Result** | - Outputs `[DRY-RUN] Would sync <source> -> <remote>/current/ with diffs to diffs/<name>`. <br>- No remote changes made. |
| **Category**     | unit |

---

## 4. `sync` — Point-in-Time Integrity & Re-sync Behavior

### TC-PIR-001 — Manifests track point-in-time state accurately across syncs

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-PIR-001 |
| **Test Name**    | Manifests accurately record file metadata at each point in time |
| **Preconditions** | Several syncs executed with varying changes. |
| **Steps**        | 1. Read `.meta/manifests/<name>.json` for each snapshot. |
| **Expected Result** | - Each manifest lists exact files, sizes, MD5s, and timestamps matching source at that time. |
| **Category**     | integration |

---

### TC-PIR-002 — Crash during sync resumes safely via Write-Ahead Journal

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-PIR-002 |
| **Test Name**    | Incomplete sync resumes from last pending journal step |
| **Preconditions** | Sync crashed during file transfer leaving `.meta/journal.json`. |
| **Steps**        | 1. Run `incr-backup sync` |
| **Expected Result** | - Detects interrupted journal. <br>- Resumes from last pending step. <br>- Completes successfully and cleans up journal. |
| **Category**     | integration |

---

### TC-PIR-003 — Idempotent sync resume skips already-transferred files

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-PIR-003 |
| **Test Name**    | Resumed sync does not re-upload identical files |
| **Preconditions** | Half of files already transferred before interruption. |
| **Steps**        | 1. Resume sync. |
| **Expected Result** | - `rclone sync` compares MD5 hashes and transfers only remaining/changed files. <br>- Total bandwidth spent is minimal. |
| **Category**     | e2e |

---

### TC-PIR-004 — Sync respects exclude filters

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-PIR-004 |
| **Test Name**    | Excluded files are omitted from sync and manifests |
| **Preconditions** | Config has `exclude = ["*.tmp", ".git/**"]`. Source contains matching files. |
| **Steps**        | 1. Run `incr-backup sync` |
| **Expected Result** | - Excluded files do not appear in `current/` or any snapshot manifests. |
| **Category**     | integration |

---

### TC-PIR-005 — Stale lock auto-recovery during sync

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-PIR-005 |
| **Test Name**    | Sync breaks expired lock automatically |
| **Preconditions** | Existing lock file older than timeout_minutes (60 min). |
| **Steps**        | 1. Run `incr-backup sync` |
| **Expected Result** | - Warning logged about stale lock. <br>- Lock broken and re-acquired by current process. <br>- Sync completes normally. |
| **Category**     | integration |

---

### TC-PIR-006 — Manual lock break override (`--force`)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-PIR-006 |
| **Test Name**    | --force breaks active lock |
| **Preconditions** | Active non-stale lock exists on remote. |
| **Steps**        | 1. Run `incr-backup sync --force` |
| **Expected Result** | - Active lock is forcefully broken. <br>- New lock acquired. <br>- Sync proceeds to completion. |
| **Category**     | integration |

---

## 5. `list` Command

### TC-LIST-001 — List all snapshots

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-LIST-001 |
| **Test Name**    | List displays all snapshots |
| **Preconditions** | Multiple snapshots exist (e.g. `2026-10-28T08-00Z`, `2026-10-28T07-00Z`, `daily-2026-10-27`). |
| **Steps**        | 1. Run `incr-backup list` |
| **Expected Result** | - Output shows all snapshots sorted chronologically. <br>- Each entry shows: snapshot name, timestamp, number of files, total size. <br>- `current` is listed as a special entry at the top. |
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
| **Preconditions** | Snapshots exist. |
| **Steps**        | 1. Run `incr-backup list --json` |
| **Expected Result** | - Valid JSON array output. <br>- Each object has `name`, `timestamp`, `file_count`, `total_size`. <br>- Parseable by `jq`. |
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
| **Expected Result** | - Output shows: <br>  - Total remote usage. <br>  - `current/` size. <br>  - `diffs/` size. <br>  - `.meta/` size. <br>  - Last sync time. <br>  - Lock status (locked/unlocked). |
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

### TC-REST-002 — Restore a historical snapshot

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REST-002 |
| **Test Name**    | Restore snapshot from diffs |
| **Preconditions** | `snap1`, `snap2`, `snap3` exist. `/tmp/restore` is empty. |
| **Steps**        | 1. Run `incr-backup restore snap1 --to /tmp/restore` |
| **Expected Result** | - Tool reads snap1's manifest to get the file list. <br>- For each file: looks in diff directories created AFTER snap1 (e.g. `snap2`, `snap3`) for old versions, or uses `current/` if unchanged. <br>- All files downloaded to `/tmp/restore` matching snap1's state. <br>- md5 hashes of restored files match manifest entries. |
| **Category**     | e2e |

---

### TC-REST-003 — Restore oldest snapshot

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REST-003 |
| **Test Name**    | Restore the oldest available snapshot |
| **Preconditions** | `oldest-snap` is the earliest snapshot. |
| **Steps**        | 1. Run `incr-backup restore oldest-snap --to /tmp/restore` |
| **Expected Result** | - File resolution searches through all diff directories created after `oldest-snap` forward to current. <br>- Complete restoration of `oldest-snap` state. <br>- All hashes validated. |
| **Category**     | e2e |

---

### TC-REST-004 — Restore with --path filter (single file)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REST-004 |
| **Test Name**    | Restore a single file from a snapshot |
| **Preconditions** | `snap1` exists with manifest containing `docs/report.pdf`. |
| **Steps**        | 1. Run `incr-backup restore snap1 --to /tmp/restore --path docs/report.pdf` |
| **Expected Result** | - Only `docs/report.pdf` is restored to `/tmp/restore/docs/report.pdf`. <br>- No other files downloaded. <br>- Hash matches manifest. |
| **Category**     | e2e |

---

### TC-REST-005 — Restore with --path filter (subdirectory)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REST-005 |
| **Test Name**    | Restore an entire subdirectory from a snapshot |
| **Preconditions** | `snap1` manifest contains files under `photos/2026/`. |
| **Steps**        | 1. Run `incr-backup restore snap1 --to /tmp/restore --path photos/2026/` |
| **Expected Result** | - All files under `photos/2026/` in that snapshot are restored. <br>- Directory structure preserved. <br>- Files outside `photos/2026/` are not downloaded. |
| **Category**     | e2e |

---

### TC-REST-006 — Restore to non-empty directory

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REST-006 |
| **Test Name**    | Restore refuses to overwrite existing files without --overwrite |
| **Preconditions** | `/tmp/restore` already contains files. |
| **Steps**        | 1. Run `incr-backup restore snap1 --to /tmp/restore` |
| **Expected Result** | - Exit code non-zero. <br>- Error: "Target directory `/tmp/restore` is not empty. Use `--overwrite` to replace existing files." |
| **Category**     | e2e |

---

### TC-REST-007 — Restore non-existent snapshot

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REST-007 |
| **Test Name**    | Restore a snapshot that doesn't exist |
| **Preconditions** | Only `snap1` and `snap2` exist. |
| **Steps**        | 1. Run `incr-backup restore non-existent-snap --to /tmp/restore` |
| **Expected Result** | - Exit code non-zero. <br>- Error: "Snapshot `non-existent-snap` does not exist. Run `incr-backup list` to see available snapshots." |
| **Category**     | e2e |

---

### TC-REST-008 — Restore file resolution: file in diff vs current

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-REST-008 |
| **Test Name**    | File resolution correctly chooses diff or current source |
| **Preconditions** | - `snap1` manifest lists `file_A.txt` (md5 = `aaa`). <br>- `file_A.txt` was modified after `snap1`, so its old version (md5 = `aaa`) is in `diffs/snap2/`. <br>- `current/file_A.txt` has md5 = `bbb` (newer). <br>- `snap1` manifest also lists `file_B.txt` (md5 = `ccc`), which hasn't changed — it's in `current/` with md5 = `ccc`. |
| **Steps**        | 1. Run `incr-backup restore snap1 --to /tmp/restore` |
| **Expected Result** | - `file_A.txt` is sourced from `diffs/snap2/` (md5 `aaa`). <br>- `file_B.txt` is sourced from `current/` (md5 `ccc`). <br>- Both files match snap1 manifest hashes. |
| **Category**     | integration |

---

## 8. `diff` Command

### TC-DIFF-001 — Diff between two discrete snapshots

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-DIFF-001 |
| **Test Name**    | Diff shows changes between snap1 and snap2 |
| **Preconditions** | `snap1` and `snap2` exist with different manifests. |
| **Steps**        | 1. Run `incr-backup diff snap1 snap2` |
| **Expected Result** | - Output lists: <br>  - **Added** files (in snap2 but not snap1). <br>  - **Deleted** files (in snap1 but not snap2). <br>  - **Modified** files (same path, different md5/size). <br>- Counts displayed: "3 added, 1 deleted, 2 modified." |
| **Category**     | e2e |

---

### TC-DIFF-002 — Diff between a snapshot and itself

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-DIFF-002 |
| **Test Name**    | Diff of a snapshot with itself is empty |
| **Preconditions** | `snap1` exists. |
| **Steps**        | 1. Run `incr-backup diff snap1 snap1` |
| **Expected Result** | - Output: "No differences." <br>- Zero added, deleted, or modified files. |
| **Category**     | unit |

---

### TC-DIFF-003 — Diff between timestamped and consolidated snapshots

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-DIFF-003 |
| **Test Name**    | Diff between a timestamped and a consolidated snapshot |
| **Preconditions** | Timestamped snapshot and consolidated snapshot both exist with manifests. |
| **Steps**        | 1. Run `incr-backup diff 2026-10-28T08-00Z daily-2026-10-27` |
| **Expected Result** | - Correctly compares manifests. <br>- Shows all added/deleted/modified files between the two points in time. |
| **Category**     | e2e |

---

### TC-DIFF-004 — Diff with current

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-DIFF-004 |
| **Test Name**    | Diff a historical snapshot against current |
| **Preconditions** | `snap1` and `current` (latest manifest) exist. |
| **Steps**        | 1. Run `incr-backup diff snap1 current` |
| **Expected Result** | - Shows all changes between snap1 and the current state. <br>- Useful for seeing "what changed since that snapshot." |
| **Category**     | e2e |

---

### TC-DIFF-005 — Diff with --json output

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-DIFF-005 |
| **Test Name**    | JSON-formatted diff output |
| **Preconditions** | Two snapshots exist. |
| **Steps**        | 1. Run `incr-backup diff snap1 snap2 --json` |
| **Expected Result** | - Valid JSON with `added`, `deleted`, `modified` arrays. <br>- Each entry has `path`, `size_a`, `size_b`, `md5_a`, `md5_b` (where applicable). |
| **Category**     | e2e |

---

### TC-DIFF-006 — Diff with non-existent snapshot

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-DIFF-006 |
| **Test Name**    | Diff references a snapshot that doesn't exist |
| **Preconditions** | `snap1` exists, `snap99` does not. |
| **Steps**        | 1. Run `incr-backup diff snap1 snap99` |
| **Expected Result** | - Exit code non-zero. <br>- Error: "Snapshot `snap99` does not exist." |
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
| **Test Name**    | Verify snap1 checks files across diffs and current |
| **Preconditions** | `snap1` exists. Its manifest references files in diffs/ and current/. |
| **Steps**        | 1. Run `incr-backup verify snap1` |
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
| **Steps**        | 1. In a second terminal, run `incr-backup sync` |
| **Expected Result** | - Exit code non-zero. <br>- Error: "Another operation is in progress (locked by PID XXXX on host YYYY at TIME). Use `--force` to break the lock if you believe this is stale." <br>- No data corruption. |
| **Category**     | e2e |

---

### TC-LOCK-002 — Break stale lock with --force

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-LOCK-002 |
| **Test Name**    | Force-break a stale lock |
| **Preconditions** | Lock file exists but is older than timeout (1 hour). The process that created it has crashed. |
| **Steps**        | 1. Run `incr-backup sync --force` |
| **Expected Result** | - Warning: "Breaking stale lock (age: Xh Ym, created by PID XXXX on host YYYY)." <br>- Old lock removed, new lock acquired. <br>- Checks journal for interrupted operation and attempts resume/cleanup. <br>- Sync proceeds normally. |
| **Category**     | e2e |

---

### TC-LOCK-003 — Lock within timeout not breakable without --force

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-LOCK-003 |
| **Test Name**    | Recent lock cannot be broken without --force |
| **Preconditions** | Lock file exists and is 10 minutes old (within timeout). |
| **Steps**        | 1. Run `incr-backup sync` (without --force) |
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
| **Steps**        | 1. Run `incr-backup sync --force` |
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
| **Steps**        | 1. Run `incr-backup sync`. <br>2. Inspect `.meta/journal.json` during execution (or from logs). |
| **Expected Result** | - Journal entries include: `acquire_lock`, `rclone_sync`, `generate_manifest`, `upload_manifest`, `cleanup_journal`, `release_lock`. <br>- After successful completion, journal is removed. |
| **Category**     | integration |

---

### TC-JOUR-002 — Resume after interruption during rclone sync

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-JOUR-002 |
| **Test Name**    | Crash during rclone sync phase |
| **Preconditions** | Simulate crash during `rclone sync` (mid-transfer). Journal shows `rclone_sync`. |
| **Steps**        | 1. Run `incr-backup sync --force` |
| **Expected Result** | - Tool reads journal, sees sync was interrupted. <br>- Re-runs `rclone sync` from scratch (rclone handles its own idempotency). <br>- Manifest generated from final state. <br>- No data loss. |
| **Category**     | e2e |

---

### TC-JOUR-003 — Resume after interruption during manifest upload

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-JOUR-003 |
| **Test Name**    | Crash after manifest generated but before upload |
| **Preconditions** | Journal shows `generate_manifest` done but `upload_manifest` pending. |
| **Steps**        | 1. Run `incr-backup sync --force` |
| **Expected Result** | - Tool re-generates manifest (or re-uploads the generated one). <br>- Completes cleanly. |
| **Category**     | e2e |

---

### TC-JOUR-004 — Clean journal after successful operation

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-JOUR-004 |
| **Test Name**    | Journal is clean after successful sync |
| **Preconditions** | None. |
| **Steps**        | 1. Run `incr-backup sync`. <br>2. Read `.meta/journal.json`. |
| **Expected Result** | - Journal file is removed after successful completion. |
| **Category**     | integration |

---

## 12. Error Handling & Edge Cases

### TC-ERR-001 — Source directory does not exist at sync time

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ERR-001 |
| **Test Name**    | Sync when source directory has been removed |
| **Preconditions** | Initialized namespace. Source directory `/home/user/docs` no longer exists. |
| **Steps**        | 1. Run `incr-backup sync` |
| **Expected Result** | - Exit code non-zero. <br>- Error: "Source directory `/home/user/docs` does not exist." <br>- No lock left behind. No partial state. |
| **Category**     | e2e |

---

### TC-ERR-002 — Manifest corruption (invalid JSON)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ERR-002 |
| **Test Name**    | Tool handles corrupted manifest gracefully |
| **Preconditions** | Manually corrupt a manifest file on remote (write invalid JSON). |
| **Steps**        | 1. Run `incr-backup verify` <br>2. Run `incr-backup restore snap1 --to /tmp/restore` (if snap1's manifest is corrupt) |
| **Expected Result** | - `verify`: Reports "Manifest for snap1 is corrupted: invalid JSON at byte X." <br>- `restore`: Fails with clear error rather than panic. <br>- Suggests: "Run `incr-backup sync` to regenerate the current manifest." |
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
| **Steps**        | 1. Run `incr-backup sync` |
| **Expected Result** | - incr-backup captures rclone's stderr. <br>- Error: "rclone sync failed (exit code X): <rclone stderr>". <br>- Journal records the failure. Lock is released. <br>- No manifest generated (sync didn't complete). |
| **Category**     | integration |

---

### TC-ERR-006 — Remote quota exceeded (disk full)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-ERR-006 |
| **Test Name**    | Sync when Google Drive storage is full |
| **Preconditions** | Google Drive account is at storage quota. |
| **Steps**        | 1. Add new files to source. <br>2. Run `incr-backup sync` |
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
| **Steps**        | 1. Run `incr-backup sync` |
| **Expected Result** | - Warning: "System clock appears to have moved backward. Last sync was at T, current time is T-2h." <br>- Sync proceeds (using current system time for the new snapshot). <br>- Append-only snapshot created cleanly. |
| **Category**     | integration |

---

## 13. Configuration & Environment

### TC-CONF-001 — Config override via environment variable

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CONF-001 |
| **Test Name**    | Override source path via env var |
| **Preconditions** | Config has `source = "/home/user/docs"`. |
| **Steps**        | 1. Run `INCR_BACKUP_SOURCE=/home/user/other-docs incr-backup sync` |
| **Expected Result** | - Source path `/home/user/other-docs` is used for this run. <br>- Config file is NOT modified (env override is transient). |
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
| **Steps**        | 1. Run `INCR_BACKUP_RCLONE=/opt/rclone/rclone incr-backup sync` |
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
| **Steps**        | 1. Create a lock that is 20 minutes old. <br>2. Run `incr-backup sync --force` |
| **Expected Result** | - Lock is considered stale at 20 min (> 15 min configured timeout). <br>- Lock broken and sync proceeds. |
| **Category**     | integration |

---

### TC-CONF-006 — Override timezone via environment variable

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CONF-006 |
| **Test Name**    | Custom timezone via INCR_BACKUP_TIMEZONE |
| **Preconditions** | Config has `timezone = "utc"`. |
| **Steps**        | 1. Run `INCR_BACKUP_TIMEZONE=local incr-backup sync` |
| **Expected Result** | - Snapshot name is formatted with local offset (e.g. `+0530`) instead of `Z`. |
| **Category**     | integration |

---

### TC-CONF-007 — Timezone-aware snapshot naming via CLI and config

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CONF-007 |
| **Test Name**    | Timezone indicator in snapshot directory names (UTC vs Local) |
| **Preconditions** | Initialized backup repository. |
| **Steps**        | 1. Run `incr-backup sync --timezone local` <br>2. Run `incr-backup sync --tz utc` |
| **Expected Result** | - Step 1 creates snapshot ending with numeric offset `+HHMM` or `-HHMM` without colons `:` <br>- Step 2 creates snapshot ending with `Z` (UTC designator). |
| **Category**     | integration |

---

### TC-CONF-008 — Robust TOML timezone parsing across sections and unquoted values

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CONF-008 |
| **Test Name**    | TOML configuration timezone parsing flexibility |
| **Preconditions** | Valid source directory and remote path. |
| **Steps**        | 1. Test `timezone = "local"` at root level. <br>2. Test `timezone = "local"` after `[remote]` or `[source]` table headers. <br>3. Test `[snapshot]` section with `timezone = "local"`. <br>4. Test unquoted `timezone = local` and `timezone=local`. |
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
| **Steps**        | 1. Run `incr-backup sync` <br>2. Run `incr-backup verify current` |
| **Expected Result** | - File synced to `current/my document (final).docx`. <br>- Manifest entry path preserved exactly. <br>- Verify confirms hash match. |
| **Category**     | e2e |

---

### TC-FS-002 — Files with unicode characters

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-002 |
| **Test Name**    | Sync handles unicode filenames |
| **Preconditions** | Source contains `日本語ファイル.txt`, `café_résumé.pdf`, `файл_документ.doc`. |
| **Steps**        | 1. Run `incr-backup sync` <br>2. Run `incr-backup verify current` <br>3. Run `incr-backup restore current --to /tmp/restore` |
| **Expected Result** | - All files synced with correct names. <br>- Manifest correctly encodes unicode paths (UTF-8). <br>- Restore produces correct filenames. |
| **Category**     | e2e |

---

### TC-FS-003 — Files with special characters (brackets, hashes, etc.)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-003 |
| **Test Name**    | Filenames with characters special to shells/GDrive |
| **Preconditions** | Source contains files named: `file#1.txt`, `report[2026].pdf`, `notes & ideas.md`, `100%.txt`. |
| **Steps**        | 1. Run `incr-backup sync` |
| **Expected Result** | - All files synced correctly. <br>- rclone subprocess arguments properly escaped. <br>- Manifest paths are exact. |
| **Category**     | e2e |

---

### TC-FS-004 — Symlinks in source

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-004 |
| **Test Name**    | Symlinks handled according to rclone behavior |
| **Preconditions** | Source contains a symlink `link.txt → ../other/real.txt`. |
| **Steps**        | 1. Run `incr-backup sync` |
| **Expected Result** | - Follows rclone's default behavior (typically follows symlinks and copies the target file). <br>- If rclone skips symlinks, incr-backup logs a warning. <br>- No crashes or panics. |
| **Category**     | e2e |

---

### TC-FS-005 — Permission-denied files in source

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-005 |
| **Test Name**    | Unreadable files in source |
| **Preconditions** | Source contains a file with `chmod 000` (no read permission). |
| **Steps**        | 1. Run `incr-backup sync` |
| **Expected Result** | - rclone reports permission denied for that file. <br>- incr-backup logs the error but continues syncing other files (rclone's `--ignore-errors` or similar). <br>- Exit code indicates partial failure. <br>- Manifest does not include the unreadable file (or marks it as errored). |
| **Category**     | e2e |

---

### TC-FS-006 — Very large file (>5GB, chunked upload)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-006 |
| **Test Name**    | Sync a file larger than Google Drive's chunked upload threshold |
| **Preconditions** | Source contains a 6GB file. |
| **Steps**        | 1. Run `incr-backup sync` |
| **Expected Result** | - rclone handles chunked upload automatically. <br>- File appears in `current/` with correct md5. <br>- Manifest records correct size (> 5GB). <br>- If interrupted mid-upload, journal enables resume. |
| **Category**     | e2e |

---

### TC-FS-007 — Empty file (0 bytes)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-007 |
| **Test Name**    | Sync handles zero-byte files |
| **Preconditions** | Source contains `empty.txt` (0 bytes). |
| **Steps**        | 1. Run `incr-backup sync` |
| **Expected Result** | - File synced to `current/`. <br>- Manifest entry has `size: 0` and valid md5 (`d41d8cd98f00b204e9800998ecf8427e`). |
| **Category**     | e2e |

---

### TC-FS-008 — Deeply nested directory structure

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-008 |
| **Test Name**    | Sync handles deeply nested paths |
| **Preconditions** | Source has a file at depth 20: `a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q/r/s/t/file.txt`. |
| **Steps**        | 1. Run `incr-backup sync` <br>2. Run `incr-backup restore current --to /tmp/restore` |
| **Expected Result** | - Full path preserved in `current/` and manifest. <br>- Restore recreates the full directory hierarchy. |
| **Category**     | e2e |

---

### TC-FS-009 — Very long filename

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-009 |
| **Test Name**    | File with name near filesystem/GDrive limits |
| **Preconditions** | Source has a file with a 200+ character filename. |
| **Steps**        | 1. Run `incr-backup sync` |
| **Expected Result** | - If rclone can handle it, file synced normally. <br>- If GDrive rejects it, error reported clearly. <br>- No panic or data corruption. |
| **Category**     | e2e |

---

### TC-FS-010 — Thousands of small files

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-010 |
| **Test Name**    | Performance with many small files |
| **Preconditions** | Source contains 50,000 small files (1KB each). |
| **Steps**        | 1. Run `incr-backup sync` <br>2. Run `incr-backup verify current` |
| **Expected Result** | - Sync completes successfully (may take time). <br>- Manifest has 50,000 entries and is valid. <br>- Verify completes without error. <br>- `rclone lsjson --recursive` handles the volume. |
| **Category**     | e2e |

---

### TC-FS-011 — Hidden files (dotfiles)

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-FS-011 |
| **Test Name**    | Sync includes hidden files |
| **Preconditions** | Source contains `.gitignore`, `.env`, `.ssh/config`. |
| **Steps**        | 1. Run `incr-backup sync` |
| **Expected Result** | - Hidden files and directories synced to `current/`. <br>- Manifest includes dotfile entries. |
| **Category**     | e2e |

---

## 15. Network & Remote Failures

### TC-NET-001 — Network interruption during sync

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-NET-001 |
| **Test Name**    | Network drops during sync execution |
| **Preconditions** | Simulate network failure (e.g., disconnect WiFi) during sync. |
| **Steps**        | 1. Start `incr-backup sync`. <br>2. Kill network during file transfer. <br>3. Restore network. <br>4. Run `incr-backup sync --force`. |
| **Expected Result** | - First run fails with network error. Lock and journal left on remote. <br>- Second run (with --force) reads journal, detects incomplete sync step. <br>- Cleanup/resume succeeds. <br>- Final state is consistent. |
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
| **Steps**        | 1. Run `incr-backup sync` with `--retries 5` (or rclone's default retries). |
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
| **Steps**        | 1. Run `incr-backup sync` for host_a. <br>2. Run `incr-backup sync` for host_b. |
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
| **Preconditions** | Initial backup taken with `secret.txt` and `file1.txt`. Second backup taken where `secret.txt` was modified (creating a historical diff copy in `diffs/<snapshot_name>/`). |
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
| **Preconditions** | Remote initialized with `exclude = ["*.tmp"]`. Local config edited to add `"*.log"`. |
| **Steps**        | 1. Run `incr-backup -c my-config.toml config diff` |
| **Expected Result** | - Output displays `Excludes: (DIFFER)`. <br>- Highlights local values vs remote values. <br>- Reports `Found 1 difference(s)`. |
| **Category**     | integration |

---

### TC-CFG-002 — `config push` updates `.meta/config.json` under advisory cloud lock

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CFG-002 |
| **Test Name**    | Push local configuration to remote metadata |
| **Preconditions** | Local configuration has updated exclude patterns. |
| **Steps**        | 1. Run `incr-backup -c my-config.toml config push`. <br>2. Run `incr-backup -c my-config.toml config diff`. |
| **Expected Result** | - Remote lock acquired and released. <br>- Remote `.meta/config.json` updated with local settings. <br>- Subsequent `config diff` reports `Local configuration and remote metadata are identical`. |
| **Category**     | integration |

---

### TC-CFG-003 — `config pull` writes remote metadata into local file with overwrite guard

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CFG-003 |
| **Test Name**    | Pull remote configuration into a local TOML file |
| **Preconditions** | Remote `.meta/config.json` exists. Local destination path already exists. |
| **Steps**        | 1. Run `incr-backup config pull --remote <REMOTE> --to existing.toml` without `--overwrite`. <br>2. Re-run with `--overwrite`. |
| **Expected Result** | - Step 1 fails with error requiring `--overwrite`. <br>- Step 2 overwrites local file with valid TOML generated from remote metadata. |
| **Category**     | integration |

---

### TC-CFG-004 — Configuration template validation

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-CFG-004 |
| **Test Name**    | Validate syntax and parsing of `config/incr-backup-example.toml` |
| **Preconditions** | File `config/incr-backup-example.toml` exists. |
| **Steps**        | 1. Run unit test `test_example_config_file_parses` loading `config/incr-backup-example.toml`. |
| **Expected Result** | - Successfully parses with `Config::load`. <br>- All fields, options, comments, and globs verified. |
| **Category**     | unit |

---

## 19. `compact` Command — Dynamic Snapshot Consolidation

### TC-COMP-001 — ISO-8601 timezone offset validation matching configured timezone

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-COMP-001 |
| **Test Name**    | Explicit offset in `--from` and `--to` must match configured backup timezone |
| **Preconditions** | Remote backup initialized with `timezone = "utc"`. |
| **Steps**        | 1. Run `incr-backup compact --from 2026-10-28T00:00:00+05:30 --to 2026-10-28T23:59:59+05:30 --name daily-test --timezone utc`. <br>2. Run `incr-backup compact --from 2026-10-28T00:00:00Z --to 2026-10-28T23:59:59Z --name daily-test --timezone local` (on machine with +05:30). |
| **Expected Result** | - Step 1 fails with: `Error: Timezone offset in --from (+05:30) does not match configured backup timezone (+00:00).`<br>- Step 2 fails with: `Error: Timezone offset in --from (+00:00) does not match configured backup timezone (+05:30).` <br>- Zero changes made to remote. |
| **Category**     | integration |

---

### TC-COMP-002 — Zero snapshots in range gracefully exits

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-COMP-002 |
| **Test Name**    | Zero snapshots found in user-specified date window |
| **Preconditions** | Remote contains snapshots from 2026 only. |
| **Steps**        | 1. Run `incr-backup compact --from 2025-01-01T00:00:00Z --to 2025-01-02T00:00:00Z --name empty-range --timezone utc` |
| **Expected Result** | - Exits with code 1. <br>- Error message: `No snapshots found in range [2025-01-01T00:00:00Z, 2025-01-02T00:00:00Z]`. <br>- Zero remote files modified or deleted. |
| **Category**     | integration |

---

### TC-COMP-003 — Single snapshot in range aliases/renames cleanly

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-COMP-003 |
| **Test Name**    | Range contains exactly one snapshot |
| **Preconditions** | Range contains a single snapshot `2026-10-28T08-00Z`. |
| **Steps**        | 1. Run `incr-backup compact --from <range> --name daily-single` |
| **Expected Result** | - Snapshot manifest renamed to `.meta/manifests/daily-single.json`. <br>- Diff directory moved to `diffs/daily-single/`. <br>- Manifest `snapshot_name` updated to `daily-single`. <br>- Exit code 0. |
| **Category**     | integration |

---

### TC-COMP-004 — Concrete reverse-incremental walkthrough

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-COMP-004 |
| **Test Name**    | Earliest-diff rule keeps required pre-range files and discards intermediate revisions |
| **Preconditions** | Day 1 has `report.docx` ($V_A$), `budget.xlsx` ($V_1$), `logo.png` ($V_L$). <br>Day 2 runs: 10:00 (`diffs/snap-10:00` has $V_1$), 14:00 (`diffs/snap-14:00` has $V_A$), 20:00 (`diffs/snap-20:00` has $V_B$). <br>Day 3 run: 12:00 (`diffs/snap-12:00` has $V_C$). |
| **Steps**        | 1. Run `incr-backup compact --from 2026-10-28T00:00:00Z --to 2026-10-28T23:59:59Z --name daily-2026-10-28 --timezone utc`. |
| **Expected Result** | - Consolidated diff `diffs/daily-2026-10-28/` contains `report.docx` ($V_A$) and `budget.xlsx` ($V_1$). <br>- Intermediate diff revision $V_B$ is discarded. <br>- Source snapshots `snap-10:00`, `snap-14:00`, `snap-20:00` and manifests are purged. <br>- `.meta/manifests/daily-2026-10-28.json` records anchor state ($V_C, V_2, V_L$). |
| **Category**     | integration |

---

### TC-COMP-005 — Full point-in-time restore across pre-range, consolidated, and post-range snapshots

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-COMP-005 |
| **Test Name**    | Mathematically verifiable restore after compaction |
| **Preconditions** | Compaction completed as in TC-COMP-004. |
| **Steps**        | 1. `incr-backup restore daily-2026-10-28 --to /tmp/r_day2 --verify` <br>2. `incr-backup restore day-2026-10-27 --to /tmp/r_day1 --verify` <br>3. `incr-backup restore snap-12:00 --to /tmp/r_day3 --verify` |
| **Expected Result** | - Step 1 restores Day 2 state ($V_C, V_2, V_L$) with MD5 verification passing. <br>- Step 2 restores Day 1 state ($V_A, V_1, V_L$) with MD5 verification passing. <br>- Step 3 restores Day 3 state ($V_D, V_2, V_L$) with MD5 verification passing. |
| **Category**     | integration |

---

### TC-COMP-006 — Intra-range created and deleted file lifecycle

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-COMP-006 |
| **Test Name**    | File created and deleted within compaction range is purged completely |
| **Preconditions** | Pre-range snapshot $S_{\text{prev}}$ does NOT contain `temp.log`. <br>Intra-range snapshot 1 creates `temp.log`. <br>Intra-range snapshot 2 deletes `temp.log`, moving it into diff. |
| **Steps**        | 1. Run `incr-backup compact --from <START> --to <END> --name daily-temp-test`. |
| **Expected Result** | - `temp.log` is identified as intra-range scratch. <br>- Discarded completely; not present in `diffs/daily-temp-test/`. <br>- Storage reclaimed with zero leakage. |
| **Category**     | integration |

---

### TC-COMP-007 — Compaction from beginning of history

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-COMP-007 |
| **Test Name**    | Compacting snapshots from the beginning of history discards all diffs |
| **Preconditions** | Range starts at the very first snapshot of the backup namespace (no older snapshots exist). |
| **Steps**        | 1. Run `incr-backup compact` covering all initial snapshots. |
| **Expected Result** | - Compactor detects $S_{\text{prev}} = \text{None}$. <br>- All intra-range diffs are safely discarded because no pre-range history exists to reference them. <br>- Anchor manifest preserved at `.meta/manifests/<name>.json`. |
| **Category**     | integration |

---

### TC-COMP-008 — Dry-run visibility with `--summary` and `--full-list`

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-COMP-008 |
| **Test Name**    | Dry-run previews plan without modifying any remote data |
| **Preconditions** | Multiple snapshots in range. |
| **Steps**        | 1. Run `incr-backup compact --from <START> --to <END> --name <NAME> --dry-run --summary`. <br>2. Run `incr-backup compact --from <START> --to <END> --name <NAME> --dry-run --full-list`. |
| **Expected Result** | - Step 1 prints high-level summary (range, snapshots in range, anchor, files kept/discarded counts and bytes). <br>- Step 2 prints detailed file-by-file plan (+ kept, - discarded with reasons, manifest and purge actions). <br>- Remote diffs and manifests remain 100% untouched. |
| **Category**     | integration |

---

### TC-COMP-009 — Advisory cloud locking and concurrency protection

| Field            | Value |
|------------------|-------|
| **Test ID**      | TC-COMP-009 |
| **Test Name**    | Compaction acquires exclusive lock and refuses concurrent execution |
| **Preconditions** | Active lock exists at `.meta/lock`. |
| **Steps**        | 1. Run `incr-backup compact` without `--force`. <br>2. Run with `--force`. |
| **Expected Result** | - Step 1 fails with `backup locked by '<holder>'`. <br>- Step 2 breaks stale lock, acquires advisory lock, performs compaction, and releases lock upon completion. |
| **Category**     | integration |

---

## Appendix A: Test Case Summary

| Section | ID Range | Count |
|---------|----------|-------|
| 1. `init` Command | TC-INIT-001 – TC-INIT-014 | 14 |
| 2. `sync` — Core | TC-SYNC-001 – TC-SYNC-010 | 10 |
| 3. `sync` — Discrete Snapshots | TC-SNAP-001 – TC-SNAP-006 | 6 |
| 4. `sync` — Point-in-Time Integrity | TC-PIR-001 – TC-PIR-006 | 6 |
| 5. `list` Command | TC-LIST-001 – TC-LIST-003 | 3 |
| 6. `status` Command | TC-STAT-001 – TC-STAT-003 | 3 |
| 7. `restore` Command | TC-REST-001 – TC-REST-008 | 8 |
| 8. `diff` Command | TC-DIFF-001 – TC-DIFF-006 | 6 |
| 9. `verify` Command | TC-VERI-001 – TC-VERI-005 | 5 |
| 10. Locking & Concurrency | TC-LOCK-001 – TC-LOCK-005 | 5 |
| 11. Journal & Resumability | TC-JOUR-001 – TC-JOUR-004 | 4 |
| 12. Error Handling | TC-ERR-001 – TC-ERR-008 | 8 |
| 13. Configuration & Env | TC-CONF-001 – TC-CONF-008 | 8 |
| 14. Filesystem Edge Cases | TC-FS-001 – TC-FS-011 | 11 |
| 15. Network & Remote | TC-NET-001 – TC-NET-005 | 5 |
| 16. Multi-Source & Namespace | TC-NS-001 – TC-NS-004 | 4 |
| 17. `remove` Command | TC-REM-001 – TC-REM-006 | 6 |
| 18. `config` Command | TC-CFG-001 – TC-CFG-004 | 4 |
| 19. `compact` Command | TC-COMP-001 – TC-COMP-009 | 9 |
| **Total** | | **121** |

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
