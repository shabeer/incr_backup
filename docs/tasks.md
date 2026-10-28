# incr-backup — Implementation Tasks

> Tracking file for implementation progress.

## Phase 1: Foundation

| # | Module | Status | Lines | Notes |
|---|--------|--------|-------|-------|
| 1 | `Cargo.toml` | ✅ Done | 38 | All dependencies configured & fetched |
| 2 | `error.rs` | ✅ Done | 159 | All error variants defined with thiserror |
| 3 | `config.rs` | ✅ Done | 907 | Full config, presets, env overrides & 16 unit tests |
| 4 | `rclone.rs` | ✅ Done | 545 | Subprocess wrapper, streaming rcat, dry-run, recursive listing & 5 unit tests |
| 5 | `manifest.rs` | ✅ Done | 473 | Manifest CRUD, rclone lsjson parser, diff & 10 unit tests |
| 6 | `scanner.rs` | ✅ Done | 406 | Parallel MD5 (rayon+walkdir), glob exclude & 6 unit tests |
| 7 | `lock.rs` | ✅ Done | 424 | Advisory remote lock manager, staleness & 8 unit tests |
| 8 | `lib.rs` | ✅ Done | 39 | Re-exports all 12 modules |

## Phase 2: Core Backup

| # | Module | Status | Lines | Notes |
|---|--------|--------|-------|-------|
| 9 | `journal.rs` | ✅ Done | 277 | WAL journal, step state machine, resume detection & 5 unit tests |
| 10 | `snapshot.rs` | ✅ Done | 608 | Rotation, promotion, tier cascading & 11 unit tests |
| 11 | `sync.rs` | ✅ Done | 733 | Full sync pipeline orchestration, WAL resume & 2 unit tests |
| 12 | `main.rs` | ✅ Done | 716 | CLI entry point with clap derive, global flags, all subcommands dispatched |

## Phase 3: Restore, Diff & Remove

| # | Module | Status | Lines | Notes |
|---|--------|--------|-------|-------|
| 13 | `restore.rs` | ✅ Done | 439 | Point-in-time location resolution, grouping, verify & 2 unit tests |
| 14 | `diff.rs` | ✅ Done | 179 | Manifest comparison, stat/only/json flags & unit test |
| 15 | `remove.rs` | ✅ Done | 419 | Purge files/dirs from all snapshots & manifests, dry-run, JSON & 5 unit tests |

## Phase 4: Hardening

| # | Module | Status | Lines | Notes |
|---|--------|--------|-------|-------|
| 16 | `verify.rs` | ✅ Done | 590 | 12 integrity checks (config, manifests, count, size, hash, lock, space) |

## Phase 5: Verification & Integration Testing

| # | Task | Status | Notes |
|---|------|--------|-------|
| 17 | Compilation Check | ✅ Done | `cargo check` clean (0 errors, 0 warnings) |
| 18 | Linter Check | ✅ Done | `cargo clippy --all-targets` clean (0 warnings) |
| 19 | Unit Tests Suite | ✅ Done | 75/75 unit tests passing |
| 20 | `test_init.rs` Integration Tests | ✅ Done | 4/4 integration tests passing (init, custom retention, adopt, error cases) |
| 21 | `test_sync.rs` Integration Tests | ✅ Done | 2/2 integration tests passing (basic sync, incremental diffs, dry-run, list, status) |
| 22 | `test_restore.rs` Integration Tests | ✅ Done | 3/3 integration tests passing (current, point-in-time from diffs, overwrite protection) |
| 23 | `test_diff.rs` Integration Tests | ✅ Done | 1/1 integration tests passing (human-readable, --stat, --json) |
| 24 | `test_locking.rs` Integration Tests | ✅ Done | 1/1 integration tests passing (mutual exclusion, --force override) |
| 25 | `test_verify.rs` Integration Tests | ✅ Done | 1/1 integration tests passing (quick checks, --space, --check-hashes) |
| 26 | `test_remove.rs` Integration Tests | ✅ Done | 7/7 integration tests passing (single file, directory, dry-run, JSON, root protection) |
| 27 | `config` Subcommands & Metadata Sync | ✅ Done | `config push`, `config pull`, `config diff`, and mismatch warnings in `status` and `verify` |
| 28 | `test_config.rs` Integration Tests | ✅ Done | 1/1 integration tests passing (push, pull, diff, mismatch warning, overwrite guard) |
| 29 | Release Build | ✅ Done | Optimized standalone binary produced at `target/release/incr-backup` (3.1 MB) |
| 30 | Documentation | ✅ Done | Updated all markdown files (`README.md`, `walkthrough.md`, `TEST_CASES.md`, `tasks.md`, `IMPLEMENTATION_PLAN.md`) |
| 31 | Total Test Count | ✅ Done | **98 passed / 0 failed** across all test suites (77 unit + 21 integration) |

---

*Last updated: 2026-10-28T19:20+05:30*
