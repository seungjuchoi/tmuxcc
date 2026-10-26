//! Claude subscription rate limits (the weekly / 5-hour usage meters).
//!
//! Claude Code hands its statusline command a `rate_limits` object on every
//! render, but exposes it nowhere else. The statusline script is expected to
//! dump that object to a snapshot file, which this module reads:
//!
//! ```text
//! $XDG_STATE_HOME/tmuxcc/claude-usage.json   (default ~/.local/state/...)
//! {
//!   "five_hour": {"used_percentage": 12, "resets_at": 1761465600},
//!   "seven_day": {"used_percentage": 42, "resets_at": 1761811200},
//!   "captured_at": 1761450000
//! }
//! ```
//!
//! Reading a file keeps tmuxcc off the network and away from the OAuth token.
//! The trade-off is that the numbers only move while some Claude Code session
//! is rendering its statusline.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// How often the snapshot file is re-read
const REFRESH_INTERVAL: Duration = Duration::from_secs(5);

/// A window whose reset passed this long ago without a fresh snapshot is
/// treated as abandoned (Claude Code unused or uninstalled) and hidden.
const ABANDONED_AFTER_SECS: i64 = 7 * 24 * 3600;

/// One rate-limit window as Claude Code reports it
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageWindow {
    /// Share of the window already consumed, 0-100
    pub used_percentage: f64,
    /// Unix epoch seconds when the window resets
    pub resets_at: i64,
}

impl UsageWindow {
    /// Reads `{"used_percentage": n, "resets_at": n}`; numbers may be
    /// integers or floats. Anything else yields None.
    fn from_json(value: &Value) -> Option<Self> {
        let used = value.get("used_percentage")?.as_f64()?;
        let resets_at = value.get("resets_at")?.as_f64()?;
        (used.is_finite() && resets_at.is_finite()).then_some(Self {
            used_percentage: used.max(0.0),
            resets_at: resets_at as i64,
        })
    }

    /// True once the snapshot is too old to be worth showing
    pub fn is_abandoned(&self, now: i64) -> bool {
        now >= self.resets_at.saturating_add(ABANDONED_AFTER_SECS)
    }

    /// Usage as of `now` (epoch seconds). Once the window has reset the
    /// snapshot is stale and the fresh window starts empty.
    pub fn used_at(&self, now: i64) -> f64 {
        if now >= self.resets_at {
            0.0
        } else {
            self.used_percentage
        }
    }

    /// Seconds until the window resets, or None if it already has
    pub fn remaining_secs(&self, now: i64) -> Option<i64> {
        (self.resets_at > now).then(|| self.resets_at - now)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClaudeUsage {
    pub five_hour: Option<UsageWindow>,
    pub seven_day: Option<UsageWindow>,
}

impl ClaudeUsage {
    /// Parses a snapshot. Each window is read on its own, so a malformed or
    /// missing one does not hide the other. None when the file is not JSON
    /// or has no usable window at all.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let value: Value = serde_json::from_slice(bytes).ok()?;
        let window = |key: &str| value.get(key).and_then(UsageWindow::from_json);
        let usage = Self {
            five_hour: window("five_hour"),
            seven_day: window("seven_day"),
        };
        (usage.five_hour.is_some() || usage.seven_day.is_some()).then_some(usage)
    }
}

/// Current Unix time in seconds
pub fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Short countdown such as `2d5h`, `3h12m` or `42m`
pub fn format_remaining(secs: i64) -> String {
    let mins = secs.max(0) / 60;
    let (days, hours, mins) = (mins / 1440, mins / 60 % 24, mins % 60);
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{mins}m")
    } else {
        format!("{mins}m")
    }
}

/// `$XDG_STATE_HOME/tmuxcc/claude-usage.json`, falling back to
/// `~/.local/state/tmuxcc/claude-usage.json`
pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("state")))?;
    Some(base.join("tmuxcc").join("claude-usage.json"))
}

/// Re-reads the snapshot file at most every [`REFRESH_INTERVAL`]
pub struct ClaudeUsageReader {
    path: Option<PathBuf>,
    usage: Option<ClaudeUsage>,
    last_read: Option<Instant>,
}

impl ClaudeUsageReader {
    pub fn new() -> Self {
        Self {
            path: default_path(),
            usage: None,
            last_read: None,
        }
    }

    /// Latest snapshot; None when no file exists or it cannot be parsed
    pub fn usage(&mut self) -> Option<&ClaudeUsage> {
        if self
            .last_read
            .is_none_or(|t| t.elapsed() >= REFRESH_INTERVAL)
        {
            self.usage = self
                .path
                .as_ref()
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|bytes| ClaudeUsage::parse(&bytes));
            self.last_read = Some(Instant::now());
        }
        self.usage.as_ref()
    }
}

impl Default for ClaudeUsageReader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_statusline_snapshot() {
        let usage = ClaudeUsage::parse(
            br#"{"five_hour":{"used_percentage":12.4,"resets_at":1000},
                 "seven_day":{"used_percentage":74,"resets_at":2000.0},
                 "spend_limit":{"used_usd":1},"captured_at":900}"#,
        )
        .unwrap();
        assert_eq!(
            usage.seven_day,
            Some(UsageWindow {
                used_percentage: 74.0,
                resets_at: 2000
            })
        );
        assert_eq!(usage.five_hour.unwrap().used_percentage, 12.4);
    }

    #[test]
    fn a_broken_window_does_not_hide_the_other() {
        let usage = ClaudeUsage::parse(
            br#"{"five_hour":{"used_percentage":null},
                 "seven_day":{"used_percentage":50,"resets_at":2000}}"#,
        )
        .unwrap();
        assert!(usage.five_hour.is_none());
        assert!(usage.seven_day.is_some());
    }

    #[test]
    fn unusable_snapshots_yield_nothing() {
        for bad in [
            &b""[..],
            b"not json",
            b"{}",
            b"[]",
            b"null",
            br#"{"seven_day":"x"}"#,
            br#"{"seven_day":{"used_percentage":"50","resets_at":1}}"#,
        ] {
            assert_eq!(ClaudeUsage::parse(bad), None, "{:?}", bad);
        }
    }

    #[test]
    fn missing_file_yields_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reader = ClaudeUsageReader {
            path: Some(tmp.path().join("claude-usage.json")),
            usage: None,
            last_read: None,
        };
        assert!(reader.usage().is_none());

        let mut no_home = ClaudeUsageReader {
            path: None,
            usage: None,
            last_read: None,
        };
        assert!(no_home.usage().is_none());
    }

    #[test]
    fn reset_and_abandoned_windows() {
        let w = UsageWindow {
            used_percentage: 80.0,
            resets_at: 1000,
        };
        assert_eq!(w.used_at(999), 80.0);
        assert_eq!(w.remaining_secs(999), Some(1));
        // Reset passed: the new window starts empty
        assert_eq!(w.used_at(1000), 0.0);
        assert_eq!(w.remaining_secs(1000), None);
        assert!(!w.is_abandoned(1000 + ABANDONED_AFTER_SECS - 1));
        assert!(w.is_abandoned(1000 + ABANDONED_AFTER_SECS));
    }

    #[test]
    fn formats_countdown() {
        assert_eq!(format_remaining(-5), "0m");
        assert_eq!(format_remaining(59), "0m");
        assert_eq!(format_remaining(42 * 60), "42m");
        assert_eq!(format_remaining(3 * 3600 + 12 * 60), "3h12m");
        assert_eq!(format_remaining(2 * 86400 + 5 * 3600 + 59), "2d5h");
    }
}
