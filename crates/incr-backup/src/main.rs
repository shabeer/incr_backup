//! # incr-backup — CLI Entry Point
//!
//! Reverse-incremental backup and point-in-time restore with dynamic snapshots for rclone remotes.
//!
//! This binary uses `clap` derive macros for argument parsing and dispatches
//! to the library modules for each subcommand.

use std::path::{Path, PathBuf};
use std::process;
use std::time::Instant;

use clap::{Parser, Subcommand};
use log::{error, info};
use rayon::prelude::*;

use incr_backup::changelog::{format_changelog, run_changelog, ChangelogOptions};
use incr_backup::config::{CliOverrides, Config, SnapshotTimezone};
use incr_backup::diff::{DiffOptions, run_diff};
use incr_backup::error::IncrError;
use incr_backup::gc::{run_gc, GcOptions};
use incr_backup::journal::{Journal, JournalManager, StepStatus};
use incr_backup::manifest::{format_size, Manifest};
use incr_backup::rclone::Rclone;
use incr_backup::remove::{RemoveOptions, RemoveSnapshotsOptions, run_remove, run_remove_snapshots};
use incr_backup::restore::{RestoreOptions, run_restore};
use incr_backup::verify::{VerifyOptions, format_verify_result, run_verify};

/// incr-backup — Reverse-incremental backup and point-in-time restore with dynamic snapshots for rclone remotes.
///
/// Backs up local directories to rclone remotes using reverse-incremental
/// snapshots. The `current/` directory on the remote is always a full
/// mirror of the source, directly browseable on Google Drive.
#[derive(Parser, Debug)]
#[command(name = "incr-backup", version, about, long_about = None)]
#[command(next_help_heading = "Global Options")]
struct Cli {
    /// Config file path.
    #[arg(short = 'c', long = "config", default_value = "./incr-backup.toml", env = "INCR_BACKUP_CONFIG", global = true)]
    config: PathBuf,

    /// Override source directory.
    #[arg(short = 's', long = "source", env = "INCR_BACKUP_SOURCE", global = true)]
    source: Option<String>,

    /// Override rclone remote path.
    #[arg(short = 'r', long = "remote", env = "INCR_BACKUP_REMOTE", global = true)]
    remote: Option<String>,

    /// Increase verbosity (-v, -vv, -vvv).
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Show what would happen without making changes.
    #[arg(short = 'n', long = "dry-run", global = true)]
    dry_run: bool,

    /// Path to rclone binary.
    #[arg(long = "rclone-path", env = "INCR_BACKUP_RCLONE_PATH", global = true)]
    rclone_path: Option<String>,

    /// Extra flags passed to every rclone call.
    #[arg(long = "rclone-flags", env = "INCR_BACKUP_RCLONE_FLAGS", global = true)]
    rclone_flags: Option<String>,

    /// Timezone for snapshot timestamps: "utc" or "local".
    #[arg(
        long = "timezone",
        visible_alias = "tz",
        env = "INCR_BACKUP_TIMEZONE",
        value_enum,
        global = true
    )]
    timezone: Option<SnapshotTimezone>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize a new backup configuration.
    Init {
        /// Local source directory (optional if provided in config).
        #[arg(long)]
        source: Option<String>,

        /// rclone remote path (optional if provided in config).
        #[arg(long)]
        remote: Option<String>,

        /// Adopt an existing rclone-synced directory.
        #[arg(long)]
        adopt: bool,

        /// Download/stream files to compute missing hashes if not precomputed by remote.
        #[arg(long)]
        download_to_hash: bool,

        /// Exclude patterns (repeatable).
        #[arg(long)]
        exclude: Vec<String>,
    },

    /// Run a sync operation (creates an append-only snapshot).
    Sync {
        /// Optional custom snapshot name (defaults to current timestamp).
        #[arg(long)]
        name: Option<String>,

        /// Break stale lock and run.
        #[arg(long)]
        force: bool,
    },

    /// List all snapshots.
    List {
        /// Start of snapshot timestamp range (inclusive: first snapshot on or after this timestamp).
        #[arg(long = "from", visible_alias = "start")]
        from: Option<String>,

        /// End of snapshot timestamp range (inclusive: latest snapshot on or before this timestamp).
        #[arg(long = "to", visible_alias = "end")]
        to: Option<String>,

        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Show backup status.
    Status,

    /// Restore a snapshot to a local directory.
    Restore {
        /// Snapshot to restore (e.g., 2026-10-28T08-00Z, daily-2026-10-27, current).
        snapshot: String,

        /// Local destination directory.
        #[arg(long = "to")]
        destination: PathBuf,

        /// Restore only a subdirectory or file.
        #[arg(long = "path")]
        subpath: Option<String>,

        /// Verify checksums after restore.
        #[arg(long)]
        verify: bool,

        /// Allow restoring to non-empty directory.
        #[arg(long)]
        overwrite: bool,
    },

    /// Show differences between two snapshots (snapshot1 should be older and snapshot2 should be newer).
    Diff {
        /// First snapshot - should be older.
        snapshot1: String,

        /// Second snapshot - should be newer.
        snapshot2: String,

        /// JSON output.
        #[arg(long)]
        json: bool,

        /// Show only added files.
        #[arg(long)]
        only_added: bool,

        /// Show only deleted files.
        #[arg(long)]
        only_deleted: bool,

        /// Show only modified files.
        #[arg(long)]
        only_modified: bool,

        /// Summary counts/sizes only.
        #[arg(long)]
        stat: bool,
    },

    /// Show changelog across snapshots with files and directories added, deleted, and updated.
    Changelog {
        /// Start of snapshot timestamp range (inclusive: first snapshot on or after this timestamp).
        #[arg(long = "from", visible_alias = "start")]
        from: Option<String>,

        /// End of snapshot timestamp range (inclusive: latest snapshot on or before this timestamp).
        #[arg(long = "to", visible_alias = "end")]
        to: Option<String>,

        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Verify backup integrity.
    Verify {
        /// Verify a specific snapshot.
        snapshot: Option<String>,

        /// Verify remote file hashes.
        #[arg(long)]
        check_hashes: bool,

        /// Download and re-hash (slow).
        #[arg(long)]
        deep: bool,

        /// Include space usage report.
        #[arg(long)]
        space: bool,
    },

    /// Remove a given directory or file(s) from all snapshots.
    #[command(alias = "rm")]
    Remove {
        /// Target file(s) or directory path(s) to remove (relative to backup root).
        #[arg(num_args = 0..)]
        paths: Vec<String>,

        /// Specific path to remove (repeatable).
        #[arg(long = "path")]
        path: Vec<String>,

        /// Break stale lock and run if locked.
        #[arg(long)]
        force: bool,

        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Delete all snapshots up to and including a given cutoff timestamp.
    #[command(name = "remove_snapshots", visible_alias = "remove-snapshots")]
    RemoveSnapshots {
        /// End of snapshot timestamp range (inclusive: all snapshots on or before this timestamp are deleted).
        #[arg(long = "to", visible_alias = "end")]
        to: String,

        /// Break stale lock and run if locked.
        #[arg(long)]
        force: bool,

        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Consolidate multiple snapshots within a date range into a single named snapshot.
    ///
    /// Combines intermediate snapshots into a single consolidated snapshot without data loss,
    /// using reverse-incremental dependency resolution and the earliest-diff rule.
    ///
    /// Timestamp arguments (--from and --to) must be in ISO-8601 format with an explicit
    /// timezone offset matching the configured backup timezone.
    #[command(
        after_help = "EXAMPLES:\n  \
          # Compact snapshots in UTC timezone (matching --timezone utc):\n  \
          incr-backup compact \\\n    \
            --from 2026-10-28T00:00:00Z \\\n    \
            --to 2026-10-28T23:59:59Z \\\n    \
            --name daily-2026-10-28\n\n  \
          # Compact snapshots in local timezone (matching --timezone local, e.g. +05:30):\n  \
          incr-backup compact \\\n    \
            --from 2026-10-28T00:00:00+05:30 \\\n    \
            --to 2026-10-28T23:59:59+05:30 \\\n    \
            --name daily-2026-10-28\n\n  \
          # Dry-run preview with high-level summary of kept and discarded diffs:\n  \
          incr-backup compact \\\n    \
            --from 2026-10-28T00:00:00Z \\\n    \
            --to 2026-10-28T23:59:59Z \\\n    \
            --name daily-2026-10-28 \\\n    \
            --dry-run --summary\n\n  \
          # Dynamic date substitution for cron / nightly consolidation:\n  \
          incr-backup compact \\\n    \
            --from \"$(date -v-1d +%Y-%m-%dT00:00:00%z)\" \\\n    \
            --to \"$(date -v-1d +%Y-%m-%dT23:59:59%z)\" \\\n    \
            --name \"daily-$(date -v-1d +%Y-%m-%d)\""
    )]
    Compact {
        /// Start of snapshot timestamp range in ISO-8601 format with explicit timezone offset.
        #[arg(
            long = "from",
            help = "Start of snapshot timestamp range (ISO-8601, e.g. 2026-10-28T00:00:00Z or 2026-10-28T00:00:00+05:30)",
            long_help = "Start of snapshot timestamp range in ISO-8601 format with explicit timezone offset.\n\nExamples:\n  UTC:   2026-10-28T00:00:00Z or 2026-10-28T00:00:00+00:00\n  Local: 2026-10-28T00:00:00+05:30 or 2026-10-28T00:00:00-08:00"
        )]
        from: String,

        /// End of snapshot timestamp range in ISO-8601 format with explicit timezone offset.
        #[arg(
            long = "to",
            help = "End of snapshot timestamp range (ISO-8601, e.g. 2026-10-28T23:59:59Z or 2026-10-28T23:59:59+05:30)",
            long_help = "End of snapshot timestamp range in ISO-8601 format with explicit timezone offset.\n\nExamples:\n  UTC:   2026-10-28T23:59:59Z or 2026-10-28T23:59:59+00:00\n  Local: 2026-10-28T23:59:59+05:30 or 2026-10-28T23:59:59-08:00"
        )]
        to: String,

        /// Name for the consolidated snapshot.
        #[arg(long = "name")]
        name: String,

        /// Show high-level summary of changes.
        #[arg(long = "summary")]
        summary: bool,

        /// Show detailed file-by-file plan.
        #[arg(long = "full-list")]
        full_list: bool,

        /// Break stale lock and run if locked.
        #[arg(long = "force")]
        force: bool,
    },

    /// Garbage collection: identify and clean up unreferenced or stale remote data.
    Gc {
        /// Delete orphaned diff directories that have no corresponding snapshot manifests.
        #[arg(long = "orphaned-diffs")]
        orphaned_diffs: bool,

        /// Break stale lock and run if locked.
        #[arg(long)]
        force: bool,

        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Manage configuration between local file and remote metadata.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand, Debug)]
enum ConfigAction {
    /// Push local configuration to remote metadata (.meta/config.json).
    Push {
        /// Force update even if remote lock is held.
        #[arg(long)]
        force: bool,
    },

    /// Pull remote metadata (.meta/config.json) into local configuration file.
    Pull {
        /// Destination path for the local config file (defaults to current --config path).
        #[arg(long)]
        to: Option<PathBuf>,

        /// Overwrite local config file if it already exists.
        #[arg(long)]
        overwrite: bool,
    },

    /// Show differences between local configuration and remote metadata.
    Diff,
}

fn main() {
    let cli = Cli::parse();

    // Early timezone detection so log timestamps use the configured timezone immediately
    let detected_tz = cli
        .timezone
        .or_else(|| {
            std::env::var("INCR_BACKUP_TIMEZONE")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .or_else(|| {
            if cli.config.exists() {
                std::fs::read_to_string(&cli.config)
                    .ok()
                    .and_then(|c| Config::from_toml_str(&c).ok())
                    .map(|c| c.timezone)
            } else {
                None
            }
        })
        .unwrap_or_default();
    incr_backup::config::set_configured_timezone(detected_tz);

    // Initialize logging based on verbosity
    let log_level = match cli.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let mut builder = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(log_level),
    );
    builder.format(|buf, record| {
        use std::io::Write;
        let ts = incr_backup::config::get_configured_timezone().format_log_timestamp();
        let style = buf.default_level_style(record.level());
        writeln!(
            buf,
            "[{} {style}{:5}{style:#} {}] {}",
            ts,
            record.level(),
            record.target(),
            record.args()
        )
    });
    builder.init();

    // Execute the command
    if let Err(e) = run_command(cli) {
        error!("{}", e);
        // Print user-friendly error message
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}

fn run_command(cli: Cli) -> incr_backup::error::Result<()> {
    let dry_run = cli.dry_run;
    let config = load_config(&cli)?;

    match cli.command {
        Commands::Init {
            source,
            remote,
            adopt,
            download_to_hash,
            exclude,
        } => {
            cmd_init(config, source, remote, adopt, download_to_hash, exclude)?;
        }

        Commands::Sync {
            name,
            force,
        } => {
            cmd_sync(config, name, force)?;
        }

        Commands::List { from, to, json } => {
            cmd_list(config, from, to, json)?;
        }

        Commands::Status => {
            cmd_status(config)?;
        }

        Commands::Restore {
            snapshot,
            destination,
            subpath,
            verify,
            overwrite,
        } => {
            cmd_restore(config, &snapshot, destination, subpath, verify, overwrite)?;
        }

        Commands::Diff {
            snapshot1,
            snapshot2,
            json,
            only_added,
            only_deleted,
            only_modified,
            stat,
        } => {
            cmd_diff(config, &snapshot1, &snapshot2, json, only_added, only_deleted, only_modified, stat)?;
        }

        Commands::Changelog { from, to, json } => {
            cmd_changelog(config, from, to, json)?;
        }

        Commands::Verify {
            snapshot,
            check_hashes,
            deep,
            space,
        } => {
            cmd_verify(config, snapshot, check_hashes, deep, space)?;
        }

        Commands::Remove {
            paths,
            path,
            force,
            json,
        } => {
            cmd_remove(config, paths, path, force, json)?;
        }

        Commands::RemoveSnapshots { to, force, json } => {
            cmd_remove_snapshots(config, to, force, json)?;
        }

        Commands::Compact {
            from,
            to,
            name,
            summary,
            full_list,
            force,
        } => {
            cmd_compact(config, from, to, name, summary, full_list, force)?;
        }

        Commands::Gc {
            orphaned_diffs,
            force,
            json,
        } => {
            cmd_gc(config, orphaned_diffs, force, json)?;
        }

        Commands::Config { action } => {
            cmd_config(config, action, &cli.config, dry_run)?;
        }
    }

    Ok(())
}

/// Load config from file with CLI overrides applied.
fn load_config(cli: &Cli) -> incr_backup::error::Result<Config> {
    let config = if cli.config.exists() {
        Config::load(&cli.config)?
    } else {
        info!("Config file not found at {}, using defaults", cli.config.display());
        Config::default_config()
    };

    let overrides = CliOverrides {
        source: cli.source.clone(),
        remote: cli.remote.clone(),
        rclone_path: cli.rclone_path.clone(),
        rclone_flags: cli.rclone_flags.clone(),
        verbose: Some(cli.verbose),
        dry_run: Some(cli.dry_run),
        timezone: cli.timezone,
    };

    let config = config.with_overrides(&overrides);
    incr_backup::config::set_configured_timezone(config.timezone);
    Ok(config)
}

// ── Command Implementations ─────────────────────────────────────────────────

fn step_is_done(journal: &Journal, step_name: &str) -> bool {
    journal
        .steps
        .iter()
        .any(|s| s.name == step_name && s.status == StepStatus::Done)
}

fn cmd_init(
    mut config: Config,
    source: Option<String>,
    remote: Option<String>,
    adopt: bool,
    download_to_hash: bool,
    exclude: Vec<String>,
) -> incr_backup::error::Result<()> {
    let t_inspect = Instant::now();

    if let Some(s) = source {
        config.source_path = s;
    }
    if let Some(r) = remote {
        config.remote_path = r;
    }

    if config.source_path.is_empty() {
        return Err(IncrError::InvalidConfig(
            "source path is required. Set [source].path in config, --source flag, or INCR_BACKUP_SOURCE env var".to_string(),
        ));
    }

    if config.remote_path.is_empty() {
        return Err(IncrError::InvalidConfig(
            "remote path is required. Set [remote].path in config, --remote flag, or INCR_BACKUP_REMOTE env var".to_string(),
        ));
    }

    if !exclude.is_empty() {
        config.exclude.patterns = exclude;
    }

    // Validate source
    let source_path = Path::new(&config.source_path);
    if !source_path.exists() {
        return Err(IncrError::SourceNotFound(source_path.to_path_buf()));
    }

    let remote_base = config.remote_path.trim_end_matches('/').to_string();
    let rclone = Rclone::new(&config);

    // Inspect remote state
    let root_entries = rclone.list_root_entries(&remote_base)?;

    let has_current = root_entries
        .iter()
        .any(|e| e.path == "current" || e.name == "current");
    let has_meta = root_entries
        .iter()
        .any(|e| e.path == ".meta" || e.name == ".meta");
    let has_diffs = root_entries
        .iter()
        .any(|e| e.path == "diffs" || e.name == "diffs");
    let is_initialized = has_current && has_meta && has_diffs;

    let journal_mgr = JournalManager::new(&rclone, &remote_base);
    let active_adopt_journal = match journal_mgr.load_journal() {
        Ok(Some(j)) if j.operation == "init_adopt" => Some(j),
        _ => None,
    };

    // Scenario 2: Remote is already initialized (NO-OP, ignore --adopt)
    if is_initialized && active_adopt_journal.is_none() {
        println!(
            "[INFO] Remote already initialized at {}; nothing to do.",
            config.remote_path
        );
        let stray_entries: Vec<&str> = root_entries
            .iter()
            .map(|e| e.path.as_str())
            .filter(|name| *name != "current" && *name != ".meta" && *name != "diffs")
            .collect();
        if !stray_entries.is_empty() {
            eprintln!(
                "[WARN] Unmanaged root files/directories detected in '{}' ({}). Please manually delete or move them out.",
                config.remote_path,
                stray_entries.join(", ")
            );
        }
        return Ok(());
    }

    // Scenario: 'current/' directory exists without an active incomplete adopt journal
    if has_current && active_adopt_journal.is_none() {
        return Err(IncrError::AdoptCurrentAlreadyExists {
            remote: config.remote_path.clone(),
        });
    }

    // Scenario 3: Remote is non-empty, missing backup structure, and --adopt was NOT passed
    if !root_entries.is_empty() && !adopt && active_adopt_journal.is_none() {
        return Err(IncrError::RemoteNotEmptyNeedsAdopt {
            remote: config.remote_path.clone(),
        });
    }

    let is_resuming = active_adopt_journal.is_some();
    let is_adopting = is_resuming || (!root_entries.is_empty() && adopt);

    if let Some(ref j) = active_adopt_journal {
        let ts = config.timezone.format_message_datetime(j.started_at);
        println!("[INFO] Found incomplete adopt journal from {ts}. Resuming adoption...");
    }

    if is_adopting {
        println!("[WARN] Adopting existing remote contents into 'current/'...");
    }

    println!("Initializing backup:");
    println!("  Source:    {}", config.source_path);
    println!("  Remote:    {}", config.remote_path);

    if config.dry_run {
        println!("[DRY-RUN] Would create remote directory structure");
        return Ok(());
    }

    let total_start = Instant::now();
    println!(
        "[1/5] Inspecting remote status... ({:.1}s)",
        t_inspect.elapsed().as_secs_f64()
    );

    if is_adopting {
        if !is_resuming {
            // Write WAL journal before moving root items
            journal_mgr.create_adopt_journal()?;
            // Create current/
            rclone.mkdir(&format!("{remote_base}/current/"))?;
        }

        // [2/5] Move root entries into current/ (DirMove)
        let t_step2 = Instant::now();
        println!("[2/5] Adopting root entries into 'current/'...");

        let move_done = active_adopt_journal
            .as_ref()
            .map(|j| step_is_done(j, "move_root_items"))
            .unwrap_or(false);

        if !move_done {
            journal_mgr.update_step("move_root_items", StepStatus::Running)?;
            println!("      Adopting existing files (server-side move, zero bandwidth)...");

            let current_root = rclone.list_root_entries(&remote_base)?;
            let mut moved_dirs = 0;
            let mut moved_files = 0;

            for item in &current_root {
                let name = item.path.trim_end_matches('/');
                if name == "current" || name == ".meta" || name == "diffs" {
                    continue;
                }
                let src = format!("{remote_base}/{name}");
                let dst = format!("{remote_base}/current/{name}");
                if item.is_dir {
                    rclone.move_dir(&src, &dst, &[], true)?;
                    moved_dirs += 1;
                } else {
                    rclone.moveto(&src, &dst)?;
                    moved_files += 1;
                }
            }

            journal_mgr.update_step("move_root_items", StepStatus::Done)?;
            println!(
                "      - Moved {} directories and {} files (DirMove)... ({:.1}s)",
                moved_dirs,
                moved_files,
                t_step2.elapsed().as_secs_f64()
            );
        } else {
            println!(
                "      - Root entries already moved in previous run ({:.1}s)",
                t_step2.elapsed().as_secs_f64()
            );
        }

        // [3/5] Create remote structure and config
        let t_step3 = Instant::now();
        let create_struct_done = active_adopt_journal
            .as_ref()
            .map(|j| step_is_done(j, "create_structure"))
            .unwrap_or(false);

        if !create_struct_done {
            journal_mgr.update_step("create_structure", StepStatus::Running)?;
            rclone.mkdir(&format!("{remote_base}/diffs/"))?;
            rclone.mkdir(&format!("{remote_base}/.meta/"))?;
            rclone.mkdir(&format!("{remote_base}/.meta/manifests/"))?;
            journal_mgr.update_step("create_structure", StepStatus::Done)?;
        }

        let upload_config_done = active_adopt_journal
            .as_ref()
            .map(|j| step_is_done(j, "upload_config"))
            .unwrap_or(false);

        if !upload_config_done {
            journal_mgr.update_step("upload_config", StepStatus::Running)?;
            let meta_path = format!("{remote_base}/.meta/config.json");
            let remote_config = config.to_remote_config();
            let config_json = remote_config.to_json()?;
            rclone.rcat(&meta_path, &config_json)?;
            journal_mgr.update_step("upload_config", StepStatus::Done)?;
        }
        println!(
            "[3/5] Creating remote structure and config... ({:.1}s)",
            t_step3.elapsed().as_secs_f64()
        );

        // [4/5] Scan adopted files for manifest (fast-list)
        let t_step4 = Instant::now();
        println!("[4/5] Scanning adopted files for manifest (fast-list)...");

        let gen_manifest_done = active_adopt_journal
            .as_ref()
            .map(|j| step_is_done(j, "generate_manifest"))
            .unwrap_or(false);

        if !gen_manifest_done {
            journal_mgr.update_step("generate_manifest", StepStatus::Running)?;
        }

        let current_path = format!("{remote_base}/current/");
        let lsjson = rclone.lsjson_recursive_hash(&current_path)?;
        let mut entries = Manifest::parse_rclone_lsjson(&lsjson, "current")?;

        let missing_count = entries
            .iter()
            .filter(|e| {
                !e.is_dir
                    && e.hashes
                        .as_ref()
                        .and_then(|h| h.md5.as_deref())
                        .unwrap_or("")
                        .is_empty()
            })
            .count();

        if missing_count > 0 {
            if !download_to_hash {
                return Err(IncrError::MissingPrecomputedHashes {
                    remote: config.remote_path.clone(),
                    missing_count,
                });
            }

            println!(
                "      Downloading/streaming {missing_count} files to compute MD5 hashes..."
            );
            for e in &mut entries {
                if !e.is_dir {
                    let has_md5 = e
                        .hashes
                        .as_ref()
                        .and_then(|h| h.md5.as_deref())
                        .map(|s| !s.is_empty())
                        .unwrap_or(false);
                    if !has_md5 {
                        let remote_file = format!("{remote_base}/current/{}", e.path);
                        let computed_md5 = rclone.compute_remote_md5(&remote_file)?;
                        e.hashes = Some(incr_backup::manifest::RcloneHashes {
                            md5: Some(computed_md5),
                        });
                    }
                }
            }
        }

        let now = chrono::Utc::now();
        let manifest = Manifest::from_entries(entries, "current", &now);

        if !gen_manifest_done {
            journal_mgr.update_step("generate_manifest", StepStatus::Done)?;
        }
        println!(
            "      - Scanned {} files ({:.1}s)",
            manifest.file_count,
            t_step4.elapsed().as_secs_f64()
        );

        // [5/5] Generating and committing initial manifest
        let t_step5 = Instant::now();
        let upload_manifest_done = active_adopt_journal
            .as_ref()
            .map(|j| step_is_done(j, "upload_manifest"))
            .unwrap_or(false);

        if !upload_manifest_done {
            journal_mgr.update_step("upload_manifest", StepStatus::Running)?;
            let manifest_json = manifest.to_json()?;
            rclone.rcat(
                &format!("{remote_base}/.meta/manifests/current.json"),
                &manifest_json,
            )?;
            journal_mgr.update_step("upload_manifest", StepStatus::Done)?;
        }
        println!(
            "[5/5] Generating and committing initial manifest... ({:.1}s)",
            t_step5.elapsed().as_secs_f64()
        );

        // Cleanup journal
        journal_mgr.cleanup()?;

        println!("✓ Adopted {} files into current/", manifest.file_count);
        println!(
            "✓ Successfully adopted {} files ({}) in {:.1}s.",
            manifest.file_count,
            format_size(manifest.total_size),
            total_start.elapsed().as_secs_f64()
        );
        println!("✓ Initialized {}", config.remote_path);
        println!(
            "  {} files indexed ({})",
            manifest.file_count,
            format_size(manifest.total_size)
        );
        println!("  Ready for incremental backups.");
    } else {
        println!("[2/5] Skipping adoption (empty remote) (0.0s)");

        let t_step3 = Instant::now();
        println!("Creating remote directory structure...");
        rclone.mkdir(&format!("{remote_base}/current/"))?;
        rclone.mkdir(&format!("{remote_base}/diffs/"))?;
        rclone.mkdir(&format!("{remote_base}/.meta/"))?;
        rclone.mkdir(&format!("{remote_base}/.meta/manifests/"))?;

        let meta_path = format!("{remote_base}/.meta/config.json");
        let remote_config = config.to_remote_config();
        let config_json = remote_config.to_json()?;
        rclone.rcat(&meta_path, &config_json)?;
        println!(
            "[3/5] Creating remote structure and config... ({:.1}s)",
            t_step3.elapsed().as_secs_f64()
        );

        let t_step4 = Instant::now();
        println!("Scanning remote for initial manifest...");
        let current_path = format!("{remote_base}/current/");
        let lsjson = rclone.lsjson_recursive_hash(&current_path)?;
        let now = chrono::Utc::now();
        let manifest = Manifest::from_rclone_lsjson(&lsjson, "current", &now)?;
        println!(
            "[4/5] Scanning remote for initial manifest... ({:.1}s)",
            t_step4.elapsed().as_secs_f64()
        );

        let t_step5 = Instant::now();
        let manifest_json = manifest.to_json()?;
        rclone.rcat(
            &format!("{remote_base}/.meta/manifests/current.json"),
            &manifest_json,
        )?;
        println!(
            "[5/5] Generating and committing initial manifest... ({:.1}s)",
            t_step5.elapsed().as_secs_f64()
        );

        println!("✓ Initialized {}", config.remote_path);
        println!(
            "  {} files indexed ({})",
            manifest.file_count,
            format_size(manifest.total_size)
        );
        println!("  Ready for incremental backups.");
    }

    Ok(())
}

fn cmd_sync(
    config: Config,
    name: Option<String>,
    force: bool,
) -> incr_backup::error::Result<()> {
    config.validate()?;

    if config.dry_run {
        let now = chrono::Utc::now();
        let snapshot_name = match &name {
            Some(n) => n.clone(),
            None => config.timezone.format_datetime(now),
        };
        let backup_dir = format!("diffs/{snapshot_name}");
        let remote_current = format!("{}/current/", config.remote_path);
        let remote_backup_dir = format!("{}/{}", config.remote_path, backup_dir);
        println!("[DRY-RUN] Would sync {} → {}", config.source_path, remote_current);
        println!("[DRY-RUN] Backup dir: {}", remote_backup_dir);
        return Ok(());
    }

    let rclone = Rclone::new(&config);
    let pipeline = incr_backup::sync::SyncPipeline::new(&config, &rclone);
    let options = incr_backup::sync::SyncOptions {
        name,
        force_lock: force,
        dry_run: false,
    };

    let result = pipeline.run(&options)?;

    println!(
        "✓ Sync complete: {} ({} files, {}, {:.1}s)",
        result.snapshot_name,
        result.files_changed,
        format_size(result.bytes_transferred),
        result.duration.as_secs_f64(),
    );

    Ok(())
}

fn cmd_list(
    config: Config,
    from: Option<String>,
    to: Option<String>,
    json_output: bool,
) -> incr_backup::error::Result<()> {
    let rclone = Rclone::new(&config);
    let remote_base = &config.remote_path;

    let mut all_summaries = incr_backup::catalog::load_snapshot_catalog(&rclone, remote_base)
        .unwrap_or_default();
    all_summaries.sort_by_key(|s| s.created_at);

    let filtered = incr_backup::changelog::filter_snapshots_by_bounds(
        &all_summaries,
        from.as_deref(),
        to.as_deref(),
    )?;

    if json_output {
        let snapshots: Vec<_> = filtered
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "timestamp": s.created_at.to_rfc3339(),
                    "dir_name": s.diff_dir,
                    "files": s.file_count,
                    "size": s.total_size,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&snapshots)?);
    } else {
        println!("Snapshots at {}:\n", remote_base);
        if filtered.is_empty() {
            println!("  (no snapshots yet)");
        } else {
            for snap in filtered {
                let display_time = config.timezone.format_display_datetime(snap.created_at);
                println!(
                    "  {} ({} files, {}, {})",
                    snap.name,
                    snap.file_count,
                    format_size(snap.total_size),
                    display_time,
                );
            }
        }
        println!("\nTotal: {} snapshots", filtered.len());
    }

    Ok(())
}

fn cmd_compact(
    config: Config,
    from: String,
    to: String,
    name: String,
    summary: bool,
    full_list: bool,
    force: bool,
) -> incr_backup::error::Result<()> {
    let rclone = Rclone::new(&config);
    let options = incr_backup::compact::CompactOptions {
        from,
        to,
        name,
        summary,
        full_list,
        dry_run: config.dry_run,
        force_lock: force,
    };

    let result = incr_backup::compact::run_compact(&config, &rclone, &options)?;

    if !config.dry_run {
        println!(
            "✓ Compaction complete: {} ({} snapshots consolidated, {} diff files kept [{}], {} discarded [{} reclaimed], {:.1}s)",
            result.name,
            result.snapshots_consolidated,
            result.files_kept,
            format_size(result.bytes_kept),
            result.files_discarded,
            format_size(result.bytes_discarded),
            result.duration.as_secs_f64(),
        );
    }

    Ok(())
}

fn cmd_status(config: Config) -> incr_backup::error::Result<()> {
    let rclone = Rclone::new(&config);
    let remote_base = &config.remote_path;

    println!("Backup status for {}\n", remote_base);
    println!("Source:     {}", config.source_path);
    println!("Remote:     {}", remote_base);

    // Check lock
    let lock_mgr = incr_backup::lock::LockManager::new(&rclone, remote_base, config.timezone);
    match lock_mgr.check()? {
        Some(lock) => {
            let stale = if incr_backup::lock::is_stale(&lock) {
                " (STALE)"
            } else {
                ""
            };
            println!(
                "Lock:       {} (pid {}, since {}){}",
                lock.holder,
                lock.pid,
                config.timezone.format_message_datetime(lock.started_at),
                stale
            );
        }
        None => println!("Lock:       (none)"),
    }

    // Space usage
    if let Ok(size) = rclone.size(&format!("{}/current/", remote_base)) {
        println!("Current:    {} files, {}", size.count, format_size(size.bytes));
    }
    if let Ok(size) = rclone.size(&format!("{}/diffs/", remote_base)) {
        println!("Diffs:      {} files, {}", size.count, format_size(size.bytes));
    }
    if let Ok(mut diff_dirs) = rclone.lsd(&format!("{}/diffs/", remote_base)) {
        diff_dirs.sort();
        let dir_sizes: Vec<(String, incr_backup::error::Result<incr_backup::rclone::SizeOutput>)> = diff_dirs
            .par_iter()
            .map(|dir| {
                let size = rclone.size(&format!("{}/diffs/{}/", remote_base, dir));
                (dir.clone(), size)
            })
            .collect();

        for (dir, size_res) in dir_sizes {
            if let Ok(size) = size_res {
                let tag = if incr_backup::snapshot::is_tombstone_diff_dir(&dir) {
                    " [pending GC]"
                } else {
                    ""
                };
                println!("  {}: {} files, {}{tag}", dir, size.count, format_size(size.bytes));
            }
        }
    }

    Ok(())
}

fn cmd_restore(
    config: Config,
    snapshot: &str,
    destination: PathBuf,
    subpath: Option<String>,
    verify: bool,
    overwrite: bool,
) -> incr_backup::error::Result<()> {
    let rclone = Rclone::new(&config);
    let options = RestoreOptions {
        snapshot: snapshot.to_string(),
        destination: destination.clone(),
        subpath,
        verify_after: verify,
        overwrite,
        dry_run: config.dry_run,
    };

    let result = run_restore(&config, &rclone, &options)?;

    println!(
        "✓ Restored {} files ({}) to {} in {:.1}s",
        result.files_restored,
        format_size(result.bytes_restored),
        destination.display(),
        result.duration.as_secs_f64(),
    );

    if let Some(passed) = result.verify_passed {
        if passed {
            println!("  Verification: PASSED ✓");
        } else {
            println!("  Verification: FAILED ✗");
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_diff(
    config: Config,
    snapshot_a: &str,
    snapshot_b: &str,
    json: bool,
    only_added: bool,
    only_deleted: bool,
    only_modified: bool,
    stat: bool,
) -> incr_backup::error::Result<()> {
    let rclone = Rclone::new(&config);
    let options = DiffOptions {
        snapshot_a: snapshot_a.to_string(),
        snapshot_b: snapshot_b.to_string(),
        json_output: json,
        only_added,
        only_deleted,
        only_modified,
        stat_only: stat,
    };

    let output = run_diff(&config, &rclone, &options)?;
    println!("{}", output);

    Ok(())
}

fn cmd_changelog(
    config: Config,
    from: Option<String>,
    to: Option<String>,
    json: bool,
) -> incr_backup::error::Result<()> {
    let rclone = Rclone::new(&config);
    let options = ChangelogOptions { from, to, json };
    let result = run_changelog(&config, &rclone, &options)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print!("{}", format_changelog(&result, config.timezone));
    }

    Ok(())
}

fn cmd_verify(
    config: Config,
    snapshot: Option<String>,
    check_hashes: bool,
    deep: bool,
    space: bool,
) -> incr_backup::error::Result<()> {
    let rclone = Rclone::new(&config);
    let options = VerifyOptions {
        snapshot,
        check_hashes,
        deep,
        show_space: space,
    };

    let result = run_verify(&config, &rclone, &options)?;
    let output = format_verify_result(&result, &config.remote_path);
    println!("{}", output);

    if !result.all_passed {
        process::exit(1);
    }

    Ok(())
}

fn cmd_remove(
    config: Config,
    mut paths: Vec<String>,
    path_flags: Vec<String>,
    force: bool,
    json: bool,
) -> incr_backup::error::Result<()> {
    paths.extend(path_flags);
    if paths.is_empty() {
        return Err(IncrError::InvalidTarget(
            "no target path(s) specified. Usage: incr-backup remove <PATH>...".to_string(),
        ));
    }

    let rclone = Rclone::new(&config);
    let options = RemoveOptions {
        targets: paths,
        force_lock: force,
        dry_run: config.dry_run,
        json,
    };

    let result = run_remove(&config, &rclone, &options)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if result.dry_run {
        println!(
            "[DRY-RUN] Simulating removal of target(s) across all snapshots at {}:\n  Targets: {}",
            config.remote_path,
            result.targets.join(", ")
        );
        if result.snapshots_affected.is_empty() {
            println!("\n[DRY-RUN] No matching files found in any snapshot.");
        } else {
            println!("\n[DRY-RUN] Affected snapshot manifests:");
            for detail in &result.details {
                println!(
                    "  - {}: would remove {} file(s) ({})",
                    detail.snapshot,
                    detail.files_removed,
                    format_size(detail.bytes_removed)
                );
            }
            println!(
                "\n[DRY-RUN] Summary: {} unique file(s) ({} total instances, {}) would be removed across {} snapshot(s).",
                result.unique_files_removed,
                result.total_file_instances_removed,
                format_size(result.total_bytes_removed),
                result.snapshots_affected.len()
            );
        }
    } else {
        println!(
            "Removing target(s) from all snapshots at {}:\n  Targets: {}",
            config.remote_path,
            result.targets.join(", ")
        );

        if result.snapshots_affected.is_empty() {
            println!("\nNo matching files found across snapshots.");
        } else {
            println!("\nManifest updates:");
            for detail in &result.details {
                println!(
                    "  ✓ {}: removed {} file(s) ({})",
                    detail.snapshot,
                    detail.files_removed,
                    format_size(detail.bytes_removed)
                );
            }

            println!("\nRemote storage cleaned:");
            println!("  ✓ current/");
            for snap in &result.snapshots_affected {
                if snap != "current" {
                    println!("  ✓ diffs/{}/", snap);
                }
            }

            println!(
                "\n✓ Successfully removed {} unique file(s) ({} instances across {} snapshot(s), {} freed).",
                result.unique_files_removed,
                result.total_file_instances_removed,
                result.snapshots_affected.len(),
                format_size(result.total_bytes_removed)
            );
            println!("\nTip: If you do not want this file/directory backed up in future syncs, add it to 'exclude.patterns' in incr-backup.toml.");
        }
    }

    Ok(())
}

fn cmd_remove_snapshots(
    config: Config,
    to: String,
    force: bool,
    json: bool,
) -> incr_backup::error::Result<()> {
    let rclone = Rclone::new(&config);
    let options = RemoveSnapshotsOptions {
        to,
        force_lock: force,
        dry_run: config.dry_run,
        json,
    };

    let result = run_remove_snapshots(&config, &rclone, &options)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if result.dry_run {
        println!(
            "[DRY-RUN] Simulating removal of snapshot(s) up to '{}' at {}:\n  Resolved cutoff: {}",
            result.to,
            config.remote_path,
            config.timezone.format_display_datetime(result.to_timestamp)
        );
        if result.snapshots_removed == 0 {
            println!("\n[DRY-RUN] No matching snapshots found on or before cutoff.");
        } else {
            println!("\n[DRY-RUN] Snapshots that would be deleted:");
            for snap in &result.snapshots {
                println!("  - {snap}");
            }
            if !result.diff_dirs_purged.is_empty() {
                println!("\n[DRY-RUN] Diff directories that would be purged:");
                for dir in &result.diff_dirs_purged {
                    println!("  * diffs/{dir}/");
                }
            }
            println!(
                "\n[DRY-RUN] Summary: {} snapshot(s) would be deleted. {} snapshot(s) remaining.",
                result.snapshots_removed, result.snapshots_remaining
            );
        }
    } else {
        println!(
            "Removing snapshot(s) up to '{}' at {}:",
            result.to, config.remote_path
        );

        if result.snapshots_removed == 0 {
            println!("\nNo matching snapshots found on or before cutoff.");
        } else {
            println!("\nDeleted snapshots:");
            for snap in &result.snapshots {
                println!("  ✓ {snap}");
            }

            if !result.diff_dirs_purged.is_empty() {
                println!("\nPurged diff directories:");
                for dir in &result.diff_dirs_purged {
                    println!("  ✓ diffs/{dir}/");
                }
            }

            println!(
                "\n✓ Successfully deleted {} snapshot(s). Remaining snapshots: {}.",
                result.snapshots_removed, result.snapshots_remaining
            );
        }
    }

    Ok(())
}

fn cmd_gc(
    config: Config,
    orphaned_diffs: bool,
    force: bool,
    json: bool,
) -> incr_backup::error::Result<()> {
    let rclone = Rclone::new(&config);
    let options = GcOptions {
        orphaned_diffs,
        force_lock: force,
        dry_run: config.dry_run,
        json,
    };

    let result = run_gc(&config, &rclone, &options)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if result.dry_run {
        println!(
            "[DRY-RUN] Simulating garbage collection at {}:",
            config.remote_path
        );
        if result.total_orphaned_dirs == 0 {
            println!("\n[DRY-RUN] No orphaned diff directories found. All diff directories correspond to active snapshot manifests.");
        } else {
            println!("\n[DRY-RUN] Orphaned diff directories that would be purged:");
            for diff in &result.orphaned_diffs {
                println!(
                    "  * {} ({} files, {})",
                    diff.path,
                    diff.file_count,
                    format_size(diff.size_bytes)
                );
            }
            println!(
                "\n[DRY-RUN] Summary: {} orphaned diff directory/ies would be purged ({} files, {} freed).",
                result.total_orphaned_dirs,
                result.total_files,
                format_size(result.total_bytes)
            );
        }
    } else {
        println!(
            "Garbage collection at {}:",
            config.remote_path
        );
        if result.total_orphaned_dirs == 0 {
            println!("\nNo orphaned diff directories found. All diff directories correspond to active snapshot manifests.");
        } else {
            println!("\nPurged orphaned diff directories:");
            for diff in &result.orphaned_diffs {
                println!(
                    "  ✓ {} ({} files, {})",
                    diff.path,
                    diff.file_count,
                    format_size(diff.size_bytes)
                );
            }
            println!(
                "\n✓ Successfully purged {} orphaned diff directory/ies ({} files, {} freed).",
                result.total_orphaned_dirs,
                result.total_files,
                format_size(result.total_bytes)
            );
        }
    }

    Ok(())
}

fn cmd_config(
    config: Config,
    action: ConfigAction,
    cli_config_path: &Path,
    dry_run: bool,
) -> incr_backup::error::Result<()> {
    match action {
        ConfigAction::Push { force } => {
            cmd_config_push(&config, force, dry_run)?;
        }
        ConfigAction::Pull { to, overwrite } => {
            cmd_config_pull(&config, to, overwrite, cli_config_path, dry_run)?;
        }
        ConfigAction::Diff => {
            cmd_config_diff(&config)?;
        }
    }
    Ok(())
}

fn cmd_config_push(
    config: &Config,
    force: bool,
    dry_run: bool,
) -> incr_backup::error::Result<()> {
    if config.remote_path.is_empty() {
        return Err(IncrError::InvalidConfig(
            "remote path is required. Specify [remote].path in config or use --remote".to_string(),
        ));
    }

    let rclone = Rclone::new(config);
    let remote_base = &config.remote_path;
    let meta_path = format!("{}/.meta/config.json", remote_base);

    // Acquire lock
    let lock_mgr = incr_backup::lock::LockManager::new(&rclone, remote_base, config.timezone);
    if !dry_run {
        lock_mgr.acquire(force)?;
    }

    let res = (|| -> incr_backup::error::Result<()> {
        // Read current remote config if present
        let current_remote: Option<incr_backup::config::RemoteConfig> = rclone
            .cat(&meta_path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok());

        let target_remote = config.to_remote_config();

        if let Some(ref cur) = current_remote {
            if cur == &target_remote {
                println!("Remote configuration (.meta/config.json) is already identical to local configuration.");
                return Ok(());
            }

            println!("Updating remote metadata at {}:", meta_path);
            if cur.source_path != target_remote.source_path {
                println!("  Source:    {} → {}", cur.source_path, target_remote.source_path);
            }
            if cur.exclude_patterns != target_remote.exclude_patterns {
                println!("  Excludes:  {:?} → {:?}", cur.exclude_patterns, target_remote.exclude_patterns);
            }
        } else {
            println!("Creating remote metadata at {}:", meta_path);
            println!("  Source:    {}", target_remote.source_path);
            if !target_remote.exclude_patterns.is_empty() {
                println!("  Excludes:  {:?}", target_remote.exclude_patterns);
            }
        }

        if dry_run {
            println!("[DRY-RUN] Would upload updated configuration to {}", meta_path);
            return Ok(());
        }

        let json = target_remote.to_json()?;
        rclone.rcat(&meta_path, &json)?;

        println!("✓ Successfully pushed local configuration to remote metadata.");
        Ok(())
    })();

    if !dry_run {
        let _ = lock_mgr.release();
    }

    res
}

fn cmd_config_pull(
    config: &Config,
    to: Option<PathBuf>,
    overwrite: bool,
    cli_config_path: &Path,
    dry_run: bool,
) -> incr_backup::error::Result<()> {
    if config.remote_path.is_empty() {
        return Err(IncrError::InvalidConfig(
            "remote path is required to pull configuration. Specify --remote or [remote].path in config".to_string(),
        ));
    }

    let rclone = Rclone::new(config);
    let remote_base = &config.remote_path;
    let meta_path = format!("{}/.meta/config.json", remote_base);

    let content = rclone.cat(&meta_path).map_err(|e| {
        IncrError::Config(format!(
            "failed to read remote metadata from '{}': {e}. Remote may not be initialized.",
            meta_path
        ))
    })?;

    let remote_config: incr_backup::config::RemoteConfig = serde_json::from_str(&content)
        .map_err(|e| IncrError::Config(format!("failed to parse remote config JSON: {e}")))?;

    let dest = to.unwrap_or_else(|| cli_config_path.to_path_buf());

    if dest.exists() && !overwrite {
        return Err(IncrError::Config(format!(
            "local configuration file '{}' already exists. Use --overwrite to overwrite it.",
            dest.display()
        )));
    }

    let pulled_config = Config {
        source_path: remote_config.source_path.clone(),
        remote_path: remote_base.to_string(),
        rclone: config.rclone.clone(),
        exclude: incr_backup::config::ExcludeConfig {
            patterns: remote_config.exclude_patterns.clone(),
        },
        verbose: config.verbose,
        dry_run: false,
        timezone: config.timezone,
    };

    println!("Pulling configuration from {}:", meta_path);
    println!("  Source:    {}", pulled_config.source_path);
    println!("  Remote:    {}", pulled_config.remote_path);
    if !pulled_config.exclude.patterns.is_empty() {
        println!("  Excludes:  {:?}", pulled_config.exclude.patterns);
    }

    if dry_run {
        println!("[DRY-RUN] Would write configuration to '{}'", dest.display());
        return Ok(());
    }

    let toml_str = pulled_config.to_toml_string()?;
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                IncrError::Config(format!("failed to create directory '{}': {e}", parent.display()))
            })?;
        }
    }
    std::fs::write(&dest, toml_str).map_err(|e| {
        IncrError::Config(format!("failed to write config to '{}': {e}", dest.display()))
    })?;

    println!("✓ Successfully wrote configuration to '{}'", dest.display());
    Ok(())
}

fn cmd_config_diff(config: &Config) -> incr_backup::error::Result<()> {
    if config.remote_path.is_empty() {
        return Err(IncrError::InvalidConfig(
            "remote path is required. Specify [remote].path in config or use --remote".to_string(),
        ));
    }

    let rclone = Rclone::new(config);
    let remote_base = &config.remote_path;
    let meta_path = format!("{}/.meta/config.json", remote_base);

    let content = rclone.cat(&meta_path).map_err(|e| {
        IncrError::Config(format!(
            "failed to read remote metadata from '{}': {e}. Remote may not be initialized.",
            meta_path
        ))
    })?;

    let remote_config: incr_backup::config::RemoteConfig = serde_json::from_str(&content)
        .map_err(|e| IncrError::Config(format!("failed to parse remote config JSON: {e}")))?;

    let target_remote = config.to_remote_config();

    println!("Configuration comparison (Local vs Remote):");
    println!("  Remote metadata: {}", meta_path);
    println!();

    let mut differences = 0;

    if target_remote.source_path != remote_config.source_path {
        println!("  Source Path:");
        println!("    Local:  {}", target_remote.source_path);
        println!("    Remote: {}", remote_config.source_path);
        differences += 1;
    } else {
        println!("  Source Path: {} (match)", target_remote.source_path);
    }

    if target_remote.exclude_patterns != remote_config.exclude_patterns {
        println!("  Exclude Patterns: (DIFFER)");
        println!("    Local:  {:?}", target_remote.exclude_patterns);
        println!("    Remote: {:?}", remote_config.exclude_patterns);
        differences += 1;
    } else {
        println!("  Exclude Patterns: {:?} (match)", target_remote.exclude_patterns);
    }

    println!();
    if differences == 0 {
        println!("✓ Local configuration and remote metadata are identical.");
    } else {
        println!("⚠ Found {} difference(s).", differences);
        println!("  Run `incr-backup config push` to update remote metadata from local config.");
        println!("  Run `incr-backup config pull` to overwrite local config from remote metadata.");
    }

    Ok(())
}
