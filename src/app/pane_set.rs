use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A persisted set of tmux panes, keyed by the stable pane id (`%12`).
///
/// Used for the panes the user hid (dim section at the bottom of the list)
/// and the panes the user pinned (section at the top). tmuxcc is a popup that
/// dies on every jump, so the sets live on disk; pane ids are used because
/// list indices shift as agents come and go.
#[derive(Debug, Clone, Default)]
pub struct PaneSet {
    pane_ids: BTreeSet<String>,
    /// Where the set is saved; `None` keeps it in memory only (tests)
    path: Option<PathBuf>,
    /// Comment written on the first line of the file
    header: &'static str,
}

/// Panes hidden in the dim list at the bottom (h)
pub type HiddenPanes = PaneSet;
/// Panes pinned to the top of the list (p)
pub type PinnedPanes = PaneSet;

/// File name and header of the hidden set
const HIDDEN: (&str, &str) = (
    "hidden",
    "# panes hidden in tmuxcc (h toggles); one pane id per line",
);
/// File name and header of the pinned set
const PINNED: (&str, &str) = (
    "pinned",
    "# panes pinned in tmuxcc (p toggles); one pane id per line",
);

/// tmuxcc's state directory: `$XDG_STATE_HOME/tmuxcc`, falling back to
/// `~/.local/state/tmuxcc` (the `dirs` crate has no state dir on macOS).
pub fn state_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("state")))?;
    Some(base.join("tmuxcc"))
}

impl PaneSet {
    /// An in-memory set that is never written to disk
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Loads the hidden set from `<state dir>/hidden`
    pub fn load_hidden() -> Self {
        Self::load_named(HIDDEN)
    }

    /// Loads the pinned set from `<state dir>/pinned`
    pub fn load_pinned() -> Self {
        Self::load_named(PINNED)
    }

    /// Loads a set from the state dir (an unreadable or missing file is simply
    /// an empty set)
    fn load_named((name, header): (&str, &'static str)) -> Self {
        match state_dir() {
            Some(dir) => Self::load_from(dir.join(name), header),
            None => Self::in_memory(),
        }
    }

    /// Loads the set from `path`; one pane id per line
    pub fn load_from(path: PathBuf, header: &'static str) -> Self {
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
            header,
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
        let mut content = String::new();
        if !self.header.is_empty() {
            content.push_str(self.header);
            content.push('\n');
        }
        for id in &self.pane_ids {
            content.push_str(id);
            content.push('\n');
        }
        std::fs::write(path, content)
    }

    /// True when the pane is in the set. An empty id is never in it.
    pub fn contains(&self, pane_id: &str) -> bool {
        !pane_id.is_empty() && self.pane_ids.contains(pane_id)
    }

    /// Adds a pane that is not in the set or removes one that is; returns
    /// whether it is in the set now. An empty id (pane id unknown) is ignored.
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

    /// Removes a pane; returns whether it was in the set
    pub fn remove(&mut self, pane_id: &str) -> bool {
        self.pane_ids.remove(pane_id)
    }

    /// Drops ids whose pane no longer exists on the tmux server.
    ///
    /// Meant for startup with the full pane list: pruning against the agent
    /// list would drop a pane every time its agent restarts.
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
        let mut hidden = PaneSet::in_memory();
        assert!(!hidden.contains("%1"));
        assert!(hidden.toggle("%1"));
        assert!(hidden.contains("%1"));
        assert!(!hidden.toggle("%1"));
        assert!(!hidden.contains("%1"));
    }

    #[test]
    fn empty_id_is_never_hidden() {
        let mut hidden = PaneSet::in_memory();
        assert!(!hidden.toggle(""));
        assert!(!hidden.contains(""));
        assert!(hidden.is_empty());
    }

    #[test]
    fn round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("hidden");

        let mut hidden = PaneSet::load_from(path.clone(), HIDDEN.1);
        assert!(hidden.is_empty());
        hidden.toggle("%12");
        hidden.toggle("%3");
        hidden.save().unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(HIDDEN.1));

        let reloaded = PaneSet::load_from(path, HIDDEN.1);
        assert_eq!(reloaded.len(), 2);
        assert!(reloaded.contains("%12"));
        assert!(reloaded.contains("%3"));
    }

    #[test]
    fn prunes_panes_that_are_gone() {
        let mut hidden = PaneSet::in_memory();
        hidden.toggle("%1");
        hidden.toggle("%2");
        assert!(hidden.retain_existing(["%2", "%9"]));
        assert!(!hidden.contains("%1"));
        assert!(hidden.contains("%2"));
        assert!(!hidden.retain_existing(["%2"]));
    }

    #[test]
    fn remove_reports_membership() {
        let mut pinned = PaneSet::in_memory();
        pinned.toggle("%4");
        assert!(pinned.remove("%4"));
        assert!(!pinned.remove("%4"));
        assert!(pinned.is_empty());
    }
}
