use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

use crate::app::{Action, AppState, Config, Region, Regions};
use crate::monitor::{MonitorTask, SystemStatsCollector};
use crate::parsers::ParserRegistry;
use crate::tmux::TmuxClient;

use super::components::{
    AgentTreeWidget, FooterWidget, HeaderWidget, HelpWidget, PanePreviewWidget, SubagentLogWidget,
};
use super::Layout;

/// Rows scrolled per mouse wheel notch
const WHEEL_STEP: usize = 3;

/// Two left clicks on the same sidebar row within this window count as a
/// double-click (crossterm reports no double-click event of its own)
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Runs the main application loop
///
/// `origin_pane` is the pane tmuxcc was launched from (`%12`, `session:1.0`,
/// ...); the cursor starts on the agent running there. When it is `None` the
/// current client's active pane is used, which inside a tmux popup is still
/// the pane the popup was opened from.
pub async fn run_app(config: Config, origin_pane: Option<String>) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Initialize state
    let mut state = AppState::new();
    state.hidden = crate::app::HiddenPanes::load();

    // Create tmux client and parser registry
    let tmux_client = Arc::new(TmuxClient::with_capture_lines(config.capture_lines));
    let parser_registry = Arc::new(ParserRegistry::new());

    // Check if tmux is available
    if !tmux_client.is_available() {
        state.set_error("tmux is not running".to_string());
    }

    // Forget hidden panes that no longer exist (pruned against *all* panes,
    // not just agent panes, so a restarting agent stays hidden)
    if let Ok(pane_ids) = tmux_client.list_pane_ids() {
        if state
            .hidden
            .retain_existing(pane_ids.iter().map(String::as_str))
        {
            if let Err(e) = state.hidden.save() {
                state.set_error(format!("Failed to save hidden list: {}", e));
            }
        }
    }

    // Remember where we were opened from, so the cursor can start there.
    // An explicit pane that tmux cannot resolve falls back to the active one.
    state.origin_target = tmux_client
        .resolve_pane_target(origin_pane.as_deref())
        .or_else(|| tmux_client.resolve_pane_target(None));

    // Create channel for monitor updates
    let (tx, mut rx) = mpsc::channel(32);

    // Start monitor task
    let monitor = MonitorTask::new(
        tmux_client.clone(),
        parser_registry.clone(),
        tx,
        Duration::from_millis(config.poll_interval_ms),
    );
    let monitor_handle = tokio::spawn(async move {
        monitor.run().await;
    });

    // Create system stats collector
    let mut system_stats = SystemStatsCollector::new();

    // Main loop
    let result = run_loop(
        &mut terminal,
        &mut state,
        &mut rx,
        &tmux_client,
        &mut system_stats,
    )
    .await;

    // Cleanup
    monitor_handle.abort();
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut AppState,
    rx: &mut mpsc::Receiver<crate::monitor::MonitorUpdate>,
    tmux_client: &TmuxClient,
    system_stats: &mut SystemStatsCollector,
) -> Result<()> {
    // Last left click on the agent list, for double-click detection
    let mut last_click: Option<(usize, Instant)> = None;

    loop {
        // Advance animation tick
        state.tick();

        // Update system stats
        system_stats.refresh();
        state.system_stats = system_stats.stats().clone();

        // Draw UI
        terminal.draw(|frame| {
            let size = frame.area();
            let main_chunks = Layout::main_layout(size, FooterWidget::is_visible(state));

            // Header
            HeaderWidget::render(frame, main_chunks[0], state);

            if Layout::is_compact(size) {
                // Narrow terminal: the list alone, full width, no preview
                let left = main_chunks[1];

                state.regions = Regions {
                    sidebar: region(left),
                    preview: Region::default(),
                    subagent_log: Region::default(),
                };

                AgentTreeWidget::render(frame, left, state);
            } else if state.show_subagent_log {
                // With subagent log: sidebar | preview | subagent_log
                let (left, preview, subagent_log) =
                    Layout::content_layout_with_log(main_chunks[1], state.sidebar_width);

                state.regions = Regions {
                    sidebar: region(left),
                    preview: region(preview),
                    subagent_log: region(subagent_log),
                };

                AgentTreeWidget::render(frame, left, state);
                PanePreviewWidget::render_detailed(frame, preview, state);
                SubagentLogWidget::render(frame, subagent_log, state);
            } else {
                // Normal: sidebar | preview
                let (left, preview) = Layout::content_layout(main_chunks[1], state.sidebar_width);

                state.regions = Regions {
                    sidebar: region(left),
                    preview: region(preview),
                    subagent_log: Region::default(),
                };

                AgentTreeWidget::render(frame, left, state);
                PanePreviewWidget::render_detailed(frame, preview, state);
            }

            // Footer (only when there is an error or an active selection)
            if FooterWidget::is_visible(state) {
                FooterWidget::render(frame, main_chunks[2], state);
            }

            // Help overlay
            if state.show_help {
                HelpWidget::render(frame, size);
            }
        })?;

        // Handle events with short timeout for responsive UI (~60fps)
        let timeout = Duration::from_millis(16);

        tokio::select! {
            // Handle monitor updates
            Some(update) = rx.recv() => {
                // Keeps the list ordered like the rendered tree (visible agents
                // by session/window/pane, hidden ones last), so j/k navigation
                // and the 1-9 jump numbers match what is displayed, and keeps
                // the cursor on the same agent.
                state.replace_agents(update.agents);
                // On the first scan, start on the agent we were opened from
                state.focus_origin_agent();
            }

            // Handle keyboard and mouse events
            _ = tokio::time::sleep(timeout) => {
                // Process all pending events to avoid input lag
                while event::poll(Duration::from_millis(0))? {
                    let event = event::read()?;

                    // Handle mouse events. Everything is routed by what sits
                    // under the pointer, using the regions recorded while drawing.
                    if let Event::Mouse(mouse) = event {
                        let regions = state.regions;
                        let (x, y) = (mouse.column, mouse.row);

                        match mouse.kind {
                            MouseEventKind::Down(MouseButton::Left) => {
                                let mut clicked = None;
                                if regions.sidebar.contains(x, y) {
                                    // Row 0 of the list sits just below the border
                                    if y > regions.sidebar.y {
                                        let row = (y - regions.sidebar.y - 1) as usize;
                                        if let Some(idx) = state.agent_at_sidebar_row(row) {
                                            state.select_agent(idx);
                                            clicked = Some(idx);
                                        }
                                    }
                                }
                                // A double-click on an agent acts like Enter:
                                // jump to its pane and close tmuxcc
                                let now = Instant::now();
                                let is_double = matches!(
                                    (clicked, last_click),
                                    (Some(a), Some((b, at))) if a == b && now.duration_since(at) <= DOUBLE_CLICK
                                );
                                if is_double {
                                    last_click = None;
                                    if let Some(agent) = state.selected_agent() {
                                        let target = agent.target.clone();
                                        if let Err(e) = tmux_client.focus_pane(&target) {
                                            state.set_error(format!("Failed to jump: {}", e));
                                        } else {
                                            state.should_quit = true;
                                        }
                                    }
                                } else {
                                    last_click = clicked.map(|idx| (idx, now));
                                }
                            }
                            MouseEventKind::ScrollUp => {
                                if regions.preview.contains(x, y) {
                                    state.scroll_preview_back(WHEEL_STEP);
                                } else if regions.sidebar.contains(x, y) {
                                    // Scrolls the list viewport, not the cursor
                                    state.scroll_sidebar_up(WHEEL_STEP);
                                }
                            }
                            MouseEventKind::ScrollDown => {
                                if regions.preview.contains(x, y) {
                                    state.scroll_preview_forward(WHEEL_STEP);
                                } else if regions.sidebar.contains(x, y) {
                                    state.scroll_sidebar_down(WHEEL_STEP);
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Handle keyboard events
                    if let Event::Key(key) = event {
                        let action = map_key_to_action(key.code, key.modifiers, state);

                        match action {
                            Action::Quit => {
                                state.should_quit = true;
                            }
                            Action::NextAgent => {
                                state.select_next();
                            }
                            Action::PrevAgent => {
                                state.select_prev();
                            }
                            Action::ToggleHidden => {
                                if let Err(e) = state.toggle_hidden() {
                                    state.set_error(e);
                                }
                            }
                            Action::Approve => {
                                if let Some(agent) = state.selected_agent() {
                                    if agent.status.needs_attention() {
                                        let target = agent.target.clone();
                                        if let Err(e) = tmux_client.send_keys(&target, "y") {
                                            state.set_error(format!("Failed to approve: {}", e));
                                        } else if let Err(e) = tmux_client.send_keys(&target, "Enter") {
                                            state.set_error(format!("Failed to send Enter: {}", e));
                                        }
                                    }
                                }
                            }
                            Action::Reject => {
                                if let Some(agent) = state.selected_agent() {
                                    if agent.status.needs_attention() {
                                        let target = agent.target.clone();
                                        if let Err(e) = tmux_client.send_keys(&target, "n") {
                                            state.set_error(format!("Failed to reject: {}", e));
                                        } else if let Err(e) = tmux_client.send_keys(&target, "Enter") {
                                            state.set_error(format!("Failed to send Enter: {}", e));
                                        }
                                    }
                                }
                            }
                            Action::ApproveAll => {
                                for agent in &state.agents.root_agents {
                                    if agent.status.needs_attention() {
                                        if let Err(e) = tmux_client.send_keys(&agent.target, "y") {
                                            state.set_error(format!("Failed to approve {}: {}", agent.target, e));
                                            break;
                                        }
                                        if let Err(e) = tmux_client.send_keys(&agent.target, "Enter") {
                                            state.set_error(format!("Failed to send Enter to {}: {}", agent.target, e));
                                            break;
                                        }
                                    }
                                }
                            }
                            Action::JumpToPane => {
                                if let Some(agent) = state.selected_agent() {
                                    let target = agent.target.clone();
                                    if let Err(e) = tmux_client.focus_pane(&target) {
                                        state.set_error(format!("Failed to jump: {}", e));
                                    } else {
                                        state.should_quit = true;
                                    }
                                }
                            }
                            Action::ToggleSubagentLog => {
                                state.toggle_subagent_log();
                            }
                            Action::Refresh => {
                                state.clear_error();
                            }
                            Action::ShowHelp => {
                                state.toggle_help();
                            }
                            Action::HideHelp => {
                                state.show_help = false;
                            }
                            Action::SidebarWider => {
                                state.sidebar_width = (state.sidebar_width + 5).min(70);
                            }
                            Action::SidebarNarrower => {
                                state.sidebar_width = state.sidebar_width.saturating_sub(5).max(15);
                            }
                            Action::SelectAgent(idx) => {
                                state.select_agent(idx);
                            }
                            Action::PreviewScrollBack(rows) => {
                                state.scroll_preview_back(rows);
                            }
                            Action::PreviewScrollForward(rows) => {
                                state.scroll_preview_forward(rows);
                            }
                            Action::PreviewPageBack => {
                                state.scroll_preview_back(preview_page(state));
                            }
                            Action::PreviewPageForward => {
                                state.scroll_preview_forward(preview_page(state));
                            }
                            Action::PreviewToTop => {
                                state.preview_to_top();
                            }
                            Action::PreviewToBottom => {
                                state.preview_to_bottom();
                            }
                            Action::SidebarScrollUp(rows) => {
                                state.scroll_sidebar_up(rows);
                            }
                            Action::SidebarScrollDown(rows) => {
                                state.scroll_sidebar_down(rows);
                            }
                            Action::SearchBegin => {
                                state.search_begin();
                            }
                            Action::SearchInput(c) => {
                                state.search_input(c);
                            }
                            Action::SearchBackspace => {
                                state.search_backspace();
                            }
                            Action::SearchClearInput => {
                                state.search_clear_input();
                            }
                            Action::SearchAccept => {
                                state.search_accept();
                            }
                            Action::SearchCancel => {
                                state.search_cancel();
                            }
                            Action::None => {}
                        }
                    }
                }
            }
        }

        if state.should_quit {
            break;
        }
    }

    Ok(())
}

/// Records a layout rectangle for later mouse hit-testing
fn region(rect: ratatui::layout::Rect) -> Region {
    Region {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

/// Half of the visible preview, used for page-wise scrolling
fn preview_page(state: &AppState) -> usize {
    let visible = state.regions.preview.height.saturating_sub(2) as usize;
    (visible / 2).max(1)
}

fn map_key_to_action(code: KeyCode, modifiers: KeyModifiers, state: &AppState) -> Action {
    // If help is shown, any key closes it
    if state.show_help {
        return Action::HideHelp;
    }

    // Search input open: printable keys go into the query. The cursor can
    // still be moved through the matches so Enter can accept-and-jump in
    // two keystrokes.
    if state.search.editing {
        return match code {
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
            KeyCode::Esc => Action::SearchCancel,
            KeyCode::Enter => Action::SearchAccept,
            KeyCode::Backspace => Action::SearchBackspace,
            KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
                Action::SearchClearInput
            }
            KeyCode::Char('n') if modifiers.contains(KeyModifiers::CONTROL) => Action::NextAgent,
            KeyCode::Char('p') if modifiers.contains(KeyModifiers::CONTROL) => Action::PrevAgent,
            KeyCode::Down | KeyCode::Tab => Action::NextAgent,
            KeyCode::Up | KeyCode::BackTab => Action::PrevAgent,
            KeyCode::Char(c)
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                Action::SearchInput(c)
            }
            _ => Action::None,
        };
    }

    // Sidebar focused
    match code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,

        // Preview scrolling: Shift+J/K (and Shift+arrows) move the preview
        // while j/k keep moving the cursor in the list
        KeyCode::Char('J') => Action::PreviewScrollForward(1),
        KeyCode::Char('K') => Action::PreviewScrollBack(1),
        KeyCode::Down if modifiers.contains(KeyModifiers::SHIFT) => Action::PreviewScrollForward(1),
        KeyCode::Up if modifiers.contains(KeyModifiers::SHIFT) => Action::PreviewScrollBack(1),
        KeyCode::PageDown => Action::PreviewPageForward,
        KeyCode::PageUp => Action::PreviewPageBack,
        KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
            Action::PreviewPageForward
        }
        KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => Action::PreviewPageBack,
        KeyCode::Char('G') => Action::PreviewToBottom,
        KeyCode::Char('g') => Action::PreviewToTop,

        KeyCode::Char('j') | KeyCode::Down => Action::NextAgent,
        KeyCode::Char('k') | KeyCode::Up => Action::PrevAgent,
        KeyCode::Tab => Action::NextAgent,

        // Left/Right arrows resize the sidebar
        KeyCode::Right => Action::SidebarWider,
        KeyCode::Left => Action::SidebarNarrower,

        // Space parks the agent under the cursor in the dim hidden section
        // at the bottom (or brings it back)
        KeyCode::Char(' ') => Action::ToggleHidden,

        // Approval
        KeyCode::Char('y') | KeyCode::Char('Y') => Action::Approve,
        KeyCode::Char('n') | KeyCode::Char('N') => Action::Reject,
        KeyCode::Char('a') | KeyCode::Char('A') => Action::ApproveAll,

        // Number keys jump the cursor to the Nth agent in the list (1-based).
        // Only the main list is numbered; hidden agents are reached with j/k.
        // With a search active the numbers count the matches that are shown.
        KeyCode::Char(c @ '1'..='9') => {
            let nth = c.to_digit(10).unwrap() as usize - 1;
            match state.visible_matching_indices().get(nth) {
                Some(&idx) => Action::SelectAgent(idx),
                None => Action::None,
            }
        }

        // Search by title and pane content
        KeyCode::Char('/') => Action::SearchBegin,

        // Enter jumps to the selected pane and closes tmuxcc
        KeyCode::Enter => Action::JumpToPane,

        KeyCode::Char('s') | KeyCode::Char('S') => Action::ToggleSubagentLog,
        KeyCode::Char('r') => Action::Refresh,

        // Sidebar resize (only < and >)
        KeyCode::Char('<') => Action::SidebarNarrower,
        KeyCode::Char('>') => Action::SidebarWider,

        KeyCode::Char('h') | KeyCode::Char('?') => Action::ShowHelp,

        // Esc clears the search filter first, then closes the subagent log;
        // with nothing to close it quits like 'q'
        KeyCode::Esc => {
            if state.search.is_active() {
                Action::SearchCancel
            } else if state.show_subagent_log {
                Action::ToggleSubagentLog
            } else {
                Action::Quit
            }
        }

        _ => Action::None,
    }
}
