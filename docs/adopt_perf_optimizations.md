# Performance Optimization & Resumability Plan: `init --adopt`

## 1. Executive Summary & Problem Statement

When initializing an existing remote directory structure using:
```bash
incr-backup init --adopt
```
users observed that adopting a **3.3 GB** remote on Google Drive took **over 40 minutes** and appeared frozen, with no stdout/stderr feedback during execution. At that rate, adopting a realistic **200 GB** dataset would take days or fail completely.

### Core Problems Identified
1. **Lack of Observability & Silent Execution**:
   `SubprocessRunner` uses `Command::output()`, synchronously buffering all stdout/stderr until rclone terminates. Stderr is discarded upon success, giving zero real-time progress indicators or timing visibility.
2. **Pathological Move Performance ($O(\text{files})$ instead of $O(\text{dirs})$)**:
   The implementation ran:
   ```bash
   rclone move <remote> <remote>/current/ --exclude current/** --exclude diffs/** --exclude .meta/** --delete-empty-src-dirs
   ```
   Passing `--exclude` filters and nesting the destination inside the source completely disabled Google Drive's native **`DirMove`** (server-side atomic directory reparenting). Rclone fell back to crawling every nested file, making tens of thousands of individual `files.update` API calls subject to Google Drive's ~5–10 TPS API throttle, followed by thousands of delete calls for empty folders.
3. **Unoptimized Directory Scans (`lsjson`)**:
   `rclone.lsjson_recursive_hash()` did not include the `--fast-list` flag, forcing rclone to make a separate API call to list every single subfolder sequentially.
4. **Dangerous / Ambiguous State if `current/` Already Exists**:
   If `current/` was already present (from an interrupted prior run or existing user data), `--exclude current/**` silently skipped whatever was inside `current/` and dumped remaining files into it, risking state corruption.
5. **Hash Computation vs. Network Download Risks**:
   On remotes without precomputed hashes, `--hash` can trigger full file downloads/streaming. Without an explicit check, `init --adopt` could silently exhaust bandwidth.

---

## 2. In-Depth Technical Analysis

### 2.1 The 40-Minute Bottleneck: Why `rclone move` Crawled
Google Drive supports native server-side folder moves (**`DirMove`**). Moving a directory from one location to another within the same Drive requires **1 single API call** (`files.update` to modify `addParents` / `removeParents`), taking $\approx 200\text{ms}$ whether the directory contains 1 file or 100,000 files.

However, rclone's internal engine only invokes `DirMove` when **all** of the following conditions are met:
1. Destination directory does not already exist.
2. **No filters are applied** (no `--include`, `--exclude`, or `--filter`). The moment any filter is provided, rclone cannot guarantee that every item in the folder qualifies, so it disables `DirMove` and falls back to a recursive file-by-file traversal.
3. Destination is not a descendant path of the source in the same call.

Because `incr-backup` passed `--exclude current/** --exclude diffs/** --exclude .meta/**` and moved the parent directly into its own child `current/`, `DirMove` was completely bypassed. For a typical 3.3 GB folder containing ~20,000 files (e.g., code, node_modules, git objects), at 8 API calls/sec, the move alone took $\frac{20,000}{8} \approx 2,500\text{s} \approx 41.6\text{ minutes}$.

### 2.2 Hash Storage on Google Drive
- **Google Drive Storage Mechanics**: Google Drive **always** calculates and stores MD5 checksums (`md5Checksum`) for all standard uploaded files in its metadata catalog.
- **Verification Command**:
  ```bash
  rclone backend features <remote>:
  ```
  Reports `"Hashes": ["md5"]`.
  Running `rclone hashsum md5 <remote>:<path/to/file> -vv` returns the MD5 checksum instantaneously with:
  `Transferred: 0 B / 0 B, 0%, 0 B/s, ETA -`
  confirming zero data download.
- **Exception**: Native Google Workspace documents (`.gdoc`, `.gsheet`) have no checksums because they do not exist as byte streams until exported.

---

## 3. Solution Architecture

```mermaid
flowchart TD
    A["Start: init --adopt"] --> B["Step 1: Inspect Remote Root (lsjson non-recursive)"]
    B --> C{"Is remote already initialized?"}
    C -- Yes --> D["NO-OP: Exit 0 (already initialized)"]
    C -- No --> E{"Does 'current/' directory exist?"}
    
    E -- Yes --> F{"Check .meta/journal.json: Is previous adopt in progress?"}
    F -- No / Invalid --> G["Error & Abort: Unmanaged 'current/' found. Suggest manual rename/delete."]
    F -- Yes (Incomplete WAL) --> H["Resume Adoption via Journal State"]
    
    E -- No --> I["Create WAL Journal: init_adopt (Pending)"]
    I --> J["Create 'current/' remote directory"]
    H --> K["Step 2: Atomic Item Move (DirMove)"]
    J --> K
    
    K --> L["For each top-level folder: rclone move src/dir current/dir (NO filters -> DirMove)"]
    L --> M["For each top-level file: rclone moveto src/file current/file"]
    M --> N["Step 3: Create remaining structure (diffs/, .meta/) & save config"]
    N --> O["Step 4: Scan current/ (lsjson --recursive --hash --fast-list)"]
    O --> P{"Are MD5 hashes present?"}
    
    P -- Missing & !--download-to-hash --> Q["Abort & Error: Suggest --download-to-hash or local upload"]
    P -- Valid (or --download-to-hash) --> R["Step 5: Write initial manifest & cleanup Journal"]
    R --> S["Success: Display per-step and total timing metrics"]
```

### 3.1 Observability & Phase Timing Instrumentation
Every phase in `cmd_init` is wrapped with `std::time::Instant::now()`. Progress and elapsed durations are printed clearly:
```text
[1/5] Inspecting remote status... (0.7s)
[2/5] Adopting root entries into 'current/'...
      - Moved 4 directories and 12 files (DirMove)... (2.1s)
[3/5] Creating remote structure and config... (0.8s)
[4/5] Scanning adopted files for manifest (fast-list)... (3.9s)
[5/5] Generating and committing initial manifest... (0.6s)
✓ Successfully adopted 18,432 files (3.3 GB) in 8.1s.
```

### 3.2 Atomic Top-Level Adoption (`DirMove`)
Instead of moving the root folder with `--exclude`, the adoption process:
1. Obtains the top-level directory entries from the non-recursive `lsjson` check.
2. Iterates over top-level items:
   - For each **directory** `dir` (excluding `.meta` and `diffs`):
     Runs `rclone move <remote>/<dir> <remote>/current/<dir>` **without any filters**.
     Since `current/<dir>` does not exist yet, Google Drive executes a **native `DirMove`** (1 metadata API call per folder).
   - For each **file** `file`:
     Runs `rclone moveto <remote>/<file> <remote>/current/<file>`.
3. **Complexity shift**: From $O(\text{total files})$ to $O(\text{root items})$.
   - 3.3 GB / 20,000 files across 6 root directories: drops from 20,000 API calls to **6 API calls** ($\approx 2\text{ seconds}$).
   - 200 GB across 20 root directories: takes $\approx 5\text{ seconds}$.

### 3.3 Recursive Fast Listing (`--fast-list`)
In `crates/incr-backup/src/rclone.rs`, `lsjson` appends `--fast-list` when `recursive == true`. Google Drive bundles directory lookups into 1,000-item batch queries, eliminating per-directory round-trip latency.

### 3.4 Precomputed Hash Guard & `--download-to-hash`
If files in `current/` return empty MD5 values (e.g., unsupported backends or non-hashable files):
- If `--download-to-hash` is **absent**:
  Abort prior to writing the manifest, preserving remote state. Output clear instructions:
  1. Pass `--download-to-hash` if streaming all files to compute hashes is desired.
  2. Alternative: Delete the remote contents and run standard `init` + `sync` from local source (local hashing during upload is significantly faster than downloading).
- If `--download-to-hash` is **present**:
  Proceed with streaming/computing hashes.

### 3.5 Write-Ahead Journaling (WAL) & Safe `current/` Handling
To prevent corrupting existing data or failing unrecoverably upon interruption:
1. **When `current/` is detected on the remote**:
   - Query `.meta/journal.json` using `RcloneJournalBackend`.
   - **Case A: Active Incomplete Journal Found (`operation == "init_adopt"`)**:
     - An earlier `init --adopt` was interrupted.
     - Inspect journal step statuses (e.g. `move_root_items`, `create_structure`, `generate_manifest`).
     - Display: `[INFO] Found incomplete adopt journal from <timestamp>. Resuming adoption...`
     - Resume execution from the last incomplete step without failing.
   - **Case B: No Active Incomplete Journal Found**:
     - `current/` exists as unmanaged user data or from an unjournaled foreign process.
     - **Fail immediately** with `IncrError::AdoptCurrentAlreadyExists`.
     - Hint: Prompt the user to either manually rename or delete `current/` before retrying.
2. **During Normal Adopt**:
   - Write `.meta/journal.json` before initiating root moves.
   - Update step progress as each phase succeeds.
   - Delete `.meta/journal.json` only after the initial manifest is successfully committed to `.meta/manifests/current.json`.

---

## 4. Phased Implementation Plan

### Phase 1: Observability, Metrics & CLI Flags
- **CLI Update**: Add `--download-to-hash` flag to `Commands::Init` in [`crates/incr-backup/src/main.rs`](file:///Users/shabeer/git/shabeer_incr_backup/crates/incr-backup/src/main.rs).
- **Error Types**: Add to [`crates/incr-backup/src/error.rs`](file:///Users/shabeer/git/shabeer_incr_backup/crates/incr-backup/src/error.rs):
  - `IncrError::AdoptCurrentAlreadyExists { remote: String }`
  - `IncrError::MissingPrecomputedHashes { remote: String, missing_count: usize }`
- **Timing Diagnostics**: Instrument `cmd_init` with `std::time::Instant` milestones.

### Phase 2: Rclone Listing Optimization
- **`crates/incr-backup/src/rclone.rs`**:
  In `lsjson()`, append `--fast-list` when `recursive == true`.
- Update unit tests in `rclone.rs` verifying `--fast-list` is passed.

### Phase 3: Atomic Top-Level `DirMove` Adoption
- Replace `rclone.move_dir(config.remote_path, ...)` with root-level item iteration:
  - Separate root entries into directories vs files.
  - Move directories atomically to `current/<dir>` with `move` (no filters).
  - Move files to `current/<file>` with `moveto`.
- Add validation in `from_rclone_lsjson`: check for missing MD5 hashes and evaluate `--download-to-hash`.

### Phase 4: WAL Journaling & Resumable Adopt Flow
- Integrate `JournalManager` in `cmd_init`:
  - Define `Journal` schema with steps:
    1. `move_root_items`
    2. `create_structure`
    3. `upload_config`
    4. `generate_manifest`
    5. `upload_manifest`
    6. `cleanup_journal`
  - When `has_current == true`:
    - Read `.meta/journal.json`.
    - If valid in-progress `init_adopt` journal: resume from incomplete step.
    - If no in-progress journal: abort with `AdoptCurrentAlreadyExists`.
  - Clean up journal and release any lock upon successful manifest commitment.

### Phase 5: Verification & Test Suite
- **Unit Tests (`test_init.rs`)**:
  - `test_init_adopt_fails_when_current_exists_without_journal`
  - `test_init_adopt_resumes_when_incomplete_journal_present`
  - `test_init_adopt_fails_on_missing_hashes_without_flag`
  - `test_init_adopt_succeeds_with_download_to_hash_flag`
  - `test_init_adopt_uses_dir_move_timing`
- **Live Google Drive Integration Tests (`test_live_gdrive.rs`)**:
  - Verify adoption of multi-directory structure completes in under 10 seconds.
  - Verify kill/interrupt during adopt resumes cleanly.

---

## 5. Risk Assessment, Limitations & Cons

| Strategy / Step | Potential Risks / Limitations | Mitigation / Best Approach |
| :--- | :--- | :--- |
| **Atomic Top-Level `DirMove`** | Loose files in root (e.g., 50,000 files in root without subfolders) still require 50,000 API calls. | Batch top-level files or use `--transfers 16 --checkers 16`. Encourage folder structures. |
| **Atomic Top-Level `DirMove`** | Object storage backends (S3, B2) do not support native `DirMove` (no folder primitives). | S3 will still copy file-by-file, but eliminating `--exclude` and recursive child targeting avoids infinite loop checks and reduces roundtrips. |
| **Journal Resume on `current/`** | If `current/` was created by an interrupted run before the journal was uploaded to remote `.meta/journal.json`. | The journal is staged locally and committed to remote before moving root entries; if `current/` exists without a journal, failing safely forces user inspection rather than data loss. |
| **`--fast-list` Memory Usage** | On gigantic remotes with $> 500,000$ files, `--fast-list` caches directory hierarchy in RAM. | For standard machines, 500k entries is $\approx 100\text{MB}$ RAM, which is completely safe. Modern cloud environments easily handle this. |
| **`--download-to-hash`** | Users may accidentally pass `--download-to-hash` on multi-terabyte remotes, causing huge bandwidth charges. | Require explicit CLI flag; display a prominent interactive or console warning with estimated transfer size. |

