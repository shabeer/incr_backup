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

use incr_backup::config::{CliOverrides, Config, RetentionPreset};
use incr_backup::diff::{DiffOptions, run_diff};
use incr_backup::error::IncrError;
use incr_backup::manifest::{format_size, Manifest};
use incr_backup::rclone::Rclone;
use incr_backup::remove::{RemoveOptions, run_remove};
use incr_backup::restore::{RestoreOptions, run_restore};
use incr_backup::snapshot::{RotationManager, SnapshotTier};
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

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize a new backup configuration.
    Init {
        /// Local source directory (required for init).
        #[arg(long)]
        source: String,

        /// rclone remote path (required for init).
        #[arg(long)]
        remote: String,

        /// Adopt an existing rclone-synced directory.
        #[arg(long)]
        adopt: bool,

        /// Retention preset (default, aggressive, minimal).
        #[arg(long, default_value = "default")]
        retention: String,

        /// Exclude patterns (repeatable).
        #[arg(long)]
        exclude: Vec<String>,
    },

    /// Run a sync operation.
    Sync {
        /// Retention level: hourly, daily, weekly, monthly.
        #[arg(long)]
        level: String,

        /// Always rotate even if min_interval not elapsed.
        #[arg(long)]
        force_rotate: bool,

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
}

fn main() {
    let cli = Cli::parse();

    // Initialize logging based on verbosity
    let log_level = match cli.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(log_level),
    )
    .init();

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
            retention,
            exclude,
        } => {
            cmd_init(&source, &remote, adopt, &retention, &exclude, dry_run)?;
        }

        Commands::Sync {
            level,
            force_rotate,
            force,
        } => {
            cmd_sync(config, &level, force_rotate, force)?;
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
    };

    Ok(config.with_overrides(&overrides))
}

// ── Command Implementations ─────────────────────────────────────────────────

fn cmd_init(
    source: &str,
    remote: &str,
    adopt: bool,
    retention_preset: &str,
    excludes: &[String],
    dry_run: bool,
) -> incr_backup::error::Result<()> {
    let preset = RetentionPreset::from_str(retention_preset)?;
    let retention = preset.to_retention();

    // Build a config for this init
    let config = Config {
        source_path: source.to_string(),
        remote_path: remote.to_string(),
        retention: retention.clone(),
        rclone: incr_backup::config::RcloneConfig::default(),
        exclude: incr_backup::config::ExcludeConfig {
            patterns: excludes.to_vec(),
        },
        verbose: 1,
        dry_run,
    };

    // Validate source
    let source_path = Path::new(source);
    if !source_path.exists() {
        return Err(IncrError::SourceNotFound(source_path.to_path_buf()));
    }

    let rclone = Rclone::new(&config);

    // Check if already initialized
    let meta_path = format!("{}/.meta/config.json", remote);
    if rclone.cat(&meta_path).is_ok() {
        return Err(IncrError::AlreadyInitialized {
            remote: remote.to_string(),
        });
    }

    println!("Initializing backup:");
    println!("  Source:    {}", source);
    println!("  Remote:    {}", remote);
    println!(
        "  Retention: {}h/{}d/{}w/{}m ({} preset)",
        retention.hourly, retention.daily, retention.weekly, retention.monthly, retention_preset
    );

    if dry_run {
        println!("[DRY-RUN] Would create remote directory structure");
        return Ok(());
    }

    // Create directory structure
    println!("Creating remote directory structure...");
    rclone.mkdir(&format!("{}/current/", remote))?;
    rclone.mkdir(&format!("{}/diffs/", remote))?;
    rclone.mkdir(&format!("{}/.meta/", remote))?;
    rclone.mkdir(&format!("{}/.meta/manifests/", remote))?;

    if adopt {
        // Move existing files to current/
        println!("Adopting existing files (server-side move, zero bandwidth)...");
        let move_excludes = vec![
            "current/**".to_string(),
            "diffs/**".to_string(),
            ".meta/**".to_string(),
        ];
        rclone.move_dir(remote, &format!("{}/current/", remote), &move_excludes)?;
    }

    // Upload config
    let remote_config = config.to_remote_config();
    let config_json = remote_config.to_json()?;
    rclone.rcat(&meta_path, &config_json)?;

    // Generate initial manifest by scanning current/
    println!("Scanning remote for initial manifest...");
    let current_path = format!("{}/current/", remote);
    let lsjson = rclone.lsjson_recursive_hash(&current_path)?;
    let now = chrono::Utc::now();
    let manifest = Manifest::from_rclone_lsjson(&lsjson, "current", &now)?;
    let manifest_json = manifest.to_json()?;
    rclone.rcat(
        &format!("{}/.meta/manifests/current.json", remote),
        &manifest_json,
    )?;

    println!("✓ Initialized {}", remote);
    println!("  {} files indexed ({})", manifest.file_count, format_size(manifest.total_size));
    println!("  Ready for incremental backups.");

    Ok(())
}

fn cmd_sync(
    config: Config,
    level: &str,
    force_rotate: bool,
    force: bool,
) -> incr_backup::error::Result<()> {
    config.validate()?;

    let tier = SnapshotTier::from_str(level)?;
    let rclone = Rclone::new(&config);
    let retention = &config.retention;
    let remote_base = &config.remote_path;

    let lock_mgr = incr_backup::lock::LockManager::new(&rclone, remote_base);
    let journal_mgr = incr_backup::journal::JournalManager::new(&rclone, remote_base);
    let rotation_mgr = RotationManager::new(&rclone, remote_base, retention);

    let start = std::time::Instant::now();

    // Check for existing journal
    if let Some(journal) = journal_mgr.load_journal()? {
        println!("Found interrupted operation from {}. Resuming...", journal.started_at);
    }

    // Acquire lock
    if !config.dry_run {
        lock_mgr.acquire(force)?;
    }

    // Cascade rotation for higher tiers
    if tier != SnapshotTier::Hourly {
        cascade_sync(&tier, &rotation_mgr, config.dry_run)?;
    }

    // Determine rotation vs update-in-place
    let should_rotate = if force_rotate {
        true
    } else {
        rotation_mgr.should_rotate(&tier)?
    };

    let now = chrono::Utc::now();
    let timestamp = now.format("%Y-%m-%dT%H-%M").to_string();

    let (backup_dir_name, was_update) = if should_rotate {
        if !config.dry_run {
            rotation_mgr.rotate_tier(&SnapshotTier::Hourly)?;
        }
        (format!("hourly.0__{}", timestamp), false)
    } else {
        let existing = rotation_mgr
            .get_newest_snapshot_name(&SnapshotTier::Hourly)?
            .unwrap_or_else(|| format!("hourly.0__{}", timestamp));
        (existing, true)
    };

    let backup_dir = format!("diffs/{}", backup_dir_name);
    let remote_current = format!("{}/current/", remote_base);
    let remote_backup_dir = format!("{}/{}", remote_base, backup_dir);

    if config.dry_run {
        println!("[DRY-RUN] Would sync {} → {}", config.source_path, remote_current);
        println!("[DRY-RUN] Backup dir: {}", remote_backup_dir);
    } else {
        // Create journal
        journal_mgr.create_journal("sync", level, &backup_dir)?;

        // Ensure backup dir exists on remote
        rclone.mkdir(&remote_backup_dir)?;

        // Run rclone sync
        println!("Syncing {} → {}", config.source_path, remote_current);
        rclone.sync(
            &config.source_path,
            &remote_current,
            Some(&remote_backup_dir),
            &config.exclude.patterns,
        )?;

        // Generate and upload manifest
        println!("Generating manifest...");
        let lsjson = rclone.lsjson_recursive_hash(&remote_current)?;
        let manifest = Manifest::from_rclone_lsjson(&lsjson, &backup_dir_name, &now)?;
        let manifest_json = manifest.to_json()?;

        let short_name = backup_dir_name.split("__").next().unwrap_or(&backup_dir_name);
        rclone.rcat(
            &format!("{}/.meta/manifests/{}.json", remote_base, short_name),
            &manifest_json,
        )?;
        rclone.rcat(
            &format!("{}/.meta/manifests/current.json", remote_base),
            &manifest_json,
        )?;

        // Cleanup
        journal_mgr.cleanup()?;
        lock_mgr.release()?;

        let duration = start.elapsed();
        println!(
            "✓ Sync complete: {} ({} files, {}, {:.1}s){}",
            backup_dir_name,
            manifest.file_count,
            format_size(manifest.total_size),
            duration.as_secs_f64(),
            if was_update { " [update-in-place]" } else { "" },
        );
    }

    Ok(())
}

fn cascade_sync(
    tier: &SnapshotTier,
    rotation_mgr: &RotationManager,
    dry_run: bool,
) -> incr_backup::error::Result<()> {
    match tier {
        SnapshotTier::Monthly => {
            if !dry_run {
                rotation_mgr.rotate_tier(&SnapshotTier::Monthly)?;
                rotation_mgr.promote(&SnapshotTier::Weekly, &SnapshotTier::Monthly)?;
            }
            cascade_sync(&SnapshotTier::Weekly, rotation_mgr, dry_run)?;
        }
        SnapshotTier::Weekly => {
            if !dry_run {
                rotation_mgr.rotate_tier(&SnapshotTier::Weekly)?;
                rotation_mgr.promote(&SnapshotTier::Daily, &SnapshotTier::Weekly)?;
            }
            cascade_sync(&SnapshotTier::Daily, rotation_mgr, dry_run)?;
        }
        SnapshotTier::Daily => {
            if !dry_run {
                rotation_mgr.rotate_tier(&SnapshotTier::Daily)?;
                rotation_mgr.promote(&SnapshotTier::Hourly, &SnapshotTier::Daily)?;
            }
        }
        SnapshotTier::Hourly => {}
    }
    Ok(())
}

fn cmd_list(config: Config, json_output: bool) -> incr_backup::error::Result<()> {
    let rclone = Rclone::new(&config);
    let retention = &config.retention;
    let remote_base = &config.remote_path;
    let rotation_mgr = RotationManager::new(&rclone, remote_base, retention);

    let tiers = [
        SnapshotTier::Hourly,
        SnapshotTier::Daily,
        SnapshotTier::Weekly,
        SnapshotTier::Monthly,
    ];

    if json_output {
        let mut all_snapshots = Vec::new();
        for tier in &tiers {
            let snapshots = rotation_mgr.list_snapshots(tier)?;
            for snap in snapshots {
                all_snapshots.push(serde_json::json!({
                    "tier": tier.prefix(),
                    "index": snap.name.index,
                    "timestamp": snap.name.timestamp,
                    "dir_name": snap.dir_name,
                }));
            }
        }
        println!("{}", serde_json::to_string_pretty(&all_snapshots)?);
    } else {
        println!("Snapshots at {}:\n", remote_base);
        let mut total = 0;
        for tier in &tiers {
            let snapshots = rotation_mgr.list_snapshots(tier)?;
            if !snapshots.is_empty() {
                println!("  {}:", tier.prefix());
                for snap in &snapshots {
                    println!("    {}", snap.dir_name);
                }
                total += snapshots.len();
            }
        }
        if total == 0 {
            println!("  (no snapshots yet)");
        }
        println!("\nTotal: {} snapshots", total);
    }

    Ok(())
}

fn cmd_status(config: Config) -> incr_backup::error::Result<()> {
    let rclone = Rclone::new(&config);
    let remote_base = &config.remote_path;

    println!("Backup status for {}\n", remote_base);
    println!("Source:     {}", config.source_path);
    println!("Remote:     {}", remote_base);
    println!(
        "Retention:  {}h / {}d / {}w / {}m (min interval: {}min)",
        config.retention.hourly,
        config.retention.daily,
        config.retention.weekly,
        config.retention.monthly,
        config.retention.min_interval_minutes,
    );

    // Check lock
    let lock_mgr = incr_backup::lock::LockManager::new(&rclone, remote_base);
    match lock_mgr.check()? {
        Some(lock) => {
            let stale = if incr_backup::lock::is_stale(&lock) {
                " (STALE)"
            } else {
                ""
            };
            println!("Lock:       {} (pid {}, since {}){}", lock.holder, lock.pid, lock.started_at, stale);
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
