//! Kiro CLI session titles, read from Kiro's own session store.
//!
//! Kiro CLI (v3 TUI) only ever writes `kiro: <cwd>` into the terminal title,
//! so the pane title never carries the session name. The name lives on disk:
//!
//! ```text
//! ~/.kiro/sessions/<workspace-hash>/sess_<uuid>/
//!     .lock          {"pid": <tui pid>, "started_at": "..."}   (while open)
//!     session.json   {"id": "sess_<uuid>", "title": "...", ...}
//! ```
//!
//! The `.lock` pid is a descendant of the pane shell, which ties a session to
//! its pane. Because the title is read from the session itself, it survives a
//! tmux restore: resuming the session (`kiro-cli chat --resume-id ...`) takes
//! the lock again under the new pid and the title comes straight back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Deserialize;

/// How long a scan of the session store is reused. Titles change at most once
/// per turn, so this only needs to be fresher than a human glance.
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// Placeholder Kiro uses before it has generated a real title.
const UNTITLED: &str = "New Session";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiroSession {
    pub id: String,
    pub title: String,
}

#[derive(Deserialize)]
struct LockFile {
    pid: u32,
}

#[derive(Deserialize)]
struct SessionFile {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
}

struct Index {
    root: Option<PathBuf>,
    by_pid: HashMap<u32, KiroSession>,
    last_scan: Option<Instant>,
}

impl Index {
    fn refresh_if_stale(&mut self) {
        if self
            .last_scan
            .is_some_and(|t| t.elapsed() < REFRESH_INTERVAL)
        {
            return;
        }
        self.by_pid = self.root.as_deref().map(scan).unwrap_or_default();
        self.last_scan = Some(Instant::now());
    }
}

static INDEX: OnceLock<Mutex<Index>> = OnceLock::new();

fn index() -> &'static Mutex<Index> {
    INDEX.get_or_init(|| {
        Mutex::new(Index {
            root: dirs::home_dir().map(|h| h.join(".kiro").join("sessions")),
            by_pid: HashMap::new(),
            last_scan: None,
        })
    })
}

/// Maps the pid holding each session's `.lock` to that session.
///
/// Only sessions that are currently open hold a lock, so this touches a
/// handful of files even when the store has hundreds of sessions.
fn scan(root: &Path) -> HashMap<u32, KiroSession> {
    let mut by_pid = HashMap::new();
    let Ok(workspaces) = std::fs::read_dir(root) else {
        return by_pid;
    };

    for workspace in workspaces.flatten() {
        let Ok(sessions) = std::fs::read_dir(workspace.path()) else {
            continue;
        };
        for session in sessions.flatten() {
            let dir = session.path();
            let Some(lock) = read_json::<LockFile>(&dir.join(".lock")) else {
                continue;
            };
            let Some(file) = read_json::<SessionFile>(&dir.join("session.json")) else {
                continue;
            };
            let id = if file.id.is_empty() {
                dir.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            } else {
                file.id
            };
            by_pid.insert(
                lock.pid,
                KiroSession {
                    id,
                    title: file.title,
                },
            );
        }
    }

    by_pid
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Picks the session owned by one of `pids` (the pane's process tree).
fn find_session<'a>(
    by_pid: &'a HashMap<u32, KiroSession>,
    pids: &[u32],
) -> Option<&'a KiroSession> {
    pids.iter().find_map(|pid| by_pid.get(pid))
}

/// A displayable title, or None while Kiro has not named the session yet.
fn usable_title(title: &str) -> Option<String> {
    let title = title.trim();
    if title.is_empty() || title == UNTITLED {
        None
    } else {
        Some(title.to_string())
    }
}

/// Title of the Kiro session running under the pane whose process tree is
/// `pids`, if it has one yet.
pub fn title_for(pids: &[u32]) -> Option<String> {
    let mut index = index().lock();
    index.refresh_if_stale();
    find_session(&index.by_pid, pids).and_then(|s| usable_title(&s.title))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_session(root: &Path, ws: &str, id: &str, pid: Option<u32>, title: &str) {
        let dir = root.join(ws).join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("session.json"),
            serde_json::json!({ "id": id, "title": title, "modelId": "x" }).to_string(),
        )
        .unwrap();
        if let Some(pid) = pid {
            fs::write(
                dir.join(".lock"),
                format!(r#"{{"pid":{pid},"started_at":"2026-10-22T12:41:52.503Z"}}"#),
            )
            .unwrap();
        }
    }

    #[test]
    fn maps_lock_pid_to_session_title() {
        let tmp = tempfile::tempdir().unwrap();
        write_session(
            tmp.path(),
            "28af",
            "sess_a",
            Some(67004),
            "Tmux 세션 재로드 시 제목 복원",
        );
        write_session(tmp.path(), "28af", "sess_b", None, "closed session");
        write_session(
            tmp.path(),
            "878a",
            "sess_c",
            Some(98084),
            "TeslaMate \"모니터링\"",
        );

        let by_pid = scan(tmp.path());
        assert_eq!(by_pid.len(), 2, "sessions without a lock are not open");

        // The pane tree holds the shell, kiro-cli-chat and the bun TUI; only
        // the TUI pid is in the lock.
        let s = find_session(&by_pid, &[66893, 67000, 67004]).unwrap();
        assert_eq!(s.id, "sess_a");
        assert_eq!(s.title, "Tmux 세션 재로드 시 제목 복원");
        assert_eq!(
            find_session(&by_pid, &[98084]).unwrap().title,
            "TeslaMate \"모니터링\""
        );
        assert!(find_session(&by_pid, &[1, 2, 3]).is_none());
    }

    #[test]
    fn skips_unreadable_or_garbage_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ws").join("sess_x");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".lock"), "not json").unwrap();
        fs::write(dir.join("session.json"), r#"{"title":"t"}"#).unwrap();
        // Stray file at the workspace level must not break the scan.
        fs::write(tmp.path().join("stray.txt"), "x").unwrap();

        assert!(scan(tmp.path()).is_empty());
        assert!(scan(&tmp.path().join("missing")).is_empty());
    }

    #[test]
    fn untitled_sessions_have_no_title() {
        assert_eq!(usable_title("New Session"), None);
        assert_eq!(usable_title("  "), None);
        assert_eq!(
            usable_title(" 보드멜 배포 "),
            Some("보드멜 배포".to_string())
        );
    }
}
