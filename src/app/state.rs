use crate::agents::{AgentStatus, MonitoredAgent, SubagentStatus};
use crate::app::{attention_rank, HiddenPanes, PinnedPanes, Search, SortMode};
use crate::monitor::SystemStats;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long a status-driven reorder is held back. Statuses change every poll
/// (500 ms), and re-sorting on each change would make rows jump under the
/// cursor and renumber the 1-9 jump keys. The first change starts the clock;
/// when it runs out the list is re-sorted once with whatever the statuses are
/// then. Agents appearing or leaving and user actions (pin, hide, `o`) apply
/// immediately.
pub const SORT_SETTLE: Duration = Duration::from_secs(3);

/// Which part of the list an agent is drawn in. The list is sorted by
/// section first, so each section is a contiguous index range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    /// Pinned to the top (p)
    Pinned,
    /// The regular list
    Main,
    /// Parked in the dim list at the bottom (h)
    Hidden,
}

/// A rectangle on screen, recorded during rendering so mouse events can be
/// routed to whatever panel is under the pointer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Region {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Region {
    /// True when the given screen cell falls inside the region
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.width > 0
            && self.height > 0
            && x >= self.x
            && x < self.x + self.width
            && y >= self.y
            && y < self.y + self.height
    }
}

/// Screen regions of the panels as they were last rendered
#[derive(Debug, Clone, Copy, Default)]
pub struct Regions {
    pub sidebar: Region,
    pub preview: Region,
    pub subagent_log: Region,
}

/// A `D` waiting for its y/n answer: the pane to close and why it was asked
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillRequest {
    /// Agent id, to drop it from the list once the pane is gone
    pub agent_id: String,
    /// What `tmux kill-pane -t` gets (the stable `%12` when known)
    pub kill_target: String,
    /// `session:window.pane`, shown in the dialog
    pub target: String,
    /// Agent name and task summary, shown in the dialog
    pub label: String,
    /// What is still running in the pane
    pub reasons: Vec<String>,
}

/// What would be lost by closing `agent`'s pane right now. Empty means the
/// agent sits idle and the pane can go without asking.
///
/// `shells` is the number of shell commands the agent is running (Claude
/// Code's foreground and background Bash calls).
pub fn kill_reasons(agent: &MonitoredAgent, shells: usize) -> Vec<String> {
    let mut reasons = Vec::new();
    match &agent.status {
        AgentStatus::Processing { .. } => reasons.push("agent is working".to_string()),
        AgentStatus::AwaitingApproval { .. } => {
            reasons.push("agent is waiting for input".to_string())
        }
        AgentStatus::Unknown => reasons.push("agent status is unknown".to_string()),
        AgentStatus::Idle | AgentStatus::Done { .. } | AgentStatus::Error { .. } => {}
    }
    let subagents = agent
        .subagents
        .iter()
        .filter(|s| s.status == SubagentStatus::Running)
        .count();
    if subagents > 0 {
        reasons.push(format!(
            "{} subagent{} running",
            subagents,
            if subagents == 1 { "" } else { "s" }
        ));
    }
    if shells > 0 {
        reasons.push(format!(
            "{} shell command{} running",
            shells,
            if shells == 1 { "" } else { "s" }
        ));
    }
    reasons
}

/// Tree structure containing all monitored agents
#[derive(Debug, Clone, Default)]
pub struct AgentTree {
    /// Root agents (directly in tmux panes)
    pub root_agents: Vec<MonitoredAgent>,
}

impl AgentTree {
    /// Creates an empty agent tree
    pub fn new() -> Self {
        Self {
            root_agents: Vec::new(),
        }
    }

    /// Returns the total number of agents (including subagents)
    pub fn total_count(&self) -> usize {
        self.root_agents.iter().map(|a| 1 + a.subagents.len()).sum()
    }

    /// Returns the number of active agents (those needing attention)
    pub fn active_count(&self) -> usize {
        self.root_agents
            .iter()
            .filter(|a| a.status.needs_attention())
            .count()
    }

    /// Returns the total number of running subagents
    pub fn running_subagent_count(&self) -> usize {
        use crate::agents::SubagentStatus;
        self.root_agents
            .iter()
            .flat_map(|a| &a.subagents)
            .filter(|s| matches!(s.status, SubagentStatus::Running))
            .count()
    }

    /// Returns the number of processing agents
    pub fn processing_count(&self) -> usize {
        use crate::agents::AgentStatus;
        self.root_agents
            .iter()
            .filter(|a| matches!(a.status, AgentStatus::Processing { .. }))
            .count()
    }

    /// Gets an agent by index (for selection)
    pub fn get_agent(&self, index: usize) -> Option<&MonitoredAgent> {
        self.root_agents.get(index)
    }

    /// Gets a mutable agent by index
    pub fn get_agent_mut(&mut self, index: usize) -> Option<&mut MonitoredAgent> {
        self.root_agents.get_mut(index)
    }
}

/// Spinner frames for animation
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Main application state
#[derive(Debug)]
pub struct AppState {
    /// Tree of monitored agents
    pub agents: AgentTree,
    /// Currently selected agent index (cursor position)
    pub selected_index: usize,
    /// Panes the user parked in the dim "hidden" section (persisted)
    pub hidden: HiddenPanes,
    /// Panes the user pinned to the top of the list (persisted)
    pub pinned: PinnedPanes,
    /// Order of the list within each section (persisted)
    pub sort_mode: SortMode,
    /// Where the sort mode is saved; `None` keeps it in memory only (tests)
    pub sort_mode_path: Option<PathBuf>,
    /// When a status change first put the list out of order; the reorder
    /// waits until `SORT_SETTLE` has passed since then
    settle_since: Option<Instant>,
    /// The `/` search filtering the list and highlighting the preview
    pub search: Search,
    /// Whether help is being shown
    pub show_help: bool,
    /// Kill confirmation on screen, if any
    pub pending_kill: Option<KillRequest>,
    /// Whether subagent log is shown
    pub show_subagent_log: bool,
    /// Whether the application should quit
    pub should_quit: bool,
    /// Last error message (if any)
    pub last_error: Option<String>,
    /// Sidebar width in percentage (15-70)
    pub sidebar_width: u16,
    /// Preview scroll offset in wrapped rows counted from the bottom
    /// (0 follows the live output). Clamped while rendering, which is where the
    /// content geometry is known.
    pub preview_scroll: usize,
    /// First visible row of the agent list
    pub sidebar_scroll: usize,
    /// Whether the next render should scroll the list to reveal the cursor
    pub sidebar_follow_cursor: bool,
    /// Agent index for every row of the agent list, recorded while rendering so
    /// clicks land on the agent actually drawn there
    pub sidebar_row_agents: Vec<Option<usize>>,
    /// Panel regions from the last render, for mouse hit-testing
    pub regions: Regions,
    /// Animation tick counter
    pub tick: usize,
    /// Last tick time for animation throttling
    last_tick: Instant,
    /// System resource statistics
    pub system_stats: SystemStats,
    /// Target of the pane tmuxcc was launched from ("session:window.pane")
    pub origin_target: Option<String>,
    /// Whether the cursor still has to be moved onto the origin pane
    origin_pending: bool,
}

impl AppState {
    /// Creates a new AppState with default settings
    pub fn new() -> Self {
        Self {
            agents: AgentTree::new(),
            selected_index: 0,
            hidden: HiddenPanes::in_memory(),
            pinned: PinnedPanes::in_memory(),
            sort_mode: SortMode::default(),
            sort_mode_path: None,
            settle_since: None,
            search: Search::default(),
            show_help: false,
            pending_kill: None,
            show_subagent_log: false,
            should_quit: false,
            last_error: None,
            sidebar_width: 35,
            preview_scroll: 0,
            sidebar_scroll: 0,
            sidebar_follow_cursor: true,
            sidebar_row_agents: Vec::new(),
            regions: Regions::default(),
            tick: 0,
            last_tick: Instant::now(),
            system_stats: SystemStats::new(),
            origin_target: None,
            origin_pending: true,
        }
    }

    /// Advance the animation tick (throttled to ~10fps for spinner)
    pub fn tick(&mut self) {
        const TICK_INTERVAL_MS: u128 = 80; // ~12fps for smooth spinner
        if self.last_tick.elapsed().as_millis() >= TICK_INTERVAL_MS {
            self.tick = self.tick.wrapping_add(1);
            self.last_tick = Instant::now();
        }
    }

    /// Get the current spinner frame
    pub fn spinner_frame(&self) -> &'static str {
        SPINNER_FRAMES[self.tick % SPINNER_FRAMES.len()]
    }

    /// Returns the currently selected agent
    pub fn selected_agent(&self) -> Option<&MonitoredAgent> {
        self.agents.get_agent(self.selected_index)
    }

    /// Returns the currently selected agent mutably
    pub fn selected_agent_mut(&mut self) -> Option<&mut MonitoredAgent> {
        self.agents.get_agent_mut(self.selected_index)
    }

    /// Selects the next agent, skipping agents the search filters out
    pub fn select_next(&mut self) {
        let matching = self.matching_indices();
        if matching.is_empty() {
            return;
        }
        // From a cursor that is filtered out, "next" is the first match below it
        let next = matching
            .iter()
            .copied()
            .find(|&idx| idx > self.selected_index)
            .unwrap_or(matching[0]);
        self.selected_index = next;
        self.on_cursor_moved();
    }

    /// Selects the previous agent, skipping agents the search filters out
    pub fn select_prev(&mut self) {
        let matching = self.matching_indices();
        if matching.is_empty() {
            return;
        }
        let prev = matching
            .iter()
            .rev()
            .copied()
            .find(|&idx| idx < self.selected_index)
            .unwrap_or(matching[matching.len() - 1]);
        self.selected_index = prev;
        self.on_cursor_moved();
    }

    /// Indices (into `root_agents`, in list order) of the agents that pass
    /// the search filter. Without a query that is every agent.
    pub fn matching_indices(&self) -> Vec<usize> {
        self.agents
            .root_agents
            .iter()
            .enumerate()
            .filter(|(_, agent)| self.search.matches(agent))
            .map(|(idx, _)| idx)
            .collect()
    }

    /// Indices of the matching agents in the main (not hidden) list, in the
    /// order they are drawn. Their positions are the 1-9 jump numbers.
    pub fn visible_matching_indices(&self) -> Vec<usize> {
        let visible_count = self.visible_count();
        self.matching_indices()
            .into_iter()
            .filter(|&idx| idx < visible_count)
            .collect()
    }

    /// Number of agents that pass the search filter
    pub fn match_count(&self) -> usize {
        self.matching_indices().len()
    }

    /// Opens the search input; the existing query stays so it can be refined
    pub fn search_begin(&mut self) {
        self.search.begin();
    }

    /// Appends a character to the query and re-applies the filter
    pub fn search_input(&mut self, c: char) {
        self.search.push(c);
        self.on_search_changed();
    }

    /// Deletes the last query character and re-applies the filter
    pub fn search_backspace(&mut self) {
        self.search.pop();
        self.on_search_changed();
    }

    /// Empties the query but keeps the input open
    pub fn search_clear_input(&mut self) {
        self.search.set_query(String::new());
        self.on_search_changed();
    }

    /// Closes the input, keeping the filter on the list
    pub fn search_accept(&mut self) {
        self.search.accept();
    }

    /// Drops the filter and closes the input
    pub fn search_cancel(&mut self) {
        self.search.clear();
        self.sidebar_follow_cursor = true;
    }

    /// Keeps the cursor on a matching agent: when the agent under it drops
    /// out of the result, the cursor moves to the first match.
    fn on_search_changed(&mut self) {
        let matching = self.matching_indices();
        if matching.is_empty() || matching.contains(&self.selected_index) {
            self.sidebar_follow_cursor = true;
            return;
        }
        self.selected_index = matching[0];
        self.on_cursor_moved();
    }

    /// Selects an agent by index
    pub fn select_agent(&mut self, index: usize) {
        if index < self.agents.root_agents.len() {
            self.selected_index = index;
            self.on_cursor_moved();
        }
    }

    /// Puts the cursor on the agent running in the pane tmuxcc was launched
    /// from, so opening the popup from an agent starts on that agent.
    ///
    /// Only the first scan is used: after that the cursor belongs to the user.
    pub fn focus_origin_agent(&mut self) {
        if !self.origin_pending || self.agents.root_agents.is_empty() {
            return;
        }
        self.origin_pending = false;

        let Some(target) = self.origin_target.clone() else {
            return;
        };
        if let Some(index) = self
            .agents
            .root_agents
            .iter()
            .position(|agent| agent.target == target)
        {
            self.select_agent(index);
        }
    }

    /// Resets view state that is tied to the agent under the cursor
    fn on_cursor_moved(&mut self) {
        // A different pane means a different buffer: go back to the live tail
        self.preview_scroll = 0;
        self.sidebar_follow_cursor = true;
        // The user is driving now: never move the cursor to the origin pane
        self.origin_pending = false;
    }

    /// Scrolls the preview towards older output
    pub fn scroll_preview_back(&mut self, rows: usize) {
        self.preview_scroll = self.preview_scroll.saturating_add(rows);
    }

    /// Scrolls the preview towards the live output
    pub fn scroll_preview_forward(&mut self, rows: usize) {
        self.preview_scroll = self.preview_scroll.saturating_sub(rows);
    }

    /// Jumps the preview back to the live output
    pub fn preview_to_bottom(&mut self) {
        self.preview_scroll = 0;
    }

    /// Jumps the preview to the oldest captured output (clamped when rendering)
    pub fn preview_to_top(&mut self) {
        self.preview_scroll = usize::MAX;
    }

    /// True when the preview is showing history instead of the live output
    pub fn is_preview_scrolled(&self) -> bool {
        self.preview_scroll > 0
    }

    /// Scrolls the agent list viewport up without moving the cursor
    pub fn scroll_sidebar_up(&mut self, rows: usize) {
        self.sidebar_scroll = self.sidebar_scroll.saturating_sub(rows);
        self.sidebar_follow_cursor = false;
    }

    /// Scrolls the agent list viewport down without moving the cursor
    pub fn scroll_sidebar_down(&mut self, rows: usize) {
        self.sidebar_scroll = self.sidebar_scroll.saturating_add(rows);
        self.sidebar_follow_cursor = false;
    }

    /// Returns the agent drawn on the given row of the agent list, if any
    pub fn agent_at_sidebar_row(&self, row: usize) -> Option<usize> {
        self.sidebar_row_agents
            .get(self.sidebar_scroll + row)
            .copied()
            .flatten()
    }

    /// Section an agent belongs to. Hidden wins if a pane somehow ended up in
    /// both sets (the toggles keep them exclusive).
    pub fn section_of(&self, agent: &MonitoredAgent) -> Section {
        if self.hidden.contains(&agent.pane_id) {
            Section::Hidden
        } else if self.pinned.contains(&agent.pane_id) {
            Section::Pinned
        } else {
            Section::Main
        }
    }

    /// True when the agent at `index` sits in the hidden section
    pub fn is_hidden(&self, index: usize) -> bool {
        self.agents
            .get_agent(index)
            .map(|agent| self.section_of(agent) == Section::Hidden)
            .unwrap_or(false)
    }

    /// True when the agent at `index` sits in the pinned section
    pub fn is_pinned(&self, index: usize) -> bool {
        self.agents
            .get_agent(index)
            .map(|agent| self.section_of(agent) == Section::Pinned)
            .unwrap_or(false)
    }

    /// Number of agents in the pinned section. Because the list is sorted by
    /// section, these are exactly the indices `0..pinned_count`.
    pub fn pinned_count(&self) -> usize {
        self.agents
            .root_agents
            .iter()
            .filter(|agent| self.section_of(agent) == Section::Pinned)
            .count()
    }

    /// Number of agents in the pinned and main sections (everything but the
    /// hidden ones). Because the list is sorted hidden-last, these are exactly
    /// the indices `0..visible_count`.
    pub fn visible_count(&self) -> usize {
        self.agents
            .root_agents
            .iter()
            .filter(|agent| self.section_of(agent) != Section::Hidden)
            .count()
    }

    /// Number of agents in the hidden section
    pub fn hidden_count(&self) -> usize {
        self.agents.root_agents.len() - self.visible_count()
    }

    /// Pane id of the agent under the cursor, for hide/pin
    fn cursor_pane_id(&self, verb: &str) -> Result<String, String> {
        let Some(agent) = self.selected_agent() else {
            return Err("No agent under the cursor".to_string());
        };
        if agent.pane_id.is_empty() {
            return Err(format!("Cannot {}: pane id unknown", verb));
        }
        Ok(agent.pane_id.clone())
    }

    /// Hides the agent under the cursor, or shows it again if it was hidden,
    /// and saves the set. Hiding a pinned agent unpins it. The list is
    /// re-sorted and the cursor follows the agent to its new place. Returns
    /// the agent's new hidden state.
    pub fn toggle_hidden(&mut self) -> Result<bool, String> {
        let pane_id = self.cursor_pane_id("hide")?;
        let now_hidden = self.hidden.toggle(&pane_id);
        let unpinned = now_hidden && self.pinned.remove(&pane_id);
        self.sort_agents();
        self.hidden
            .save()
            .map_err(|e| format!("Failed to save hidden list: {}", e))?;
        if unpinned {
            self.pinned
                .save()
                .map_err(|e| format!("Failed to save pinned list: {}", e))?;
        }
        Ok(now_hidden)
    }

    /// Pins the agent under the cursor to the top of the list, or unpins it,
    /// and saves the set. Pinning a hidden agent unhides it. The list is
    /// re-sorted and the cursor follows the agent. Returns the agent's new
    /// pinned state.
    pub fn toggle_pinned(&mut self) -> Result<bool, String> {
        let pane_id = self.cursor_pane_id("pin")?;
        let now_pinned = self.pinned.toggle(&pane_id);
        let unhidden = now_pinned && self.hidden.remove(&pane_id);
        self.sort_agents();
        self.pinned
            .save()
            .map_err(|e| format!("Failed to save pinned list: {}", e))?;
        if unhidden {
            self.hidden
                .save()
                .map_err(|e| format!("Failed to save hidden list: {}", e))?;
        }
        Ok(now_pinned)
    }

    /// Switches to the next sort mode, re-sorts right away and saves it
    pub fn cycle_sort_mode(&mut self) -> Result<SortMode, String> {
        self.sort_mode = self.sort_mode.next();
        self.sort_agents();
        if let Some(path) = &self.sort_mode_path {
            self.sort_mode
                .save_to(path)
                .map_err(|e| format!("Failed to save sort mode: {}", e))?;
        }
        Ok(self.sort_mode)
    }

    /// Installs a fresh scan result (see `replace_agents_at`)
    pub fn replace_agents(&mut self, agents: AgentTree) {
        self.replace_agents_at(agents, Instant::now());
    }

    /// Installs a fresh scan result taken at `now`. The cursor stays on the
    /// agent it was on (matched by id), so agents appearing or disappearing
    /// above it do not push it onto a different row.
    ///
    /// When the scan holds the same agents as before, the on-screen order is
    /// kept and a reorder caused by status changes waits for `SORT_SETTLE`.
    /// A different set of agents is sorted right away.
    pub fn replace_agents_at(&mut self, agents: AgentTree, now: Instant) {
        let cursor_id = self.selected_agent().map(|agent| agent.id.clone());
        let previous: Vec<String> = self
            .agents
            .root_agents
            .iter()
            .map(|agent| agent.id.clone())
            .collect();
        self.agents = agents;

        let same_agents = previous.len() == self.agents.root_agents.len() && {
            let ids: HashSet<&str> = previous.iter().map(String::as_str).collect();
            self.agents
                .root_agents
                .iter()
                .all(|agent| ids.contains(agent.id.as_str()))
        };

        if same_agents {
            // Lay the fresh data out in the order already on screen
            let position = |id: &str| previous.iter().position(|p| p == id);
            self.agents
                .root_agents
                .sort_by_key(|agent| position(&agent.id));
        }
        if let Some(id) = cursor_id {
            if let Some(index) = self.agents.root_agents.iter().position(|a| a.id == id) {
                self.selected_index = index;
            }
        }
        if !same_agents {
            self.sort_agents();
            return;
        }

        if self.is_sorted() {
            self.settle_since = None;
            return;
        }
        match self.settle_since {
            None => self.settle_since = Some(now),
            Some(since) if now.duration_since(since) >= SORT_SETTLE => self.sort_agents(),
            Some(_) => {}
        }
    }

    /// True while a status-driven reorder is being held back
    pub fn is_settling(&self) -> bool {
        self.settle_since.is_some()
    }

    /// Sort key: section first (pinned, main, hidden), then the sort mode's
    /// key, then tmux order (session, window, pane) as the tie-break
    fn order_key<'a>(&self, agent: &'a MonitoredAgent) -> (Section, u8, &'a str, u32, u32) {
        let rank = match self.sort_mode {
            SortMode::Session => 0,
            SortMode::Attention => attention_rank(agent),
        };
        (
            self.section_of(agent),
            rank,
            agent.session.as_str(),
            agent.window,
            agent.pane,
        )
    }

    /// True when the list already is in `order_key` order
    fn is_sorted(&self) -> bool {
        self.agents
            .root_agents
            .windows(2)
            .all(|pair| self.order_key(&pair[0]) <= self.order_key(&pair[1]))
    }

    /// Orders the list the way it is drawn — pinned agents first, hidden ones
    /// last, each section by the sort mode — so j/k and the 1-9 jump numbers
    /// match the screen. The cursor stays on the same agent, and the viewport
    /// follows it if it moved.
    pub fn sort_agents(&mut self) {
        self.settle_since = None;
        let cursor_id = self.selected_agent().map(|agent| agent.id.clone());
        let mut agents = std::mem::take(&mut self.agents.root_agents);
        agents.sort_by(|a, b| self.order_key(a).cmp(&self.order_key(b)));
        self.agents.root_agents = agents;
        if let Some(id) = cursor_id {
            if let Some(index) = self.agents.root_agents.iter().position(|a| a.id == id) {
                if index != self.selected_index {
                    self.selected_index = index;
                    self.sidebar_follow_cursor = true;
                }
            }
        }
        if self.selected_index >= self.agents.root_agents.len() {
            self.selected_index = self.agents.root_agents.len().saturating_sub(1);
        }
    }

    /// Drops an agent whose pane was just closed, without waiting for the
    /// next scan. The cursor stays on the same row, i.e. the next agent.
    pub fn remove_agent(&mut self, id: &str) {
        let Some(index) = self.agents.root_agents.iter().position(|a| a.id == id) else {
            return;
        };
        self.agents.root_agents.remove(index);
        if index < self.selected_index {
            self.selected_index -= 1;
        }
        if self.selected_index >= self.agents.root_agents.len() {
            self.selected_index = self.agents.root_agents.len().saturating_sub(1);
        }
        self.preview_scroll = 0;
        self.sidebar_follow_cursor = true;
    }

    /// Toggles help display
    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    /// Toggles subagent log display
    pub fn toggle_subagent_log(&mut self) {
        self.show_subagent_log = !self.show_subagent_log;
    }

    /// Sets an error message
    pub fn set_error(&mut self, message: String) {
        self.last_error = Some(message);
    }

    /// Clears the error message
    pub fn clear_error(&mut self) {
        self.last_error = None;
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentStatus, AgentType, ApprovalType};

    #[test]
    fn test_app_state_navigation() {
        let mut state = AppState::new();

        // Add some agents
        state.agents.root_agents.push(MonitoredAgent::new(
            "1".to_string(),
            "main:0.0".to_string(),
            "main".to_string(),
            0,
            "code".to_string(),
            0,
            "/home/user/project1".to_string(),
            AgentType::ClaudeCode,
            1000,
        ));
        state.agents.root_agents.push(MonitoredAgent::new(
            "2".to_string(),
            "main:0.1".to_string(),
            "main".to_string(),
            0,
            "code".to_string(),
            1,
            "/home/user/project2".to_string(),
            AgentType::OpenCode,
            1001,
        ));

        assert_eq!(state.selected_index, 0);
        state.select_next();
        assert_eq!(state.selected_index, 1);
        state.select_next();
        assert_eq!(state.selected_index, 0); // Wraps around
        state.select_prev();
        assert_eq!(state.selected_index, 1); // Wraps around
    }

    #[test]
    fn test_focus_origin_agent() {
        let mut state = AppState::new();
        state.agents.root_agents.push(MonitoredAgent::new(
            "1".to_string(),
            "main:0.0".to_string(),
            "main".to_string(),
            0,
            "code".to_string(),
            0,
            "/home/user/project1".to_string(),
            AgentType::ClaudeCode,
            1000,
        ));
        state.agents.root_agents.push(MonitoredAgent::new(
            "2".to_string(),
            "main:0.1".to_string(),
            "main".to_string(),
            0,
            "code".to_string(),
            1,
            "/home/user/project2".to_string(),
            AgentType::OpenCode,
            1001,
        ));

        state.origin_target = Some("main:0.1".to_string());
        state.focus_origin_agent();
        assert_eq!(state.selected_index, 1);

        // Later scans leave the cursor where the user put it
        state.select_prev();
        state.focus_origin_agent();
        assert_eq!(state.selected_index, 0);
    }

    fn agent(id: &str, target: &str, window: u32, pane: u32, pane_id: &str) -> MonitoredAgent {
        let mut agent = MonitoredAgent::new(
            id.to_string(),
            target.to_string(),
            "main".to_string(),
            window,
            "code".to_string(),
            pane,
            "/home/user/project".to_string(),
            AgentType::ClaudeCode,
            1000,
        );
        agent.pane_id = pane_id.to_string();
        agent
    }

    #[test]
    fn test_kill_reasons() {
        use crate::agents::{ApprovalType, Subagent, SubagentType};

        let mut a = agent("a", "main:0.0", 0, 0, "%1");
        a.status = AgentStatus::Idle;
        assert!(kill_reasons(&a, 0).is_empty());

        a.status = AgentStatus::Done {
            finished: "1m".to_string(),
        };
        assert!(kill_reasons(&a, 0).is_empty());
        assert_eq!(kill_reasons(&a, 2).len(), 1);
        assert!(kill_reasons(&a, 2)[0].starts_with("2 shell commands"));

        a.status = AgentStatus::Processing {
            activity: String::new(),
        };
        assert_eq!(kill_reasons(&a, 0), vec!["agent is working".to_string()]);

        a.status = AgentStatus::AwaitingApproval {
            approval_type: ApprovalType::ShellCommand,
            details: String::new(),
        };
        a.subagents.push(Subagent::new(
            "s1".to_string(),
            SubagentType::Explore,
            String::new(),
        ));
        assert_eq!(
            kill_reasons(&a, 1),
            vec![
                "agent is waiting for input".to_string(),
                "1 subagent running".to_string(),
                "1 shell command running".to_string(),
            ]
        );
    }

    #[test]
    fn test_remove_agent_keeps_cursor_row() {
        let mut state = AppState::new();
        for (id, pane) in [("a", 0), ("b", 1), ("c", 2)] {
            state.agents.root_agents.push(agent(
                id,
                &format!("main:0.{}", pane),
                0,
                pane,
                &format!("%{}", pane),
            ));
        }
        state.selected_index = 1;
        state.remove_agent("b");
        assert_eq!(state.selected_agent().unwrap().id, "c");
        state.remove_agent("c");
        assert_eq!(state.selected_agent().unwrap().id, "a");
        state.remove_agent("missing");
        assert_eq!(state.agents.root_agents.len(), 1);
    }

    #[test]
    fn test_hidden_agents_sort_last_and_cursor_follows() {
        let mut state = AppState::new();
        state
            .agents
            .root_agents
            .push(agent("a", "main:0.0", 0, 0, "%1"));
        state
            .agents
            .root_agents
            .push(agent("b", "main:0.1", 0, 1, "%2"));
        state
            .agents
            .root_agents
            .push(agent("c", "main:1.0", 1, 0, "%3"));
        state.sort_agents();
        assert_eq!(state.visible_count(), 3);
        assert_eq!(state.hidden_count(), 0);

        // Hide the first agent: it moves to the end, the cursor goes with it
        state.select_agent(0);
        assert_eq!(state.toggle_hidden(), Ok(true));
        let ids: Vec<&str> = state
            .agents
            .root_agents
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(ids, vec!["b", "c", "a"]);
        assert_eq!(state.selected_index, 2);
        assert!(state.is_hidden(2));
        assert!(!state.is_hidden(0));
        assert_eq!(state.visible_count(), 2);
        assert_eq!(state.hidden_count(), 1);

        // A fresh scan delivers the agents in tmux order; sorting restores the
        // hidden-last layout without losing the cursor
        state.replace_agents(AgentTree {
            root_agents: vec![
                agent("a", "main:0.0", 0, 0, "%1"),
                agent("b", "main:0.1", 0, 1, "%2"),
                agent("c", "main:1.0", 1, 0, "%3"),
            ],
        });
        let ids: Vec<&str> = state
            .agents
            .root_agents
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(ids, vec!["b", "c", "a"]);
        assert_eq!(state.selected_index, 2);

        // Unhide: back in tree order, cursor still on "a"
        assert_eq!(state.toggle_hidden(), Ok(false));
        let ids: Vec<&str> = state
            .agents
            .root_agents
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn test_replace_agents_keeps_cursor_on_the_same_agent() {
        let mut state = AppState::new();
        state.replace_agents(AgentTree {
            root_agents: vec![agent("b", "main:0.1", 0, 1, "%2")],
        });
        state.select_agent(0);

        // A new agent shows up above "b": the cursor must stay on "b"
        state.replace_agents(AgentTree {
            root_agents: vec![
                agent("b", "main:0.1", 0, 1, "%2"),
                agent("a", "main:0.0", 0, 0, "%1"),
            ],
        });
        assert_eq!(state.selected_agent().map(|a| a.id.as_str()), Some("b"));
        assert_eq!(state.selected_index, 1);

        // The agent under the cursor vanishes: the index is clamped
        state.replace_agents(AgentTree {
            root_agents: vec![agent("a", "main:0.0", 0, 0, "%1")],
        });
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn test_search_filters_navigation_and_moves_the_cursor_onto_a_match() {
        let mut state = AppState::new();
        let mut a = agent("a", "main:0.0", 0, 0, "%1");
        a.title = "fix login".to_string();
        let mut b = agent("b", "main:0.1", 0, 1, "%2");
        b.title = "write docs".to_string();
        let mut c = agent("c", "main:1.0", 1, 0, "%3");
        c.last_content = "cargo test login".to_string();
        state.replace_agents(AgentTree {
            root_agents: vec![a, b, c],
        });
        state.select_agent(1);

        state.search_begin();
        for ch in "login".chars() {
            state.search_input(ch);
        }
        assert_eq!(state.matching_indices(), vec![0, 2]);
        assert_eq!(state.match_count(), 2);
        // "b" dropped out of the result: the cursor jumps to the first match
        assert_eq!(state.selected_index, 0);

        // j/k only ever land on matches
        state.select_next();
        assert_eq!(state.selected_index, 2);
        state.select_next();
        assert_eq!(state.selected_index, 0);
        state.select_prev();
        assert_eq!(state.selected_index, 2);

        // Enter keeps the filter, Esc drops it
        state.search_accept();
        assert!(!state.search.editing);
        assert!(state.search.is_active());
        state.search_cancel();
        assert!(!state.search.is_active());
        assert_eq!(state.matching_indices(), vec![0, 1, 2]);
    }

    #[test]
    fn test_search_with_no_match_keeps_the_cursor() {
        let mut state = AppState::new();
        state.replace_agents(AgentTree {
            root_agents: vec![agent("a", "main:0.0", 0, 0, "%1")],
        });
        state.search_input('z');
        assert!(state.matching_indices().is_empty());
        assert_eq!(state.selected_index, 0);
        state.select_next();
        assert_eq!(state.selected_index, 0);
        state.search_backspace();
        assert_eq!(state.matching_indices(), vec![0]);
    }

    fn ids(state: &AppState) -> Vec<&str> {
        state
            .agents
            .root_agents
            .iter()
            .map(|a| a.id.as_str())
            .collect()
    }

    fn with_status(mut agent: MonitoredAgent, status: AgentStatus) -> MonitoredAgent {
        agent.status = status;
        agent
    }

    fn waiting() -> AgentStatus {
        AgentStatus::AwaitingApproval {
            approval_type: ApprovalType::ShellCommand,
            details: String::new(),
        }
    }

    fn working() -> AgentStatus {
        AgentStatus::Processing {
            activity: String::new(),
        }
    }

    fn done() -> AgentStatus {
        AgentStatus::Done {
            finished: String::new(),
        }
    }

    /// Four idle agents in tmux order a..d
    fn four_agents() -> Vec<MonitoredAgent> {
        vec![
            agent("a", "main:0.0", 0, 0, "%1"),
            agent("b", "main:0.1", 0, 1, "%2"),
            agent("c", "main:1.0", 1, 0, "%3"),
            agent("d", "main:2.0", 2, 0, "%4"),
        ]
    }

    #[test]
    fn test_attention_order_puts_agents_needing_you_first() {
        let mut state = AppState::new();
        state.sort_mode = SortMode::Attention;
        let [a, b, c, d]: [MonitoredAgent; 4] = four_agents().try_into().unwrap();
        state.replace_agents(AgentTree {
            root_agents: vec![
                with_status(a, working()),
                b,
                with_status(c, done()),
                with_status(d, waiting()),
            ],
        });
        // waiting → done → working → idle
        assert_eq!(ids(&state), vec!["d", "c", "a", "b"]);

        // Back to session order: plain tmux order
        state.select_agent(0);
        assert_eq!(state.cycle_sort_mode(), Ok(SortMode::Session));
        assert_eq!(ids(&state), vec!["a", "b", "c", "d"]);
        // The cursor followed "d"
        assert_eq!(state.selected_agent().map(|a| a.id.as_str()), Some("d"));
    }

    #[test]
    fn test_status_reorder_waits_for_the_settle_time() {
        let mut state = AppState::new();
        state.sort_mode = SortMode::Attention;
        let t0 = Instant::now();
        state.replace_agents_at(
            AgentTree {
                root_agents: four_agents(),
            },
            t0,
        );
        assert_eq!(ids(&state), vec!["a", "b", "c", "d"]);
        state.select_agent(3);

        // "d" starts waiting: the row stays put for now
        let scan = |d_status: AgentStatus| {
            let mut agents = four_agents();
            agents[3].status = d_status;
            AgentTree {
                root_agents: agents,
            }
        };
        state.replace_agents_at(scan(waiting()), t0 + Duration::from_millis(500));
        assert_eq!(ids(&state), vec!["a", "b", "c", "d"]);
        assert!(state.is_settling());
        // The fresh status is shown even though the order is held
        assert!(state.agents.root_agents[3].status.needs_attention());

        state.replace_agents_at(scan(waiting()), t0 + Duration::from_millis(3000));
        assert_eq!(ids(&state), vec!["a", "b", "c", "d"]);

        // 3 s after the first change: reordered once, cursor still on "d"
        state.replace_agents_at(scan(waiting()), t0 + Duration::from_millis(3500));
        assert_eq!(ids(&state), vec!["d", "a", "b", "c"]);
        assert!(!state.is_settling());
        assert_eq!(state.selected_index, 0);
        assert!(state.sidebar_follow_cursor);
    }

    #[test]
    fn test_a_status_flicker_inside_the_settle_time_never_reorders() {
        let mut state = AppState::new();
        state.sort_mode = SortMode::Attention;
        let t0 = Instant::now();
        state.replace_agents_at(
            AgentTree {
                root_agents: four_agents(),
            },
            t0,
        );
        let mut agents = four_agents();
        agents[2].status = done();
        state.replace_agents_at(
            AgentTree {
                root_agents: agents,
            },
            t0 + Duration::from_secs(1),
        );
        assert!(state.is_settling());

        // Back to idle before the time ran out: nothing to do any more
        state.replace_agents_at(
            AgentTree {
                root_agents: four_agents(),
            },
            t0 + Duration::from_secs(2),
        );
        assert!(!state.is_settling());
        state.replace_agents_at(
            AgentTree {
                root_agents: four_agents(),
            },
            t0 + Duration::from_secs(5),
        );
        assert_eq!(ids(&state), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn test_a_new_agent_is_sorted_in_right_away() {
        let mut state = AppState::new();
        state.sort_mode = SortMode::Attention;
        let t0 = Instant::now();
        let mut agents = four_agents();
        let d = agents.pop().unwrap();
        state.replace_agents_at(
            AgentTree {
                root_agents: agents,
            },
            t0,
        );

        let mut agents = four_agents();
        agents[3] = with_status(d, waiting());
        state.replace_agents_at(
            AgentTree {
                root_agents: agents,
            },
            t0 + Duration::from_millis(500),
        );
        assert_eq!(ids(&state), vec!["d", "a", "b", "c"]);
        assert!(!state.is_settling());
    }

    #[test]
    fn test_pinned_agents_come_first_in_every_mode() {
        let mut state = AppState::new();
        state.replace_agents(AgentTree {
            root_agents: four_agents(),
        });

        state.select_agent(2);
        assert_eq!(state.toggle_pinned(), Ok(true));
        assert_eq!(ids(&state), vec!["c", "a", "b", "d"]);
        assert_eq!(state.selected_index, 0);
        assert_eq!(state.pinned_count(), 1);
        assert!(state.is_pinned(0));
        // Pinned agents are numbered first
        assert_eq!(state.visible_matching_indices(), vec![0, 1, 2, 3]);

        // Attention order applies below the pinned section only
        state.sort_mode = SortMode::Attention;
        let mut agents = four_agents();
        agents[3].status = waiting();
        state.replace_agents(AgentTree {
            root_agents: agents,
        });
        state.sort_agents();
        assert_eq!(ids(&state), vec!["c", "d", "a", "b"]);

        // Unpin: back into its place
        assert_eq!(state.toggle_pinned(), Ok(false));
        assert_eq!(ids(&state), vec!["d", "a", "b", "c"]);
        assert_eq!(state.pinned_count(), 0);
    }

    #[test]
    fn test_pin_and_hide_are_exclusive() {
        let mut state = AppState::new();
        state.replace_agents(AgentTree {
            root_agents: four_agents(),
        });

        state.select_agent(0);
        assert_eq!(state.toggle_pinned(), Ok(true));
        // Hiding the pinned agent unpins it
        assert_eq!(state.toggle_hidden(), Ok(true));
        assert!(!state.pinned.contains("%1"));
        assert_eq!(ids(&state), vec!["b", "c", "d", "a"]);
        assert_eq!(state.selected_index, 3);

        // Pinning a hidden agent unhides it
        assert_eq!(state.toggle_pinned(), Ok(true));
        assert!(!state.hidden.contains("%1"));
        assert_eq!(ids(&state), vec!["a", "b", "c", "d"]);
        assert_eq!(state.hidden_count(), 0);
        assert_eq!(state.pinned_count(), 1);
    }

    #[test]
    fn test_toggle_pinned_needs_a_pane_id() {
        let mut state = AppState::new();
        state
            .agents
            .root_agents
            .push(agent("a", "main:0.0", 0, 0, ""));
        assert!(state.toggle_pinned().is_err());
        assert_eq!(state.pinned_count(), 0);
    }

    #[test]
    fn test_toggle_hidden_needs_a_pane_id() {
        let mut state = AppState::new();
        state
            .agents
            .root_agents
            .push(agent("a", "main:0.0", 0, 0, ""));
        assert!(state.toggle_hidden().is_err());
        assert_eq!(state.hidden_count(), 0);
    }

    #[test]
    fn test_focus_origin_agent_without_a_match() {
        let mut state = AppState::new();
        state.agents.root_agents.push(MonitoredAgent::new(
            "1".to_string(),
            "main:0.0".to_string(),
            "main".to_string(),
            0,
            "code".to_string(),
            0,
            "/home/user/project1".to_string(),
            AgentType::ClaudeCode,
            1000,
        ));

        // Launched from a pane with no agent in it: cursor stays at the top
        state.origin_target = Some("main:9.9".to_string());
        state.focus_origin_agent();
        assert_eq!(state.selected_index, 0);
    }
}
