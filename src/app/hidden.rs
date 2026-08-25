use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Panes the user has hidden from the main agent list.
///
/// Hidden agents are still monitored and still listed, but in a dim section
/// at the bottom so they stop getting in the way. tmuxcc is a popup that
/// dies on every jump, so the set is persisted on disk; it is keyed by the
/// stable pane id (`%12`) because list indices shift as agents come and go.
#[derive(Debug, Clone, Default)]
pub struct HiddenPanes {
    pane_ids: BTreeSet<String>,
    /// Where the set is saved; `None` keeps it in memory only (tests)
    path: Option<PathBuf>,
}

impl HiddenPanes {
    /// An in-memory set that is never written to disk
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Default location: `$XDG_STATE_HOME/tmuxcc/hidden`, falling back to
    /// `~/.local/state/tmuxcc/hidden` (the `dirs` crate has no state dir on macOS).
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("state")))?;
        Some(base.join("tmuxcc").join("hidden"))
    }

    /// Loads the set from the default path (an unreadable or missing file is
    /// simply an empty set)
    pub fn load() -> Self {
        match Self::default_path() {
            Some(path) => Self::load_from(path),
            None => Self::in_memory(),
        }
    }

    /// Loads the set from `path`; one pane id per line
    pub fn load_from(path: PathBuf) -> Self {
        let pane_ids = std::fs::read_to_string(&path)
            .map(|content| {
                content
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            pane_ids,
            path: Some(path),
        }
    }

    /// Writes the set back to disk. Errors are returned so the UI can show
    /// them, but the in-memory state is already updated either way.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut content =
            String::from("# panes hidden in tmuxcc (space toggles); one pane id per line\n");
        for id in &self.pane_ids {
            content.push_str(id);
            content.push('\n');
        }
        std::fs::write(path, content)
    }

    /// True when the pane is hidden. An empty id is never hidden.
    pub fn contains(&self, pane_id: &str) -> bool {
        !pane_id.is_empty() && self.pane_ids.contains(pane_id)
    }

    /// Hides a shown pane or shows a hidden one; returns the new hidden state.
    /// An empty id (pane id unknown) cannot be hidden.
    pub fn toggle(&mut self, pane_id: &str) -> bool {
        if pane_id.is_empty() {
            return false;
        }
        if self.pane_ids.remove(pane_id) {
            false
        } else {
            self.pane_ids.insert(pane_id.to_string());
            true
        }
    }

    /// Drops ids whose pane no longer exists on the tmux server.
    ///
    /// Meant for startup with the full pane list: pruning against the agent
    /// list would un-hide a pane every time its agent restarts.
    pub fn retain_existing<'a>(&mut self, existing: impl IntoIterator<Item = &'a str>) -> bool {
        let existing: BTreeSet<&str> = existing.into_iter().collect();
        let before = self.pane_ids.len();
        self.pane_ids.retain(|id| existing.contains(id.as_str()));
        self.pane_ids.len() != before
    }

    pub fn len(&self) -> usize {
        self.pane_ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pane_ids.is_empty()
    }

    /// Path the set is saved to, if any
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_and_contains() {
        let mut hidden = HiddenPanes::in_memory();
        assert!(!hidden.contains("%1"));
        assert!(hidden.toggle("%1"));
        assert!(hidden.contains("%1"));
        assert!(!hidden.toggle("%1"));
        assert!(!hidden.contains("%1"));
    }

    #[test]
    fn empty_id_is_never_hidden() {
        let mut hidden = HiddenPanes::in_memory();
        assert!(!hidden.toggle(""));
        assert!(!hidden.contains(""));
        assert!(hidden.is_empty());
    }

    #[test]
    fn round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("hidden");

        let mut hidden = HiddenPanes::load_from(path.clone());
        assert!(hidden.is_empty());
        hidden.toggle("%12");
        hidden.toggle("%3");
        hidden.save().unwrap();

        let reloaded = HiddenPanes::load_from(path);
        assert_eq!(reloaded.len(), 2);
        assert!(reloaded.contains("%12"));
        assert!(reloaded.contains("%3"));
    }

    #[test]
    fn prunes_panes_that_are_gone() {
        let mut hidden = HiddenPanes::in_memory();
        hidden.toggle("%1");
        hidden.toggle("%2");
        assert!(hidden.retain_existing(["%2", "%9"]));
        assert!(!hidden.contains("%1"));
        assert!(hidden.contains("%2"));
        assert!(!hidden.retain_existing(["%2"]));
    }
}
