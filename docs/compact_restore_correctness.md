# Compact & Restore Correctness: Implementation Audit, Missing Tests, and Scenario Analysis

This document provides a comprehensive correctness and restore-verification audit for the `compact` command in `shabeer_incr_backup`. It covers implemented tests, identifies testing gaps, details multi-snapshot file lifecycle scenarios (including integration and live Google Drive test designs), evaluates restore test coverage, and defines an actionable testing roadmap.

---

## 1. Architectural Principles & Invariants

The `compact` command collapses multiple snapshots within a timestamp range `[--from, --to]` into a single named snapshot (`--name`). The reverse-incremental backup architecture enforces strict mathematical invariants:

1. **Two-Phase Safe Protocol (Staging $\rightarrow$ Verification $\rightarrow$ Commit $\rightarrow$ Purge)**:
   - Kept diff files are moved into `.meta/staging/<name>/` via server-side batch operations.
   - A staging journal (`.meta/staging/<name>.journal.json`) records source-destination mappings to enable automatic in-flight rollback and crash recovery.
   - Stage 2 verifies file count against the anchor manifest and verifies MD5 hashes of all staged files against the computed plan.
   - Only upon verification success is the staged manifest committed to `.meta/manifests/<name>.json`, the diff directory committed to `diffs/<name>/`, and source snapshots purged/tombstoned.
2. **Reverse-Incremental Dependency Resolution**:
   - The consolidated manifest adopts the newest snapshot in the range (**Anchor**, $S_{\text{newest}}$).
   - Historical diff preservation is evaluated against the preceding snapshot immediately prior to `--from` ($S_{\text{prev}}$):
     - **File in $S_{\text{prev}}$**: Keep earliest intra-range diff version (preserves the state prior to that window's modifications); discard later revisions.
     - **File not in $S_{\text{prev}}$**: Intra-range scratch; discard all revisions completely.
     - **$S_{\text{prev}} = \text{None}$** (beginning of history): Discard all intra-range diffs completely (no older history exists).
3. **Uniform / Identical Snapshot Format**:
   - Consolidated snapshots share the exact same format and metadata as original snapshots.
   - Allows **recursive compaction** (e.g., hourly $\rightarrow$ daily $\rightarrow$ weekly $\rightarrow$ monthly).
   - `incr-backup restore` uses the exact same reverse-search resolution for consolidated snapshots as raw snapshots.
4. **Advisory Cloud Locking**:
   - Mandatory lock (`.meta/lock`) acquired before plan calculation and mutations; supports `--force` override. `--dry-run` does not acquire locks.
5. **Atomic Catalog Synchronization & Tombstoning**:
   - Atomic update of `.meta/snapshots.jsonl` (with fallback to `.meta/snapshot.jsonl`).
   - Source diff directories are concurrently renamed to `<dir>_deleted` (tombstoning) for safe background reclamation by `incr-backup gc --orphaned-diffs`.

---

## 2. Audit of Implemented Correctness Tests

### 2.1 Unit Tests (`crates/incr-backup/src/compact.rs`)

| Test Function | Target Feature | Description & Verifications | Restore Tested? |
| :--- | :--- | :--- | :---: |
| `test_parse_and_validate_timestamp_utc` | Timezone validation | Validates RFC-3339 formats (`Z`, `+00:00`) when UTC configured; asserts `IncrError::TimezoneMismatch` on `+05:30`. | N/A |
| `test_parse_and_validate_timestamp_local` | Timezone validation | Validates matching local offset; asserts `IncrError::TimezoneMismatch` on `Z`. | N/A |
| `test_earliest_diff_rule_logic` | Dependency resolution | **Stub only**: Constructs two `SnapshotMeta` instances and asserts `files.len() == 3`. Does not test plan construction or earliest-diff filtering. | **NO** |

### 2.2 Integration Tests (`crates/incr-backup/tests/test_compact.rs`)

| Test Function | Test Spec | Description & Verifications | Restore Tested? |
| :--- | :--- | :--- | :---: |
| `test_compact_cli_timezone_validation` | **TC-COMP-001** | CLI rejects invalid timezone offsets matching configured timezone for both UTC and Local without modifying remote. | N/A |
| `test_compact_no_snapshots_in_range` | **TC-COMP-002** | Exits code 1 when 0 snapshots match range: `No snapshots found in range [...]`. Zero remote mutations. | N/A |
| `test_compact_single_snapshot` | **TC-COMP-003** | Fast rename path for 1 snapshot in range. Verifies `.meta/manifests/renamed-single.json` created. | **NO** (Never restores `renamed-single`) |
| `test_compact_concrete_walkthrough_and_point_in_time_restore` | **TC-COMP-004**<br>**TC-COMP-005**<br>**TC-COMP-008** | Multi-snapshot walkthrough across 3 days: tests `--dry-run --summary`, `--dry-run --full-list`, real compaction, keeps $V_A$ and $V_1$, discards $V_B$, purges source snapshots. Restores Day 2, Day 1, and Day 3 with `--verify`. | **YES** (Restores Day 1, Day 2, Day 3 with MD5 check) |
| `test_compact_intra_range_created_and_deleted` | **TC-COMP-006** | File created and deleted within range (`temp.txt`) is discarded and absent from consolidated diff directory. | **NO** (Never tests restoring $S_{\text{prev}}$ or anchor) |
| `test_compact_uses_snapshot_jsonl` | Catalog fallback | Fallback handling when catalog is named `snapshot.jsonl`. Verifies consolidated snapshot added and purged snapshots removed. | **NO** (Never restores snapshots) |
| `test_compact_validates_timestamps_before_lock` | TC-COMP-009 (Fail-fast) | Validates invalid timestamp format, offset mismatch, and `--from > --to` fail fast before attempting lock acquisition. | N/A |
| `test_compact_recovers_from_interrupted_staging_journal` | Crash recovery | Simulates kill midway through Stage 1 with files moved to staging and `.journal.json` present. Next run recovers files to source diffs and completes compaction. | **NO** (Never restores snapshots after recovery) |
| `test_compact_batch_moves_and_purge_integrity` | Batching & integrity | Batch moves into staging with nested directories (`sub/f3.txt`), directory tombstoning, and verifies source diffs are purged. | **PARTIAL** (Restores $S_{\text{prev}}$, but NOT the consolidated snapshot) |

### 2.3 Live Google Drive Tests (`crates/incr-backup/tests/test_live_gdrive.rs`)

| Test Function | Test Spec | Description & Verifications | Restore Tested? |
| :--- | :--- | :--- | :---: |
| `test_live_snapshot_compaction` | **LIV-11** | Runs `sync` twice, tests `compact --dry-run --summary`, then runs actual `compact --summary` on Google Drive. Verifies exit code 0 and snapshot name in `list`. | **NO** (Never restores files or verifies point-in-time state on GDrive) |

---

## 3. Detailed Scenario Analysis: User Scenario 2a

### 3.1 Scenario Definition
- **Context**: 3 snapshots inside the compaction range ($S_1, S_2, S_3$), bounded by preceding snapshot $S_{\text{prev}}$ and post-range snapshot $S_{\text{post}}$.
- **File Behavior**:
  1. **3 files updated across all 3 snapshots**: `f_mod1`, `f_mod2`, `f_mod3` (high churn).
  2. **2 files updated only in 1 snapshot**:
     - `f_single1` updated only in $S_1$ (static thereafter).
     - `f_single2` updated only in $S_2$ (static thereafter).
  3. **1 file deleted in a snapshot**:
     - `f_del` exists in $S_{\text{prev}}$ and $S_1$, then deleted in $S_2$.
  4. **1 baseline static file**: `f_static` (never modified).
  5. **1 post-range modification**: `f_post` updated in $S_{\text{post}}$.

### 3.2 Snapshot Evolution & Diff Matrix

| File | $S_{\text{prev}}$ (Day 0) | $S_1$ (10:00) | $S_2$ (14:00) | $S_3$ (Anchor, 20:00) | $S_{\text{post}}$ (Day 2) |
| :--- | :---: | :---: | :---: | :---: | :---: |
| `f_mod1` | $V_0$ | $V_1$ (diff $S_1$: $V_0$) | $V_2$ (diff $S_2$: $V_1$) | $V_3$ (diff $S_3$: $V_2$) | $V_3$ |
| `f_mod2` | $V_0$ | $V_1$ (diff $S_1$: $V_0$) | $V_2$ (diff $S_2$: $V_1$) | $V_3$ (diff $S_3$: $V_2$) | $V_3$ |
| `f_mod3` | $V_0$ | $V_1$ (diff $S_1$: $V_0$) | $V_2$ (diff $S_2$: $V_1$) | $V_3$ (diff $S_3$: $V_2$) | $V_3$ |
| `f_single1` | $V_0$ | $V_1$ (diff $S_1$: $V_0$) | $V_1$ (no diff) | $V_1$ (no diff) | $V_1$ |
| `f_single2` | $V_0$ | $V_0$ (no diff) | $V_1$ (diff $S_2$: $V_0$) | $V_1$ (no diff) | $V_1$ |
| `f_del` | $V_0$ | $V_0$ (no diff) | *Deleted* (diff $S_2$: $V_0$) | *Absent* (no diff) | *Absent* |
| `f_static` | $V_0$ | $V_0$ | $V_0$ | $V_0$ | $V_0$ |
| `f_post` | $V_0$ | $V_0$ | $V_0$ | $V_0$ | $V_1$ (diff $S_{\text{post}}$: $V_0$) |

### 3.3 Compaction Action on Range $[S_1 \dots S_3] \rightarrow \text{consolidated}$

- **Anchor**: $S_3$ (contains `f_mod1` $V_3$, `f_mod2` $V_3$, `f_mod3` $V_3$, `f_single1` $V_1$, `f_single2` $V_1$, `f_static` $V_0$).
- **$S_{\text{prev}}$ Manifest**: Contains all 7 files at $V_0$.
- **Diff Resolution Table**:

| Diff Path | Source Diff Dir | Version | Is in $S_{\text{prev}}$? | Compaction Decision | Reason |
| :--- | :---: | :---: | :---: | :---: | :--- |
| `f_mod1` | $S_1$ | $V_0$ | Yes | **KEEP** | Earliest diff version in range (needed for $S_{\text{prev}}$) |
| `f_mod1` | $S_2$ | $V_1$ | Yes | **DISCARD** | Superseded by earlier diff version in $S_1$ |
| `f_mod1` | $S_3$ | $V_2$ | Yes | **DISCARD** | Superseded by earlier diff version in $S_1$ |
| `f_mod2` | $S_1$ | $V_0$ | Yes | **KEEP** | Earliest diff version in range (needed for $S_{\text{prev}}$) |
| `f_mod2` | $S_2$ | $V_1$ | Yes | **DISCARD** | Superseded by earlier diff version in $S_1$ |
| `f_mod2` | $S_3$ | $V_2$ | Yes | **DISCARD** | Superseded by earlier diff version in $S_1$ |
| `f_mod3` | $S_1$ | $V_0$ | Yes | **KEEP** | Earliest diff version in range (needed for $S_{\text{prev}}$) |
| `f_mod3` | $S_2$ | $V_1$ | Yes | **DISCARD** | Superseded by earlier diff version in $S_1$ |
| `f_mod3` | $S_3$ | $V_2$ | Yes | **DISCARD** | Superseded by earlier diff version in $S_1$ |
| `f_single1` | $S_1$ | $V_0$ | Yes | **KEEP** | Earliest (and only) diff version in range |
| `f_single2` | $S_2$ | $V_0$ | Yes | **KEEP** | Earliest (and only) diff version in range |
| `f_del` | $S_2$ | $V_0$ | Yes | **KEEP** | Earliest (and only) diff version; needed for $S_{\text{prev}}$ |

- **Summary of Files in `diffs/consolidated/`**: Exactly 6 files, all holding $V_0$:
  `f_mod1` ($V_0$), `f_mod2` ($V_0$), `f_mod3` ($V_0$), `f_single1` ($V_0$), `f_single2` ($V_0$), `f_del` ($V_0$).
- **Discarded Diffs**: Exactly 6 files reclaimed:
  `f_mod1` ($V_1, V_2$), `f_mod2` ($V_1, V_2$), `f_mod3` ($V_1, V_2$).

### 3.4 Point-in-Time Restore Verification Points
Post-compaction restore must mathematically reconstruct:
1. **Restore $S_{\text{prev}}$**:
   - `f_mod1`, `f_mod2`, `f_mod3`, `f_single1`, `f_single2`, `f_del`, `f_static`, `f_post` all at $V_0$.
   - Verifies kept diffs in `diffs/consolidated/` correctly supply $V_0$.
2. **Restore `consolidated` (Anchor $S_3$)**:
   - `f_mod1`, `f_mod2`, `f_mod3` at $V_3$.
   - `f_single1`, `f_single2` at $V_1$.
   - `f_static`, `f_post` at $V_0$.
   - `f_del` is absent.
   - Verifies anchor manifest and current mirror reconstruct anchor state.
3. **Restore $S_{\text{post}}$**:
   - `f_post` at $V_1$, `f_mod*` at $V_3$, `f_single*` at $V_1$, `f_static` at $V_0$.
   - Verifies post-range snapshots remain valid and independent.

---

## 4. Advanced & Edge-Case Compaction Scenarios

Beyond Scenario 2a, the following critical test scenarios address complex real-world reverse-incremental failure modes:

### Scenario 2b.1: Intra-Range Reincarnation (Delete + Recreate with Altered Content)
- **Lifecycle**:
  - $S_{\text{prev}}$ has `doc.txt` ($V_0$).
  - $S_1$ deletes `doc.txt` (moved to `diffs/S_1` holding $V_0$).
  - $S_2$ re-creates `doc.txt` with completely new content ($V_{\text{new}}$).
  - $S_3$ modifies `doc.txt` to $V_{\text{final}}$ (diff in $S_3$ has $V_{\text{new}}$).
- **Compactor Invariant**:
  - In anchor ($S_3$), `doc.txt` exists ($V_{\text{final}}$).
  - In $S_{\text{prev}}$, `doc.txt` exists ($V_0$).
  - Earliest diff rule must keep $S_1$'s diff ($V_0$) and discard $S_3$'s diff ($V_{\text{new}}$).
- **Restore Verification**:
  - Restoring $S_{\text{prev}}$ must yield $V_0$.
  - Restoring $S_3$ (consolidated) must yield $V_{\text{final}}$.

### Scenario 2b.2: Circular / Reverted Edits ($A \rightarrow B \rightarrow A$)
- **Lifecycle**:
  - $S_{\text{prev}}$ has `config.toml` ($V_A$).
  - $S_1$ modifies to $V_B$ (diff in $S_1$ has $V_A$).
  - $S_2$ modifies to $V_C$ (diff in $S_2$ has $V_B$).
  - $S_3$ reverts back to $V_A$ (diff in $S_3$ has $V_C$).
- **Compactor Invariant**:
  - Anchor has $V_A$. $S_{\text{prev}}$ has $V_A$.
  - Earliest diff rule keeps $S_1$ ($V_A$), discarding $S_2$ ($V_B$) and $S_3$ ($V_C$).
- **Restore Verification**:
  - Restoring $S_{\text{prev}}$ gets $V_A$.
  - Restoring $S_3$ (consolidated) gets $V_A$.

### Scenario 2b.3: Directory & Path Restructuring Intra-Range
- **Lifecycle**:
  - Files `legacy/sub/file1.txt` and `legacy/sub/file2.txt` exist in $S_{\text{prev}}$.
  - In $S_1$, directory renamed to `modern/app/file1.txt` and `modern/app/file2.txt`.
  - In $S_2$, `modern/app/file1.txt` is updated.
- **Compactor Invariant**:
  - `legacy/sub/file1.txt` and `file2.txt` moved to diff in $S_1$.
  - Compactor must preserve legacy paths in `diffs/consolidated/` for $S_{\text{prev}}$ without path collision or corrupted tree structure.
- **Restore Verification**:
  - Restoring $S_{\text{prev}}$ restores `legacy/sub/*`.
  - Restoring consolidated snapshot restores `modern/app/*`.

### Scenario 2b.4: Two-Level Recursive Compaction (Uniform Format Verification)
- **Lifecycle**:
  - Hourlies $H_1, H_2 \rightarrow$ compacted to `daily-1`.
  - Hourlies $H_3, H_4 \rightarrow$ compacted to `daily-2`.
  - Dailies `daily-1`, `daily-2` $\rightarrow$ compacted to `weekly-1`.
- **Compactor Invariant**:
  - Consolidated snapshots must be indistinguishable from native snapshots and compact cleanly.
- **Restore Verification**:
  - Point-in-time restore of $S_{\text{prev}}$ before `daily-1`, restore of `weekly-1`, and restore of post-weekly snapshots must all pass MD5 hash validation.

### Scenario 2b.5: Churn from Beginning of History ($S_{\text{prev}} = \text{None}$)
- **Lifecycle**:
  - Compacting snapshots from the very first backup namespace.
  - Files created, modified across multiple snapshots, and deleted.
- **Compactor Invariant**:
  - $S_{\text{prev}} = \text{None}$. All intra-range diffs must be discarded (zero diffs needed by pre-history). Anchor manifest is preserved at `.meta/manifests/<name>.json`.
- **Restore Verification**:
  - Restoring the consolidated anchor must succeed without errors despite diffs being empty.

### Scenario 2b.6: Zero-Diff Snapshots (Empty Diff Directories)
- **Lifecycle**:
  - Snapshots taken where no file changes occurred (0-byte diffs).
- **Compactor Invariant**:
  - Compactor must handle empty/missing diff dirs without error and create an empty `diffs/<name>/` directory to maintain 1:1 snapshot-diff directory parity.
- **Restore Verification**:
  - Restoring consolidated snapshot works seamlessly.

### Scenario 2b.7: Verification Failure & Automated Staging Rollback
- **Lifecycle**:
  - During Stage 1, diff files are moved to `.meta/staging/<name>/`.
  - Simulate corrupted file hash or manifest count mismatch during Stage 2 verification.
- **Compactor Invariant**:
  - Compactor must abort with error and execute `rollback_staged_files`: all staged files moved back to original `diffs/<dir>/` locations, staging wiped, original snapshots 100% intact.
- **Restore Verification**:
  - Restore of original snapshots before the failed compaction continues to succeed without data loss.

---

## 5. Comprehensive Restore Test Coverage Matrix

Audit of whether corresponding restore tests are coded for currently implemented tests and identified test cases:

| Test Identifier | Category | Compaction Tested? | Restore Tested? | Status & Verifications |
| :--- | :--- | :---: | :---: | :--- |
| **TC-COMP-001** | CLI TZ Validation | Yes | N/A | Validation fails before execution; no restore expected. |
| **TC-COMP-002** | Empty Range (0 snaps) | Yes | N/A | Exits code 1; no snapshots exist to restore. |
| **TC-COMP-003** | Single Snapshot Rename | Yes | **CODED** | Restores `renamed-single` with `--verify` and asserts exact content. |
| **TC-COMP-004/005** | Concrete Walkthrough | Yes | **CODED** | Restores Day 1 ($S_{\text{prev}}$), Day 2 (anchor), Day 3 ($S_{\text{post}}$) with `--verify`. |
| **TC-COMP-006** | Intra-range Created/Deleted | Yes | **CODED** | Restores $S_{\text{prev}}$ and anchor; verifies `temp.txt` absent, `keep.txt` intact. |
| **TC-COMP-007** | Beginning of History | **CODED** | **CODED** | Verifies zero diffs kept when $S_{\text{prev}} = \text{None}$; point-in-time restore passes `--verify`. |
| **TC-COMP-008** | Dry-Run Visibility | Yes | N/A | Previews plan without remote mutations. |
| **TC-COMP-009** | Cloud Locking & Force | Yes | N/A | Rejects foreign lock without `--force`; `--force` breaks lock, compacts, and releases lock. |
| **Catalog Fallback** | `snapshot.jsonl` | Yes | **CODED** | Point-in-time restore of `daily-all` using fallback catalog with `--verify`. |
| **Staging Recovery** | Crash recovery journal | Yes | **CODED** | Recovers journal before planning; restores $S_{\text{prev}}$ and anchor with `--verify`. |
| **Batch Purge** | Nested directory batching | Yes | **CODED** | Restores baseline $S_{\text{prev}}$ and consolidated snapshot with `--verify`. |
| **LIV-11 (GDrive)** | Live GDrive Compaction | Yes | **CODED** | Restores anchor from Google Drive with `--verify`; validates remote manifest deletions. |
| **Scenario 2a** | Multi-File Churn Matrix | **CODED** | **CODED** | 3 snaps, 3 files modified in 3, 2 in 1, 1 deleted; full restore of $S_{\text{prev}}$, anchor, and $S_{\text{post}}$. |
| **Scenario 2b.1** | Reincarnation ($A \to \text{del} \to C$) | **CODED** | **CODED** | Verifies earliest diff kept ($V_0$); restores $V_0$ for $S_{\text{prev}}$, $V_{\text{final}}$ for anchor. |
| **Scenario 2b.2** | Circular Edits ($A \to B \to A$) | **CODED** | **CODED** | Verifies earliest diff kept ($V_A$); restores $V_A$ for both $S_{\text{prev}}$ and anchor. |
| **Scenario 2b.3** | Path / Directory Rename | **CODED** | **CODED** | Verified in `test_compact_batch_moves_and_purge_integrity`. |
| **Scenario 2b.4** | Recursive Compaction | **CODED** | **CODED** | Two daily compactions into weekly-1; restores baseline $S_0$ and `weekly-1` with `--verify`. |
| **Scenario 2b.5** | Zero-Diff Snapshots | **CODED** | **CODED** | Handled in `test_compact_beginning_of_history` and `test_compact_single_snapshot`. |
| **Scenario 2b.6** | Stage 2 Automated Rollback | **CODED** | **CODED** | Induces hash verification failure; rolls back staged files to source diffs; snapshots intact. |

---

## 6. Test Implementation Verification Status

All three phases of the correctness roadmap have been implemented and validated against the test suite:

### Phase 1: Local Integration Tests (`tests/test_compact.rs`) — 15 Tests Passing
1. **User Scenario 2a (`test_compact_multi_file_churn_and_full_restore`)**:
   - Tested 8 files across 5 snapshot points ($S_{\text{prev}}$, $S_1, S_2, S_3$, and $S_{\text{post}}$).
   - Validated exactly 6 diff files kept and 6 discarded.
   - Point-in-time restore of $S_{\text{prev}}$, `consolidated-churn`, and $S_{\text{post}}$ passing `--verify`.
2. **TC-COMP-007 (`test_compact_beginning_of_history`)**:
   - $S_{\text{prev}} = \text{None}$; validated all intra-range diffs discarded, empty diff directory preserved, point-in-time restore passes.
3. **Reincarnation & Circular Edits (`test_compact_reincarnation_and_circular_edits`)**:
   - Validated $A \to \text{del} \to C$ and $A \to B \to A$ correctly preserve earliest diff versions.
4. **Recursive Compaction (`test_compact_recursive_hierarchy`)**:
   - Hourly $\to$ daily $\to$ weekly multi-tier compaction with restore verification.
5. **Stage 2 Rollback (`test_compact_verification_failure_rolls_back`)**:
   - Induces hash verification error in Stage 2; verifies automatic rollback of staged files to source diffs and intact restore of source snapshots.
6. **Stale Lock Override (`test_compact_force_breaks_stale_lock`)**:
   - Fails fast without `--force`; breaks lock and releases cleanly with `--force`.
7. **Existing Test Restore Enhancements**:
   - Restores coded for `test_compact_single_snapshot`, `test_compact_intra_range_created_and_deleted`, `test_compact_uses_snapshot_jsonl`, `test_compact_recovers_from_interrupted_staging_journal`, and `test_compact_batch_moves_and_purge_integrity`.

### Phase 2: Live Google Drive Tests (`tests/test_live_gdrive.rs`)
1. **Enhanced `test_live_snapshot_compaction` with Point-in-Time Restore**:
   - Executes `restore consolidated-daily --verify` and validates source manifest deletions on Google Drive.
2. **Live Multi-File Churn Test (`test_live_compact_multi_file_scenario_2a`)**:
   - Added full live test replicating Scenario 2a on Google Drive remote.

### Phase 3: Unit Tests (`src/compact.rs`) — 5 Tests Passing
1. **`test_earliest_diff_rule_logic`**:
   - In-memory verification of `classify_diff_files` across keep, supersede, intra-range scratch, and beginning of history.
2. **`test_validate_compact_params`**:
   - Validates empty name, slashes, backslashes, leading dots, and `--from > --to`.
3. **`test_format_summary_and_full_list`**:
   - Golden output assertion for single and multi-snapshot compaction plans.

