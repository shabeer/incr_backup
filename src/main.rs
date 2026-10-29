//! # incr-backup — CLI Entry Point
//!
//! rsnapshot-like incremental backups for rclone remotes.
//!
//! This binary uses `clap` derive macros for argument parsing and dispatches
//! to the library modules for each subcommand.

use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};
use log::{error, info};

use incr_backup::config::{CliOverrides, Config, SnapshotTimezone};
use incr_backup::diff::{DiffOptions, run_diff};
use incr_backup::error::IncrError;
use incr_backup::manifest::{format_size, Manifest};
use incr_backup::rclone::Rclone;
use incr_backup::remove::{RemoveOptions, run_remove};
use incr_backup::restore::{RestoreOptions, run_restore};
use incr_backup::verify::{VerifyOptions, format_verify_result, run_verify};

/// incr-backup — rsnapshot-like incremental backups for rclone remotes.
///
/// Backs up local directories to rclone remotes using reverse-incremental
/// snapshots. The `current/` directory on the remote is always a full
/// mirror of the source, directly browseable on Google Drive.
#[derive(Parser, Debug)]
#[command(name = "incr-backup", version, about, long_about = None)]
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
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Show backup status.
    Status,

    /// Restore a snapshot to a local directory.
    Restore {
        /// Snapshot to restore (e.g., hourly.2, daily.0, current).
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

    /// Show differences between two snapshots.
    Diff {
        /// First snapshot.
        snapshot_a: String,

        /// Second snapshot.
        snapshot_b: String,

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

    /// Consolidate multiple snapshots within a date range into a single named snapshot.
    Compact {
        /// Start of snapshot timestamp range (ISO-8601 with explicit timezone offset).
        #[arg(long = "from")]
        from: String,

        /// End of snapshot timestamp range (ISO-8601 with explicit timezone offset).
        #[arg(long = "to")]
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
            exclude,
        } => {
            cmd_init(config, source, remote, adopt, exclude)?;
        }

        Commands::Sync {
            name,
            force,
        } => {
            cmd_sync(config, name, force)?;
        }

        Commands::List { json } => {
            cmd_list(config, json)?;
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
            snapshot_a,
            snapshot_b,
            json,
            only_added,
            only_deleted,
            only_modified,
            stat,
        } => {
            cmd_diff(config, &snapshot_a, &snapshot_b, json, only_added, only_deleted, only_modified, stat)?;
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

fn cmd_init(
    mut config: Config,
    source: Option<String>,
    remote: Option<String>,
    adopt: bool,
    exclude: Vec<String>,
) -> incr_backup::error::Result<()> {
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

    let rclone = Rclone::new(&config);

    // Inspect remote state
    let root_entries = match rclone.lsf(&config.remote_path) {
        Ok(entries) => entries,
        Err(IncrError::RcloneFailed { code, ref stderr, .. })
            if code == 3 || stderr.to_lowercase().contains("directory not found") =>
        {
            Vec::new()
        }
        Err(e) => return Err(e),
    };

    let has_current = root_entries.iter().any(|e| e.trim_end_matches('/') == "current");
    let has_meta = root_entries.iter().any(|e| e.trim_end_matches('/') == ".meta");
    let has_diffs = root_entries.iter().any(|e| e.trim_end_matches('/') == "diffs");
    let is_initialized = has_current && has_meta && has_diffs;

    // Scenario 2: Remote is already initialized (NO-OP, ignore --adopt)
    if is_initialized {
        println!("[INFO] Remote already initialized at {}; nothing to do.", config.remote_path);
        let stray_entries: Vec<&str> = root_entries
            .iter()
            .map(|e| e.as_str())
            .filter(|e| {
                let name = e.trim_end_matches('/');
                name != "current" && name != ".meta" && name != "diffs"
            })
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

    // Scenario 3: Remote is non-empty, missing backup structure, and --adopt was NOT passed
    if !root_entries.is_empty() && !adopt {
        return Err(IncrError::RemoteNotEmptyNeedsAdopt {
            remote: config.remote_path.clone(),
        });
    }

    // Scenario 4 vs Scenario 1:
    // If remote is not empty and adopt is true -> adopt flow.
    // If remote is empty -> silently ignore --adopt flag.
    let is_adopting = !root_entries.is_empty() && adopt;
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

    // Create current/
    rclone.mkdir(&format!("{}/current/", config.remote_path))?;

    if is_adopting {
        // Move existing files to current/
        println!("Adopting existing files (server-side move, zero bandwidth)...");
        let move_excludes = vec![
            "current/**".to_string(),
            "diffs/**".to_string(),
            ".meta/**".to_string(),
        ];
        rclone.move_dir(
            &config.remote_path,
            &format!("{}/current/", config.remote_path),
            &move_excludes,
            true,
        )?;
    }

    // Create remaining directory structure
    println!("Creating remote directory structure...");
    rclone.mkdir(&format!("{}/diffs/", config.remote_path))?;
    rclone.mkdir(&format!("{}/.meta/", config.remote_path))?;
    rclone.mkdir(&format!("{}/.meta/manifests/", config.remote_path))?;

    // Upload config (overwriting if already present)
    let meta_path = format!("{}/.meta/config.json", config.remote_path);
    let remote_config = config.to_remote_config();
    let config_json = remote_config.to_json()?;
    rclone.rcat(&meta_path, &config_json)?;

    // Generate initial manifest by scanning current/
    println!("Scanning remote for initial manifest...");
    let current_path = format!("{}/current/", config.remote_path);
    let lsjson = rclone.lsjson_recursive_hash(&current_path)?;
    let now = chrono::Utc::now();
    let manifest = Manifest::from_rclone_lsjson(&lsjson, "current", &now)?;
    let manifest_json = manifest.to_json()?;
    rclone.rcat(
        &format!("{}/.meta/manifests/current.json", config.remote_path),
        &manifest_json,
    )?;

    if is_adopting {
        println!("✓ Adopted {} files into current/", manifest.file_count);
    }
    println!("✓ Initialized {}", config.remote_path);
    println!("  {} files indexed ({})", manifest.file_count, format_size(manifest.total_size));
    println!("  Ready for incremental backups.");

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

fn cmd_list(config: Config, json_output: bool) -> incr_backup::error::Result<()> {
    let rclone = Rclone::new(&config);
    let remote_base = &config.remote_path;

    let mut all_meta = incr_backup::compact::discover_all_snapshots(&rclone, remote_base).unwrap_or_default();
    all_meta.sort_by_key(|s| s.created_at);

    if json_output {
        let snapshots: Vec<_> = all_meta
            .iter()
            .map(|meta| {
                serde_json::json!({
                    "name": meta.name,
                    "timestamp": meta.created_at.to_rfc3339(),
                    "dir_name": meta.diff_dir,
                    "files": meta.manifest.file_count,
                    "size": meta.manifest.total_size,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&snapshots)?);
    } else {
        println!("Snapshots at {}:\n", remote_base);
        if all_meta.is_empty() {
            println!("  (no snapshots yet)");
        } else {
            for snap in &all_meta {
                let display_time = config.timezone.format_display_datetime(snap.created_at);
                println!(
                    "  {} ({} files, {}, {})",
                    snap.name,
                    snap.manifest.file_count,
                    format_size(snap.manifest.total_size),
                    display_time,
                );
            }
        }
        println!("\nTotal: {} snapshots", all_meta.len());
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
