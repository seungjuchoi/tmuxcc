use crate::agents::MonitoredAgent;
use crate::app::HiddenPanes;
use crate::monitor::SystemStats;
use std::time::Instant;

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
    /// Whether help is being shown
    pub show_help: bool,
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
            show_help: false,
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

    /// Selects the next agent
    pub fn select_next(&mut self) {
        if !self.agents.root_agents.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.agents.root_agents.len();
            self.on_cursor_moved();
        }
    }

    /// Selects the previous agent
    pub fn select_prev(&mut self) {
        if !self.agents.root_agents.is_empty() {
            if self.selected_index == 0 {
                self.selected_index = self.agents.root_agents.len() - 1;
            } else {
                self.selected_index -= 1;
            }
            self.on_cursor_moved();
        }
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

    /// True when the agent at `index` sits in the hidden section
    pub fn is_hidden(&self, index: usize) -> bool {
        self.agents
            .get_agent(index)
            .map(|agent| self.hidden.contains(&agent.pane_id))
            .unwrap_or(false)
    }

    /// Number of agents in the main (not hidden) list. Because the list is
    /// sorted hidden-last, these are exactly the indices `0..visible_count`.
    pub fn visible_count(&self) -> usize {
        self.agents
            .root_agents
            .iter()
            .filter(|agent| !self.hidden.contains(&agent.pane_id))
            .count()
    }

    /// Number of agents in the hidden section
    pub fn hidden_count(&self) -> usize {
        self.agents.root_agents.len() - self.visible_count()
    }

    /// Hides the agent under the cursor, or shows it again if it was hidden,
    /// and saves the set. The list is re-sorted and the cursor follows the
    /// agent to its new place. Returns the agent's new hidden state.
    pub fn toggle_hidden(&mut self) -> Result<bool, String> {
        let Some(agent) = self.selected_agent() else {
            return Err("No agent under the cursor".to_string());
        };
        if agent.pane_id.is_empty() {
            return Err("Cannot hide: pane id unknown".to_string());
        }
        let pane_id = agent.pane_id.clone();
        let now_hidden = self.hidden.toggle(&pane_id);
        self.sort_agents();
        self.hidden
            .save()
            .map_err(|e| format!("Failed to save hidden list: {}", e))?;
        Ok(now_hidden)
    }

    /// Installs a fresh scan result. The cursor stays on the agent it was on
    /// (matched by id), so agents appearing or disappearing above it do not
    /// push it onto a different row.
    pub fn replace_agents(&mut self, agents: AgentTree) {
        let cursor_id = self.selected_agent().map(|agent| agent.id.clone());
        self.agents = agents;
        if let Some(id) = cursor_id {
            if let Some(index) = self.agents.root_agents.iter().position(|a| a.id == id) {
                self.selected_index = index;
            }
        }
        self.sort_agents();
    }

    /// Orders the list the way it is drawn — visible agents first, hidden ones
    /// last, each group by (session, window, pane) — so j/k and the 1-9 jump
    /// numbers match the screen. The cursor stays on the same agent.
    pub fn sort_agents(&mut self) {
        let cursor_id = self.selected_agent().map(|agent| agent.id.clone());
        let hidden = &self.hidden;
        self.agents.root_agents.sort_by(|a, b| {
            let key = |x: &MonitoredAgent| {
                (
                    hidden.contains(&x.pane_id),
                    x.session.clone(),
                    x.window,
                    x.pane,
                )
            };
            key(a).cmp(&key(b))
        });
        if let Some(id) = cursor_id {
            if let Some(index) = self.agents.root_agents.iter().position(|a| a.id == id) {
                self.selected_index = index;
            }
        }
        if self.selected_index >= self.agents.root_agents.len() {
            self.selected_index = self.agents.root_agents.len().saturating_sub(1);
        }
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
    use crate::agents::AgentType;

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
