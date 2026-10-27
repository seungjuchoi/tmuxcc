use std::path::PathBuf;

use crate::agents::{AgentStatus, MonitoredAgent};

/// How the main agent list is ordered (`o` cycles, persisted)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SortMode {
    /// tmux order, grouped by session: (session, window, pane)
    #[default]
    Session,
    /// Agents that need you first, then by status, then tmux order
    Attention,
}

impl SortMode {
    /// The mode `o` switches to
    pub fn next(self) -> Self {
        match self {
            SortMode::Session => SortMode::Attention,
            SortMode::Attention => SortMode::Session,
        }
    }

    /// Name stored on disk and shown in the UI
    pub fn as_str(self) -> &'static str {
        match self {
            SortMode::Session => "session",
            SortMode::Attention => "attention",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "session" => Some(SortMode::Session),
            "attention" => Some(SortMode::Attention),
            _ => None,
        }
    }

    /// `<state dir>/sort`
    pub fn default_path() -> Option<PathBuf> {
        crate::app::pane_set::state_dir().map(|dir| dir.join("sort"))
    }

    /// Reads the mode from `path`; a missing or unknown value is `Session`
    pub fn load_from(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|content| {
                content
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty() && !l.starts_with('#'))
                    .and_then(Self::parse)
            })
            .unwrap_or_default()
    }

    /// Writes the mode to `path`
    pub fn save_to(self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            path,
            format!(
                "# tmuxcc list order (o toggles): session | attention\n{}\n",
                self.as_str()
            ),
        )
    }
}

/// Rank of an agent's status in the attention order (lower comes first):
/// waiting on you (approval or question) → error → done → working → idle → unknown.
///
/// Done ranks above working on purpose (as in Orca): a finished turn is the
/// next thing to look at, a running one does not need you yet.
pub fn attention_rank(agent: &MonitoredAgent) -> u8 {
    match agent.status {
        AgentStatus::AwaitingApproval { .. } => 0,
        AgentStatus::Error { .. } => 1,
        AgentStatus::Done { .. } => 2,
        AgentStatus::Processing { .. } => 3,
        AgentStatus::Idle => 4,
        AgentStatus::Unknown => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_between_modes() {
        assert_eq!(SortMode::Session.next(), SortMode::Attention);
        assert_eq!(SortMode::Attention.next(), SortMode::Session);
    }

    #[test]
    fn round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("sort");
        assert_eq!(SortMode::load_from(&path), SortMode::Session);

        SortMode::Attention.save_to(&path).unwrap();
        assert_eq!(SortMode::load_from(&path), SortMode::Attention);

        std::fs::write(&path, "bogus\n").unwrap();
        assert_eq!(SortMode::load_from(&path), SortMode::Session);
    }
}
