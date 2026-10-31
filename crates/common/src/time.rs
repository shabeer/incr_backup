//! # Generic Time Module
//!
//! Provides reusable timezone configuration, datetime formatting (RFC 3339, display, log, message),
//! and global application timezone state.
//!
//! ## Usage Example
//! ```rust
//! use common::time::{Timezone, set_configured_timezone, get_configured_timezone};
//! use chrono::Utc;
//!
//! // Configure application timezone
//! set_configured_timezone(Timezone::Utc);
//! assert_eq!(get_configured_timezone(), Timezone::Utc);
//!
//! // Format timestamps
//! let now = Utc::now();
//! let rfc = Timezone::Utc.format_rfc3339(now);
//! assert!(rfc.contains('T'));
//! ```

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};
use thiserror::Error;

/// Errors arising from time and timezone operations.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum TimeError {
    /// The specified timezone string is not recognized.
    #[error("invalid timezone '{0}': expected 'utc' or 'local'")]
    InvalidTimezone(String),
}

/// Timezone setting for application timestamp formatting and conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Timezone {
    /// Coordinated Universal Time with 'Z' suffix (e.g., `2026-10-29T04-28Z`).
    #[default]
    Utc,
    /// Local machine timezone with numeric offset (e.g., `2026-10-29T09-58+0530`).
    Local,
}

// Re-export as SnapshotTimezone for compatibility
pub type SnapshotTimezone = Timezone;

impl Serialize for Timezone {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Timezone {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse::<Timezone>().map_err(serde::de::Error::custom)
    }
}

impl Timezone {
    /// Format a UTC DateTime into compact timestamp according to this timezone setting.
    ///
    /// ## Example
    /// ```rust
    /// use common::time::Timezone;
    /// use chrono::{DateTime, Utc};
    ///
    /// let dt = DateTime::parse_from_rfc3339("2026-10-29T04:28:00Z").unwrap().with_timezone(&Utc);
    /// assert_eq!(Timezone::Utc.format_datetime(dt), "2026-10-29T04-28Z");
    /// ```
    pub fn format_datetime(&self, dt: DateTime<Utc>) -> String {
        match self {
            Timezone::Utc => dt.format("%Y-%m-%dT%H-%MZ").to_string(),
            Timezone::Local => {
                dt.with_timezone(&Local).format("%Y-%m-%dT%H-%M%z").to_string()
            }
        }
    }

    /// Format a UTC DateTime for human-readable display according to this timezone setting.
    pub fn format_display_datetime(&self, dt: DateTime<Utc>) -> String {
        match self {
            Timezone::Utc => dt.format("%Y-%m-%dT%H:%MZ").to_string(),
            Timezone::Local => {
                dt.with_timezone(&Local).format("%Y-%m-%dT%H:%M%:z").to_string()
            }
        }
    }

    /// Format the current time according to this timezone setting.
    pub fn now_formatted(&self) -> String {
        self.format_datetime(Utc::now())
    }

    /// Format a log timestamp (seconds precision) according to this timezone setting.
    pub fn format_log_timestamp(&self) -> String {
        match self {
            Timezone::Utc => Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            Timezone::Local => Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
        }
    }

    /// Format a UTC DateTime for log/console messages with full precision according to this timezone setting.
    pub fn format_message_datetime(&self, dt: DateTime<Utc>) -> String {
        match self {
            Timezone::Utc => dt.to_string(),
            Timezone::Local => dt.with_timezone(&Local).to_string(),
        }
    }

    /// Format a UTC DateTime in RFC3339 format according to this timezone setting.
    pub fn format_rfc3339(&self, dt: DateTime<Utc>) -> String {
        match self {
            Timezone::Utc => dt.to_rfc3339(),
            Timezone::Local => dt.with_timezone(&Local).to_rfc3339(),
        }
    }
}

static CURRENT_TIMEZONE: AtomicU8 = AtomicU8::new(0);

const TZ_UTC: u8 = 0;
const TZ_LOCAL: u8 = 1;

/// Get the currently configured timezone for timestamps across the application.
pub fn get_configured_timezone() -> Timezone {
    match CURRENT_TIMEZONE.load(Ordering::Relaxed) {
        TZ_LOCAL => Timezone::Local,
        _ => Timezone::Utc,
    }
}

/// Set the configured timezone for timestamps across the application.
pub fn set_configured_timezone(tz: Timezone) {
    let val = match tz {
        Timezone::Utc => TZ_UTC,
        Timezone::Local => TZ_LOCAL,
    };
    CURRENT_TIMEZONE.store(val, Ordering::Relaxed);
}

impl std::str::FromStr for Timezone {
    type Err = TimeError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "utc" | "z" | "gmt" => Ok(Timezone::Utc),
            "local" => Ok(Timezone::Local),
            _ => Err(TimeError::InvalidTimezone(s.to_string())),
        }
    }
}

impl std::fmt::Display for Timezone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Timezone::Utc => write!(f, "utc"),
            Timezone::Local => write!(f, "local"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_timezone_parsing() {
        assert_eq!(Timezone::from_str("utc").unwrap(), Timezone::Utc);
        assert_eq!(Timezone::from_str("UTC").unwrap(), Timezone::Utc);
        assert_eq!(Timezone::from_str("z").unwrap(), Timezone::Utc);
        assert_eq!(Timezone::from_str("local").unwrap(), Timezone::Local);
        assert_eq!(Timezone::from_str("LOCAL").unwrap(), Timezone::Local);
        assert!(Timezone::from_str("invalid").is_err());
    }

    #[test]
    fn test_timezone_global_state() {
        set_configured_timezone(Timezone::Utc);
        assert_eq!(get_configured_timezone(), Timezone::Utc);
        set_configured_timezone(Timezone::Local);
        assert_eq!(get_configured_timezone(), Timezone::Local);
        set_configured_timezone(Timezone::Utc); // reset
    }

    #[test]
    fn test_datetime_formats() {
        let dt = Utc::now();
        let utc_str = Timezone::Utc.format_datetime(dt);
        assert!(utc_str.ends_with('Z'));
        let rfc_str = Timezone::Utc.format_rfc3339(dt);
        assert!(rfc_str.contains('T'));
    }
}

