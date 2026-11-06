# Diff Snapshot Timestamp & Manifest Alignment Analysis

## 1. Executive Summary

This document analyzes the snapshot naming and diff association semantics in `incr-backup`.

- **Current Approach (Model A - Event Model):** Sync at $T_2$ displaces overwritten files into `diffs/T2/` and writes `manifests/T2.json`.
- **Proposed Approach (Model B - Predecessor Archival Model):** Before updating `current/` at $T_2$, the sync engine reads the previous sync timestamp/name $T_1$ from `current.json`. It archives `current.json` to `.meta/manifests/T1.json` and displaces files into `diffs/T1/`. After sync, the new state is recorded in `current.json` as $T_2$.

> [!NOTE]
> **Notation Clarification:** Throughout this document, $T_1, T_2, T_k$ represent **actual ISO 8601 timestamps** (e.g., `2026-11-06T10-00Z`) or **custom user-provided names** (e.g., `before-upgrade`). They are **not** rotating slot numbers (like `hourly.0`, `hourly.1`). `incr-backup` operates as an append-only, discrete reverse-incremental backup system.

---

## 2. Problems with the Current Approach (Model A)

The current event-based timestamping model introduces several semantic and architectural friction points:

### 1. Semantic Inversion of Diff Directory Contents
In Model A, when sync runs at $T_2$, `--backup-dir` is set to `diffs/T2/`.
- The files moved into `diffs/T2/` are files that were modified or deleted by the $T_2$ sync.
- **The problem:** The *content* of these displaced files represents their state **as of $T_1$**, not $T_2$.
- When an operator inspects the remote storage (e.g. `rclone ls remote:diffs/2026-11-06T10-00Z/`), the directory timestamp indicates 10:00Z, but the files inside are actually the versions from 08:00Z.

### 2. Asymmetric Association in the Restore Engine
When restoring historical snapshot $T_1$, where are the files that were modified after $T_1$?
- They are **not** in `diffs/T1/`. They are located in `diffs/T2/`!
- This causes counter-intuitive lookup logic in the restore engine:
  ```rust
  // restore.rs (Model A)
  if snap.created_at > target_manifest.created_at {
      // Must look at diffs of snapshots strictly AFTER target!
  }
  ```
- Developers and operators naturally expect snapshot $T_1$ to be associated with `diffs/T1/`, not future timestamps.

### 3. Dummy / Empty Baseline Diff Directories
- On the very first backup ($T_1$), no files are displaced. Yet the system either creates an empty `diffs/T1/` directory or requires special-case tombstone checks (`is_tombstone_diff_dir`, `rmdir_specific`) to avoid lingering empty folders.

### 4. Compaction Cognitive Overhead
- During compaction of range $[T_a, T_b]$:
  - `diffs/T_a/` contains files displaced from *before* $T_a$.
  - The files displaced *at the end* of the range are in `diffs/T_b/`.
  - Reasoning about zero-data-loss invariants requires mental translation because the directory name is offset from the era of its contents.

---

## 3. The Proposed Approach (Model B: Predecessor Archival Model)

### Core Mechanics & State Lifecycle

In a reverse-incremental system, snapshots fall into two distinct operational roles:
1. **The Active Snapshot (Latest):** Lives directly in `current/` (full mirror) and `current.json`. It has **zero diff files** because no newer sync has displaced anything from it yet.
2. **Archived Snapshots (Historical):** Each historical snapshot $T_k$ has an immutable manifest `.meta/manifests/T_k.json` and a matching diff directory `diffs/T_k/`.

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           REVERSE-INCREMENTAL STATE                     │
├──────────────────────────────────────┬──────────────────────────────────┤
│         Archived Snapshots           │         Active Snapshot          │
│  (Historical points in time)         │     (Most recent backup)         │
├──────────────────────────────────────┼──────────────────────────────────┤
│  manifests/T1.json  <-> diffs/T1/    │                                  │
│  manifests/T2.json  <-> diffs/T2/    │  current.json   <-> current/     │
│  manifests/T3.json  <-> diffs/T3/    │  (zero diffs needed)             │
└──────────────────────────────────────┴──────────────────────────────────┘
```

### Execution Flow at Sync $T_2$

1. **Read Predecessor Info:** Before modifying anything, read `current.json` to obtain the timestamp / snapshot name $T_1$ of the current state.
2. **Archive Manifest:** Copy `current.json` $\rightarrow$ `.meta/manifests/T1.json`.
3. **Displace & Sync:** Run `rclone sync <source> <remote>:current/ --backup-dir <remote>:diffs/T1/`.
   - Files overwritten or deleted from $T_1$ move to `diffs/T1/`.
4. **Update Active Manifest:** Generate the new file manifest from `current/` and write to `current.json` with timestamp $T_2$.

---

## 4. Concrete Walkthrough: 3 Files Across 3 Syncs

- **$T_1$ (08:00Z):** Source has `fileA` (v1), `fileB` (v1), `fileC` (v1).
- **$T_2$ (10:00Z):** Source updates `fileA` to v2 (`fileB`, `fileC` unchanged).
- **$T_3$ (12:00Z):** Source updates `fileB` to v2 (`fileA` is v2, `fileC` is v1).

```
Timeline:
    T1 (08:00Z)                  T2 (10:00Z)                  T3 (12:00Z)
    A:v1, B:v1, C:v1             A:v2, B:v1, C:v1             A:v2, B:v2, C:v1
```

### State After Each Run Under Model B

| Sync Run | Archived Manifests & Diffs Created | Active State (`current/` + `current.json`) |
| :--- | :--- | :--- |
| **Run 1 ($T_1$)** | *(None)* — First backup establishes baseline. | `current/` = `[A:v1, B:v1, C:v1]`<br>`current.json` = $T_1$ |
| **Run 2 ($T_2$)** | `manifests/T1.json` archived.<br>`diffs/T1/` receives `fileA` (v1). | `current/` = `[A:v2, B:v1, C:v1]`<br>`current.json` = $T_2$ |
| **Run 3 ($T_3$)** | `manifests/T2.json` archived.<br>`diffs/T2/` receives `fileB` (v1). | `current/` = `[A:v2, B:v2, C:v1]`<br>`current.json` = $T_3$ |

---

## 5. Concrete Walkthrough: Compaction & Consolidation Under Model B

To see exactly how compaction operates under Model B, consider 3 files across 5 sync runs:
* **`report.docx`**: Edited repeatedly across runs.
* **`budget.xlsx`**: Edited once at $T_1$, then unchanged.
* **`logo.png`**: Baseline asset, never modified.

### Timeline of File Versions & Diffs

| Sync Run / Time | `report.docx` | `budget.xlsx` | `logo.png` | What rclone sync puts into `--backup-dir` |
| :--- | :--- | :--- | :--- | :--- |
| **$T_0$ (Oct 27 23:00)** | $V_A$ | $V_1$ | $V_L$ | Initial baseline in `current/` |
| **$T_1$ (Oct 28 10:00)** | $V_A$ *(no change)* | Modified to $V_2$ | $V_L$ *(no change)* | `diffs/T0/` receives **`budget.xlsx` ($V_1$)** |
| **$T_2$ (Oct 28 14:00)** | Modified to $V_B$ | $V_2$ *(no change)* | $V_L$ *(no change)* | `diffs/T1/` receives **`report.docx` ($V_A$)** |
| **$T_3$ (Oct 28 20:00)** | Modified to $V_C$ | $V_2$ *(no change)* | $V_L$ *(no change)* | `diffs/T2/` receives **`report.docx` ($V_B$)** |
| **$T_4$ (Oct 29 12:00)** | Modified to $V_D$ | $V_2$ *(no change)* | $V_L$ *(no change)* | `diffs/T3/` receives **`report.docx` ($V_C$)** |
| **Now ($T_4$ Active)** | In `current/`: **$V_D$** | In `current/`: **$V_2$** | In `current/`: **$V_L$** | `current.json` reflects $T_4$ |

### Executing Compaction on Range $[T_1, T_3]$
We run compaction to collapse all runs on Oct 28 into a single daily snapshot:
```bash
incr-backup compact \
    --from 2026-10-28T00:00:00Z \
    --to 2026-10-28T23:59:59Z \
    --name daily-2026-10-28
```
The snapshots in this range are: $T_1$, $T_2$, and $T_3$.
The anchor snapshot is $T_3$ (newest in range).

#### Step 1: Manifest Selection
The consolidated snapshot adopts the manifest of $T_3$:
- `report.docx` = $V_C$
- `budget.xlsx` = $V_2$
- `logo.png` = $V_L$

#### Step 2: Evaluating Diff Files to Keep vs. Discard
The diff directories corresponding to the in-range snapshots are `diffs/T1/`, `diffs/T2/`, and `diffs/T3/`:

1. **`diffs/T1/report.docx` ($V_A$):**
   - *Who needs $V_A$?* Snapshot $T_0$ needs $V_A$! (At $T_0$, `report.docx` was $V_A$).
   - Since $T_0$ sits before the compaction range, it must remain restorable.
   - **Verdict: KEEP.** Preserved in `diffs/daily-2026-10-28/report.docx`.

2. **`diffs/T2/report.docx` ($V_B$):**
   - *Who needs $V_B$?* No surviving snapshot needs $V_B$. $T_0$ needs $V_A$, and $T_3$ has $V_C$. $V_B$ was an intermediate edit between 14:00 and 20:00.
   - **Verdict: DISCARD.** Reclaimed as freed space.

3. **`diffs/T3/report.docx` ($V_C$):**
   - *Who needs $V_C$?* When $T_4$ ran, $V_C$ was displaced from $T_3$. If anyone later restores the consolidated snapshot `daily-2026-10-28` (which represents the state at $T_3$), they need $V_C$!
   - **Verdict: KEEP.** Preserved in `diffs/daily-2026-10-28/report.docx` as the diff version for the consolidated snapshot.

#### Summary of Compaction Action:
- **Consolidated Manifest:** `.meta/manifests/daily-2026-10-28.json` (cloned from $T_3$).
- **Consolidated Diff Directory:** `diffs/daily-2026-10-28/` containing `report.docx` ($V_A$ and $V_C$).
- **Purged:** `manifests/T1.json`, `manifests/T2.json`, `manifests/T3.json`, and directories `diffs/T1`, `diffs/T2`, `diffs/T3`.
- **Pre-range integrity preserved:** $T_0$ restores `report.docx` ($V_A$) from `diffs/daily-2026-10-28/`, `budget.xlsx` ($V_1$) from `diffs/T0/`, and `logo.png` from `current/`.

---

## 6. Named Snapshot Lifecycle & CLI Representation

Custom named snapshots (e.g. `--name v2.0-pre-release`) integrate seamlessly into Model B:

### Execution Example
1. **At 08:00Z:** Standard automated sync runs. `current/` is at state $T_1$.
2. **At 10:00Z:** Developer runs an explicit named snapshot before deploying a change:
   ```bash
   incr-backup sync --name "v2.0-pre-release"
   ```
   - Previous snapshot `2026-11-06T08-00Z` is archived to:
     `.meta/manifests/2026-11-06T08-00Z.json` and `diffs/2026-11-06T08-00Z/`.
   - `current.json` is updated with `snapshot_name: "v2.0-pre-release"`.

### CLI Representation (`incr-backup list`)
Running `incr-backup list` immediately after the 10:00Z sync displays:
```
SNAPSHOT                CREATED AT            DIFF DIR                FILES    TOTAL SIZE
2026-11-06T08-00Z       2026-11-06 08:00:00   diffs/08-00Z            150      1.2 GB
v2.0-pre-release (*)    2026-11-06 10:00:00   (current)               152      1.2 GB
```
*(The `(*)` and `(current)` indicate that `v2.0-pre-release` is currently active and requires zero diffs).*

3. **At 12:00Z:** Next automated sync runs (`2026-11-06T12-00Z`):
   - The sync reads `current.json`, finds `v2.0-pre-release`.
   - Archives `v2.0-pre-release` to:
     `.meta/manifests/v2.0-pre-release.json` and `diffs/v2.0-pre-release/`.
   - `current.json` advances to `2026-11-06T12-00Z`.

Now `incr-backup list` shows:
```
SNAPSHOT                CREATED AT            DIFF DIR                FILES    TOTAL SIZE
2026-11-06T08-00Z       2026-11-06 08:00:00   diffs/08-00Z            150      1.2 GB
v2.0-pre-release        2026-11-06 10:00:00   diffs/v2.0-pre-release  152      1.2 GB
2026-11-06T12-00Z (*)   2026-11-06 12:00:00   (current)               155      1.3 GB
```

---

## 7. Are There Any Changes in `compact.rs`?

**Yes.** While the high-level compaction principles remain the same, adopting Model B requires specific modifications in [`compact.rs`](file:///Users/shabeer/git/shabeer_incr_backup/crates/incr-backup/src/compact.rs):

### 1. Snapshot Meta & Diff Directory Discovery
In [`compact.rs#L249-L264`](file:///Users/shabeer/git/shabeer_incr_backup/crates/incr-backup/src/compact.rs#L249-L264):
- **Current (Model A):** Diff directory matching relies on fallback matching or legacy stem prefixes because diff directories often hold displaced files from prior snapshots.
- **Proposed (Model B):** Every archived snapshot $T_k$ has an exact 1:1 directory match `diffs/T_k/`. Discovery simplifies to direct matching:
  ```rust
  let diff_dir = if existing_diff_dirs.contains(&stem) {
      Some(stem.clone())
  } else if existing_diff_dirs.contains(&manifest.snapshot_name) {
      Some(manifest.snapshot_name.clone())
  } else {
      None
  };
  ```

### 2. Diff File Classification Logic (`classify_diff_files`)
In [`compact.rs#L353-L435`](file:///Users/shabeer/git/shabeer_incr_backup/crates/incr-backup/src/compact.rs#L353-L435):
- **In Model A:** The anchor snapshot $T_{anchor}$'s diff directory (`diffs/T_anchor`) held files displaced from $T_{anchor-1} \to T_{anchor}$. Files displaced *out* of $T_{anchor}$ lived in $T_{anchor+1}$'s diff directory (outside the range).
- **In Model B:** Files displaced *out* of $T_{anchor}$ live in **`diffs/T_anchor`** (inside the range)!
- **Required Update:** `classify_diff_files` must not discard diff files in `diffs/T_anchor` simply because they are not in $S_{prev}$. Any file in `diffs/T_anchor` that is part of the anchor snapshot's manifest must be retained for the consolidated snapshot to remain restorable against future modifications!

### 3. Compaction Range Validation
- Compaction must validate that the range does not include the active snapshot represented by `current.json`.
- The upper boundary of a compaction range must be strictly older than the active snapshot (`created_at < current_manifest.created_at`), because the active snapshot has no diff directory to merge.

---

## 8. Advantages of Model B

1. **Strict 1:1 Symmetrical Naming:**
   - Every historical snapshot $T_k$ has an exact match: `manifests/T_k.json` and `diffs/T_k/`.
2. **Accurate Timestamping of Diff Contents:**
   - Files in `diffs/T1/` are genuinely the versions that were active at $T_1$.
3. **Natural Inclusive Search Range for Restore:**
   - Restoring $T_1$ starts at $T_1$'s own diff folder and moves forward inclusively:
     $$\text{Search: } \text{diffs}/T_1 \;\to\; \text{diffs}/T_2 \;\to\; \dots \;\to\; \text{current/}$$
4. **No Dummy / Empty Baseline Diffs:**
   - The first sync creates no diffs, cleanly matching the physical reality of a fresh repository.
5. **Crash Recovery Parity with Model A:**
   - If `rclone sync` is interrupted, resumption via the write-ahead journal is identical to Model A: rclone resumes transfer idempotently with `--backup-dir diffs/T1/`.

---

## 9. Complexities, Algorithmic Shifts & Code Impact

### 1. Algorithmic Complexity: Multi-Hop Accumulation Still Exists
> [!WARNING]
> **Common Misunderstanding:** Naming the directory `diffs/T1/` does **not** mean `diffs/T1/` contains all files needed to restore $T_1$.

In Section 4's walkthrough:
- To restore $T_1$: `fileA` is in `diffs/T1/`, but `fileB` is in `diffs/T2/` (displaced at $T_3$), and `fileC` is in `current/`.
- Multi-hop resolution across subsequent snapshots is mathematically required.

### 2. Code Impact: Read-Before-Write Dependency on `current.json`
- In Model B, `sync` **must** fetch and parse `current.json` from the remote before determining the `--backup-dir` path:
  ```rust
  let current_manifest = rclone.cat(&format!("{remote_base}/.meta/manifests/current.json"))?;
  let predecessor_name = current_manifest.snapshot_name;
  let backup_dir = format!("diffs/{predecessor_name}");
  ```

### 3. Code Impact: Restore Logic Inclusivity
- In [`restore.rs`](file:///Users/shabeer/git/shabeer_incr_backup/crates/incr-backup/src/restore.rs#L431):
  - Change search filter from strictly greater (`>`) to inclusive (`>=`):
    ```rust
    if snap.created_at >= target_manifest.created_at {
        if let Some(ref dir) = snap.diff_dir { ... }
    }
    ```

---

## 10. Limitations & Edge Cases

1. **Unmanaged Repositories & `adopt`:**
   - If an existing directory on remote storage has files in `current/` but no `current.json`, `adopt` must synthesize an initial `current.json` with timestamp $T_{adopt}$ before any sync displacing files can occur.
2. **Inability to Delete / Prune the Active Snapshot Directly:**
   - `current/` is the full physical reference tree. Pruning the active snapshot requires a reverse-rollback: restoring the state of the latest archived snapshot into `current/` and dropping its diffs.
3. **Operator Intuition Risk:**
   - Operators must be reminded that `diffs/` contains reverse deltas, not standalone mirrors.

---

## 11. Comparative Evaluation Matrix

| Architectural Dimension | Model A (Event Model) | Model B (Predecessor Archival Model) |
| :--- | :--- | :--- |
| **Manifest & Diff Name Match** | Asymmetric ($T_1$ files placed in `diffs/T2`) | **Strictly symmetric** ($T_1$ files placed in `diffs/T1`) |
| **Diff Content Era** | Offset (labeled with replacement time) | **Direct** (labeled with valid content time) |
| **Latest Snapshot Storage** | Manifest duplicated in `T2.json` and `current.json` | Manifest in `current.json`; archived on next sync |
| **Restore Traversal Logic** | Strictly greater-than (`snap.created_at > target`) | **Inclusive** (`snap.created_at >= target`) |
| **Pre-Sync Remote Dependency** | None (timestamp generated locally) | **Requires reading `current.json`** prior to sync |
| **First Backup Baseline** | Creates empty/dummy `diffs/` | **Naturally skips diff creation** |
| **CLI `list` Source** | Reads `.meta/manifests/*.json` | Reads `.meta/manifests/*.json` + `current.json` |
| **Compaction Code (`compact.rs`)** | Baseline offset mapping | **Updated `classify_diff_files` to retain anchor diffs** |
| **Crash Recovery / Journaling** | Standard idempotent resume | Standard idempotent resume |

---

## 12. Conclusion & Summary

The proposed **Predecessor Archival Model (Model B)** is mathematically sound and brings valuable ergonomic symmetry to `incr-backup`:
1. It eliminates the confusing semantic inversion of finding $T_1$ file versions inside a directory stamped $T_2$.
2. It changes restore traversal to a natural inclusive search (`diffs/T1` $\to$ `diffs/T2` $\to \dots \to$ `current/`).
3. It cleanly represents active vs. archived snapshots, with custom `--name` labels working seamlessly.
4. Implementing it requires updates across `sync.rs` (read `current.json` pre-sync), `restore.rs` (inclusive search), `compact.rs` (update classification to preserve anchor diffs), and `catalog.rs` / `list` (display active snapshot).
