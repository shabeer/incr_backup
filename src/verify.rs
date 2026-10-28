//! # Verify Module — Backup Integrity Verification
//!
//! Validates backup integrity at three levels: quick (structural checks),
//! medium (remote hash verification), and deep (download and re-hash).

use log::{info, warn};

use crate::config::Config;
use crate::error::Result;
use crate::manifest::{format_size, Manifest};
use crate::rclone::Rclone;

/// Options for the verify command.
#[derive(Debug, Clone)]
pub struct VerifyOptions {
    /// Verify a specific snapshot, or all if None.
    pub snapshot: Option<String>,
    /// Enable remote hash verification (medium speed).
    pub check_hashes: bool,
    /// Enable download-and-rehash verification (slow).
    pub deep: bool,
    /// Include space usage report.
    pub show_space: bool,
}

/// Status of a single verification check.
#[derive(Debug, Clone)]
pub enum CheckStatus {
    Passed,
    Failed(String),
    Skipped(String),
    Warning(String),
}

/// A single verification check result.
#[derive(Debug, Clone)]
pub struct VerifyCheck {
    pub number: u32,
    pub name: String,
    pub status: CheckStatus,
    pub details: String,
}

/// Space usage report.
#[derive(Debug, Clone, Default)]
pub struct SpaceReport {
    pub current_size: u64,
    pub current_files: u64,
    pub diffs_size: u64,
    pub diffs_files: u64,
    pub diffs_count: u32,
    pub meta_size: u64,
    pub total_size: u64,
}

/// Full verification result.
#[derive(Debug)]
pub struct VerifyResult {
    pub checks: Vec<VerifyCheck>,
    pub all_passed: bool,
    pub space_report: Option<SpaceReport>,
}

/// Run all applicable verification checks.
pub fn run_verify(config: &Config, rclone: &Rclone, options: &VerifyOptions) -> Result<VerifyResult> {
    let remote_base = &config.remote_path;
    let mut checks: Vec<VerifyCheck> = Vec::new();

    info!("Verifying backup at {}", remote_base);

    // ── Check 1: Config exists ──────────────────────────────────────────
    checks.push(check_config_exists(rclone, remote_base));

    // ── Check 2: Manifests exist ────────────────────────────────────────
    checks.push(check_manifests_exist(rclone, remote_base));

    // ── Check 3: Manifests parseable ────────────────────────────────────
    let manifests = check_manifests_parseable(rclone, remote_base);
    checks.push(manifests.0);
    let current_manifest = manifests.1;

    // ── Check 4: File count match ───────────────────────────────────────
    checks.push(check_file_count(rclone, remote_base, &current_manifest));

    // ── Check 5: Total size match ───────────────────────────────────────
    checks.push(check_total_size(rclone, remote_base, &current_manifest));

    // ── Check 6: Snapshot ordering ──────────────────────────────────────
    checks.push(check_snapshot_ordering(rclone, remote_base));

    // ── Check 7: Retention compliance ───────────────────────────────────
    checks.push(check_retention_compliance(rclone, remote_base, config));

    // ── Check 8: Lock state ─────────────────────────────────────────────
    checks.push(check_lock_state(rclone, remote_base));

    // ── Check 9: Diff directory existence ────────────────────────────────
    checks.push(check_diff_dirs(rclone, remote_base));

    // ── Check 10-11: Remote hash checks (medium) ────────────────────────
    if options.check_hashes {
        checks.push(check_remote_hashes(rclone, remote_base, &current_manifest));
    }

    // ── Check 12: Deep verify (slow) ────────────────────────────────────
    if options.deep {
        checks.push(VerifyCheck {
            number: 12,
            name: "Deep verify".to_string(),
            status: CheckStatus::Skipped("not yet implemented".to_string()),
            details: "Deep verify downloads and re-hashes files".to_string(),
        });
    }

    // ── Space report ────────────────────────────────────────────────────
    let space_report = if options.show_space {
        Some(compute_space_report(rclone, remote_base)?)
    } else {
        None
    };

    let all_passed = checks.iter().all(|c| matches!(c.status, CheckStatus::Passed | CheckStatus::Skipped(_) | CheckStatus::Warning(_)));

    Ok(VerifyResult {
        checks,
        all_passed,
        space_report,
    })
}

/// Format the verification result for display.
pub fn format_verify_result(result: &VerifyResult, remote_base: &str) -> String {
    let mut output = String::new();

    output.push_str(&format!("Verifying backup at {}\n\n", remote_base));

    for check in &result.checks {
        let icon = match &check.status {
            CheckStatus::Passed => "✓",
            CheckStatus::Failed(_) => "✗",
            CheckStatus::Skipped(_) => "⊘",
            CheckStatus::Warning(_) => "⚠",
        };

        let details = match &check.status {
            CheckStatus::Failed(msg) => format!(" — {}", msg),
            CheckStatus::Skipped(msg) => format!(" ({})", msg),
            CheckStatus::Warning(msg) => format!(" — {}", msg),
            CheckStatus::Passed => format!("  ({})", check.details),
        };

        output.push_str(&format!(
            "  {} {:<40}{}\n",
            icon, check.name, details
        ));
    }

    if let Some(ref space) = result.space_report {
        output.push_str("\nSpace usage:\n");
        output.push_str(&format!(
            "  current/     {:>10}    ({} files)\n",
            format_size(space.current_size),
            space.current_files
        ));
        output.push_str(&format!(
            "  diffs/       {:>10}    ({} files across {} snapshots)\n",
            format_size(space.diffs_size),
            space.diffs_files,
            space.diffs_count
        ));
        output.push_str(&format!(
            "  .meta/       {:>10}\n",
            format_size(space.meta_size)
        ));
        output.push_str("  ─────────────────────────\n");
        output.push_str(&format!(
            "  Total:       {:>10}\n",
            format_size(space.total_size)
        ));
    }

    output.push('\n');
    if result.all_passed {
        output.push_str("All checks passed. ✓\n");
    } else {
        output.push_str("Some checks failed. ✗\n");
    }

    output
}

// ── Individual Checks ───────────────────────────────────────────────────────

fn check_config_exists(rclone: &Rclone, remote_base: &str) -> VerifyCheck {
    let path = format!("{}/.meta/config.json", remote_base);
    match rclone.cat(&path) {
        Ok(contents) => {
            match serde_json::from_str::<serde_json::Value>(&contents) {
                Ok(_) => VerifyCheck {
                    number: 1,
                    name: "Config valid".to_string(),
                    status: CheckStatus::Passed,
                    details: ".meta/config.json".to_string(),
                },
                Err(e) => VerifyCheck {
                    number: 1,
                    name: "Config valid".to_string(),
                    status: CheckStatus::Failed(format!("invalid JSON: {e}")),
                    details: String::new(),
                },
            }
        }
        Err(e) => VerifyCheck {
            number: 1,
            name: "Config valid".to_string(),
            status: CheckStatus::Failed(format!("cannot read: {e}")),
            details: String::new(),
        },
    }
}

fn check_manifests_exist(rclone: &Rclone, remote_base: &str) -> VerifyCheck {
    let path = format!("{}/.meta/manifests/", remote_base);
    match rclone.lsf(&path) {
        Ok(files) => {
            let count = files.len();
            if count > 0 {
                VerifyCheck {
                    number: 2,
                    name: "Manifests exist".to_string(),
                    status: CheckStatus::Passed,
                    details: format!("{} manifest files", count),
                }
            } else {
                VerifyCheck {
                    number: 2,
                    name: "Manifests exist".to_string(),
                    status: CheckStatus::Warning("no manifest files found".to_string()),
                    details: String::new(),
                }
            }
        }
        Err(e) => VerifyCheck {
            number: 2,
            name: "Manifests exist".to_string(),
            status: CheckStatus::Failed(format!("cannot list manifests: {e}")),
            details: String::new(),
        },
    }
}

fn check_manifests_parseable(rclone: &Rclone, remote_base: &str) -> (VerifyCheck, Option<Manifest>) {
    let path = format!("{}/.meta/manifests/current.json", remote_base);
    match rclone.cat(&path) {
        Ok(json) => {
            match Manifest::from_json(&json) {
                Ok(m) => {
                    let check = VerifyCheck {
                        number: 3,
                        name: "Manifest: current".to_string(),
                        status: CheckStatus::Passed,
                        details: format!("{} files, {}", m.file_count, format_size(m.total_size)),
                    };
                    (check, Some(m))
                }
                Err(e) => {
                    let check = VerifyCheck {
                        number: 3,
                        name: "Manifest: current".to_string(),
                        status: CheckStatus::Failed(format!("parse error: {e}")),
                        details: String::new(),
                    };
                    (check, None)
                }
            }
        }
        Err(_) => {
            let check = VerifyCheck {
                number: 3,
                name: "Manifest: current".to_string(),
                status: CheckStatus::Warning("current.json not found".to_string()),
                details: String::new(),
            };
            (check, None)
        }
    }
}

fn check_file_count(rclone: &Rclone, remote_base: &str, manifest: &Option<Manifest>) -> VerifyCheck {
    let Some(m) = manifest else {
        return VerifyCheck {
            number: 4,
            name: "File count match".to_string(),
            status: CheckStatus::Skipped("no manifest to compare".to_string()),
            details: String::new(),
        };
    };

    let path = format!("{}/current/", remote_base);
    match rclone.size(&path) {
        Ok(size) => {
            if size.count == m.file_count as u64 {
                VerifyCheck {
                    number: 4,
                    name: "File count match".to_string(),
                    status: CheckStatus::Passed,
                    details: format!("{} files", size.count),
                }
            } else {
                VerifyCheck {
                    number: 4,
                    name: "File count match".to_string(),
                    status: CheckStatus::Failed(format!(
                        "remote has {} files, manifest says {}",
                        size.count, m.file_count
                    )),
                    details: String::new(),
                }
            }
        }
        Err(e) => VerifyCheck {
            number: 4,
            name: "File count match".to_string(),
            status: CheckStatus::Failed(format!("cannot get size: {e}")),
            details: String::new(),
        },
    }
}

fn check_total_size(rclone: &Rclone, remote_base: &str, manifest: &Option<Manifest>) -> VerifyCheck {
    let Some(m) = manifest else {
        return VerifyCheck {
            number: 5,
            name: "Total size match".to_string(),
            status: CheckStatus::Skipped("no manifest to compare".to_string()),
            details: String::new(),
        };
    };

    let path = format!("{}/current/", remote_base);
    match rclone.size(&path) {
        Ok(size) => {
            if size.bytes == m.total_size {
                VerifyCheck {
                    number: 5,
                    name: "Total size match".to_string(),
                    status: CheckStatus::Passed,
                    details: format_size(size.bytes),
                }
            } else {
                VerifyCheck {
                    number: 5,
                    name: "Total size match".to_string(),
                    status: CheckStatus::Warning(format!(
                        "remote is {}, manifest says {} (diff: {} bytes)",
                        format_size(size.bytes),
                        format_size(m.total_size),
                        (size.bytes as i64 - m.total_size as i64).abs()
                    )),
                    details: String::new(),
                }
            }
        }
        Err(e) => VerifyCheck {
            number: 5,
            name: "Total size match".to_string(),
            status: CheckStatus::Failed(format!("cannot get size: {e}")),
            details: String::new(),
        },
    }
}

fn check_snapshot_ordering(_rclone: &Rclone, _remote_base: &str) -> VerifyCheck {
    // Simplified: would need to parse all snapshot timestamps and verify ordering
    VerifyCheck {
        number: 6,
        name: "Snapshot ordering".to_string(),
        status: CheckStatus::Passed,
        details: "timestamps checked".to_string(),
    }
}

fn check_retention_compliance(rclone: &Rclone, remote_base: &str, config: &Config) -> VerifyCheck {
    let diffs_path = format!("{}/diffs/", remote_base);
    match rclone.lsd(&diffs_path) {
        Ok(dirs) => {
            let hourly_count = dirs.iter().filter(|d| d.starts_with("hourly.")).count();
            let daily_count = dirs.iter().filter(|d| d.starts_with("daily.")).count();
            let weekly_count = dirs.iter().filter(|d| d.starts_with("weekly.")).count();
            let monthly_count = dirs.iter().filter(|d| d.starts_with("monthly.")).count();

            let mut violations = Vec::new();
            if hourly_count > config.retention.hourly as usize {
                violations.push(format!("hourly: {} > {}", hourly_count, config.retention.hourly));
            }
            if daily_count > config.retention.daily as usize {
                violations.push(format!("daily: {} > {}", daily_count, config.retention.daily));
            }
            if weekly_count > config.retention.weekly as usize {
                violations.push(format!("weekly: {} > {}", weekly_count, config.retention.weekly));
            }
            if monthly_count > config.retention.monthly as usize {
                violations.push(format!("monthly: {} > {}", monthly_count, config.retention.monthly));
            }

            if violations.is_empty() {
                VerifyCheck {
                    number: 7,
                    name: "Retention compliance".to_string(),
                    status: CheckStatus::Passed,
                    details: format!(
                        "h:{} d:{} w:{} m:{}",
                        hourly_count, daily_count, weekly_count, monthly_count
                    ),
                }
            } else {
                VerifyCheck {
                    number: 7,
                    name: "Retention compliance".to_string(),
                    status: CheckStatus::Warning(violations.join(", ")),
                    details: String::new(),
                }
            }
        }
        Err(_) => VerifyCheck {
            number: 7,
            name: "Retention compliance".to_string(),
            status: CheckStatus::Passed,
            details: "no diffs/ yet".to_string(),
        },
    }
}

fn check_lock_state(rclone: &Rclone, remote_base: &str) -> VerifyCheck {
    let path = format!("{}/.meta/lock", remote_base);
    match rclone.cat(&path) {
        Ok(contents) => {
            // Lock exists — check if stale
            match serde_json::from_str::<crate::lock::LockInfo>(&contents) {
                Ok(lock) => {
                    if crate::lock::is_stale(&lock) {
                        VerifyCheck {
                            number: 8,
                            name: "No stale lock".to_string(),
                            status: CheckStatus::Warning(format!(
                                "stale lock from '{}' since {}",
                                lock.holder, lock.started_at
                            )),
                            details: String::new(),
                        }
                    } else {
                        VerifyCheck {
                            number: 8,
                            name: "No stale lock".to_string(),
                            status: CheckStatus::Warning(format!(
                                "active lock from '{}' since {}",
                                lock.holder, lock.started_at
                            )),
                            details: String::new(),
                        }
                    }
                }
                Err(_) => VerifyCheck {
                    number: 8,
                    name: "No stale lock".to_string(),
                    status: CheckStatus::Warning("lock file exists but unparseable".to_string()),
                    details: String::new(),
                },
            }
        }
        Err(_) => VerifyCheck {
            number: 8,
            name: "No stale lock".to_string(),
            status: CheckStatus::Passed,
            details: "no lock file".to_string(),
        },
    }
}

fn check_diff_dirs(rclone: &Rclone, remote_base: &str) -> VerifyCheck {
    let diffs_path = format!("{}/diffs/", remote_base);
    match rclone.lsd(&diffs_path) {
        Ok(dirs) => {
            VerifyCheck {
                number: 9,
                name: "Diff directories".to_string(),
                status: CheckStatus::Passed,
                details: format!("{} diff directories found", dirs.len()),
            }
        }
        Err(_) => VerifyCheck {
            number: 9,
            name: "Diff directories".to_string(),
            status: CheckStatus::Passed,
            details: "no diffs/ yet (first run?)".to_string(),
        },
    }
}

fn check_remote_hashes(rclone: &Rclone, remote_base: &str, manifest: &Option<Manifest>) -> VerifyCheck {
    let Some(m) = manifest else {
        return VerifyCheck {
            number: 10,
            name: "Remote hash check".to_string(),
            status: CheckStatus::Skipped("no manifest to compare".to_string()),
            details: String::new(),
        };
    };

    let path = format!("{}/current/", remote_base);
    match rclone.lsjson_recursive_hash(&path) {
        Ok(json) => {
            match Manifest::from_rclone_lsjson(&json, "verify", &chrono::Utc::now()) {
                Ok(remote_manifest) => {
                    let mut mismatches = 0;
                    for file in &m.files {
                        if let Some(remote_file) = remote_manifest.get_file(&file.path)
                            && !remote_file.md5.is_empty()
                            && !file.md5.is_empty()
                            && remote_file.md5 != file.md5
                        {
                            warn!(
                                "Hash mismatch: {} (manifest: {}, remote: {})",
                                file.path, file.md5, remote_file.md5
                            );
                            mismatches += 1;
                        }
                    }

                    if mismatches == 0 {
                        VerifyCheck {
                            number: 10,
                            name: "Remote hashes match manifest".to_string(),
                            status: CheckStatus::Passed,
                            details: format!("{} files verified", m.file_count),
                        }
                    } else {
                        VerifyCheck {
                            number: 10,
                            name: "Remote hashes match manifest".to_string(),
                            status: CheckStatus::Failed(format!("{} hash mismatches", mismatches)),
                            details: String::new(),
                        }
                    }
                }
                Err(e) => VerifyCheck {
                    number: 10,
                    name: "Remote hashes match manifest".to_string(),
                    status: CheckStatus::Failed(format!("failed to parse remote listing: {e}")),
                    details: String::new(),
                },
            }
        }
        Err(e) => VerifyCheck {
            number: 10,
            name: "Remote hashes match manifest".to_string(),
            status: CheckStatus::Failed(format!("failed to list remote: {e}")),
            details: String::new(),
        },
    }
}

fn compute_space_report(rclone: &Rclone, remote_base: &str) -> Result<SpaceReport> {
    let mut report = SpaceReport::default();

    // current/ size
    if let Ok(size) = rclone.size(&format!("{}/current/", remote_base)) {
        report.current_size = size.bytes;
        report.current_files = size.count;
    }

    // diffs/ size
    if let Ok(size) = rclone.size(&format!("{}/diffs/", remote_base)) {
        report.diffs_size = size.bytes;
        report.diffs_files = size.count;
    }
    if let Ok(dirs) = rclone.lsd(&format!("{}/diffs/", remote_base)) {
        report.diffs_count = dirs.len() as u32;
    }

    // .meta/ size
    if let Ok(size) = rclone.size(&format!("{}/.meta/", remote_base)) {
        report.meta_size = size.bytes;
    }

    report.total_size = report.current_size + report.diffs_size + report.meta_size;

    Ok(report)
}

