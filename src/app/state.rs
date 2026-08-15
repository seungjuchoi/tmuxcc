use crate::agents::MonitoredAgent;
use crate::monitor::SystemStats;
use std::collections::HashSet;
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
    /// Multi-selected agent indices
    pub selected_agents: HashSet<usize>,
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
}

impl AppState {
    /// Creates a new AppState with default settings
    pub fn new() -> Self {
        Self {
            agents: AgentTree::new(),
            selected_index: 0,
            selected_agents: HashSet::new(),
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

    /// Resets view state that is tied to the agent under the cursor
    fn on_cursor_moved(&mut self) {
        // A different pane means a different buffer: go back to the live tail
        self.preview_scroll = 0;
        self.sidebar_follow_cursor = true;
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

    /// Toggles selection of the current agent
    pub fn toggle_selection(&mut self) {
        if self.selected_agents.contains(&self.selected_index) {
            self.selected_agents.remove(&self.selected_index);
        } else {
            self.selected_agents.insert(self.selected_index);
        }
    }

    /// Selects all agents
    pub fn select_all(&mut self) {
        for i in 0..self.agents.root_agents.len() {
            self.selected_agents.insert(i);
        }
    }

    /// Clears all selections
    pub fn clear_selection(&mut self) {
        self.selected_agents.clear();
    }

    /// Returns indices to operate on (selected agents, or current if none selected)
    pub fn get_operation_indices(&self) -> Vec<usize> {
        if self.selected_agents.is_empty() {
            vec![self.selected_index]
        } else {
            let mut indices: Vec<usize> = self.selected_agents.iter().copied().collect();
            indices.sort();
            indices
        }
    }

    /// Check if an agent is in multi-selection
    pub fn is_multi_selected(&self, index: usize) -> bool {
        self.selected_agents.contains(&index)
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
}
