use std::collections::BTreeMap;

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState},
    Frame,
};

use crate::agents::{AgentStatus, AgentType, ApprovalType, MonitoredAgent, SubagentStatus};
use crate::app::AppState;

/// Widget for displaying agents in a tree organized by session/window
pub struct AgentTreeWidget;

/// Indent of a window header below its session
const WINDOW_INDENT: &str = "  ";
/// Indent between the cursor/selection column and the agent line
const AGENT_INDENT: &str = "  ";
/// Indent of the agent detail lines (aligns under the status glyph)
const DETAIL_INDENT: &str = "      ";
/// Indent of the subagent lines
const SUBAGENT_INDENT: &str = "     ";

/// Type alias for window key (window number, window name)
type WindowKey<'a> = (u32, &'a str);

/// Type alias for agents in a window (index, agent reference)
type WindowAgents<'a> = Vec<(usize, &'a MonitoredAgent)>;

/// Type alias for windows map
type WindowsMap<'a> = BTreeMap<WindowKey<'a>, WindowAgents<'a>>;

/// Type alias for sessions map
type SessionsMap<'a> = BTreeMap<&'a str, WindowsMap<'a>>;

/// Represents the hierarchical structure: Session -> Window -> Agents
struct SessionWindowTree<'a> {
    sessions: SessionsMap<'a>,
}

impl<'a> SessionWindowTree<'a> {
    fn new(agents: &'a [MonitoredAgent]) -> Self {
        let mut sessions: SessionsMap<'a> = BTreeMap::new();

        for (idx, agent) in agents.iter().enumerate() {
            sessions
                .entry(&agent.session)
                .or_default()
                .entry((agent.window, &agent.window_name))
                .or_default()
                .push((idx, agent));
        }

        Self { sessions }
    }
}

/// Rendered rows plus the agent each row belongs to.
///
/// Rows and owners are pushed together so a click can be resolved to the agent
/// actually drawn under the pointer instead of being estimated from a row count.
#[derive(Default)]
struct Rows<'a> {
    items: Vec<ListItem<'a>>,
    agent_of_row: Vec<Option<usize>>,
}

impl<'a> Rows<'a> {
    fn push(&mut self, item: ListItem<'a>, agent: Option<usize>) {
        self.items.push(item);
        self.agent_of_row.push(agent);
    }

    fn len(&self) -> usize {
        self.items.len()
    }
}

impl AgentTreeWidget {
    pub fn render(frame: &mut Frame, area: Rect, state: &mut AppState) {
        let active_count = state.agents.active_count();
        let subagent_count = state.agents.running_subagent_count();
        let selected_count = state.selected_agents.len();
        let agent_count = state.agents.root_agents.len();

        // Build title
        let title = if selected_count > 0 {
            format!(" {} sel │ {} pending ", selected_count, active_count)
        } else if subagent_count > 0 {
            format!(" {} pending │ {} subs ", active_count, subagent_count)
        } else if active_count > 0 {
            format!(" ⚠ {} pending ", active_count)
        } else {
            format!(" {} agents ", agent_count)
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan));

        if agent_count == 0 {
            let empty_text = List::new(vec![ListItem::new(Line::from(vec![Span::styled(
                "  No agents detected",
                Style::default().fg(Color::DarkGray),
            )]))])
            .block(block);
            frame.render_widget(empty_text, area);
            state.sidebar_row_agents.clear();
            state.sidebar_scroll = 0;
            return;
        }

        let visible_rows = area.height.saturating_sub(2) as usize;
        let requested_offset = state.sidebar_scroll;
        let follow_cursor = state.sidebar_follow_cursor;
        let mut offset;
        let row_agents;

        {
            let agents = &state.agents.root_agents;
            let tree = SessionWindowTree::new(agents);
            let mut rows = Rows::default();
            let available_width = area.width.saturating_sub(4) as usize;
            // First and last row of the block belonging to the cursor
            let mut cursor_span: Option<(usize, usize)> = None;

            for (session, windows) in tree.sessions.iter() {
                // Session header
                let session_line = Line::from(vec![
                    Span::styled("▼ ", Style::default().fg(Color::Cyan)),
                    Span::styled(
                        *session,
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]);
                rows.push(ListItem::new(session_line), None);

                for ((_window_num, window_name), window_agents) in windows.iter() {
                    // Window header. The window number is left out on purpose:
                    // the only numbers in the list are the 1-9 jump numbers.
                    let window_line = Line::from(vec![
                        Span::raw(WINDOW_INDENT),
                        Span::styled(*window_name, Style::default().fg(Color::Gray)),
                    ]);
                    rows.push(ListItem::new(window_line), None);

                    for (original_idx, agent) in window_agents.iter() {
                        let is_cursor = *original_idx == state.selected_index;
                        let is_selected = state.is_multi_selected(*original_idx);
                        let owner = Some(*original_idx);
                        let block_start = rows.len();

                        let select_indicator = if is_selected && is_cursor {
                            "┃☑"
                        } else if is_selected {
                            " ☑"
                        } else if is_cursor {
                            "┃ "
                        } else {
                            "  "
                        };
                        // Continuation marker so the cursor block reads as one unit
                        let cont_indicator = if is_cursor { "┃" } else { " " };
                        let detail_prefix = format!("{}{}", cont_indicator, DETAIL_INDENT);
                        let subagent_prefix = format!("{}{}", cont_indicator, SUBAGENT_INDENT);

                        // Status indicator: the glyph and its color carry the state,
                        // so no redundant "Idle"/"Working" label is printed.
                        let (status_char, status_style) = match &agent.status {
                            AgentStatus::Idle => ("●", Style::default().fg(Color::Yellow)),
                            AgentStatus::Processing { .. } => {
                                (state.spinner_frame(), Style::default().fg(Color::Green))
                            }
                            AgentStatus::AwaitingApproval { .. } => (
                                "⚠",
                                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                            ),
                            AgentStatus::Error { .. } => ("✗", Style::default().fg(Color::Red)),
                            AgentStatus::Unknown => ("○", Style::default().fg(Color::DarkGray)),
                        };

                        let type_style = match agent.agent_type {
                            AgentType::ClaudeCode => Style::default().fg(Color::Magenta),
                            AgentType::Grok => Style::default().fg(Color::Cyan),
                            AgentType::OpenCode => Style::default().fg(Color::Blue),
                            AgentType::CodexCli => Style::default().fg(Color::Green),
                            AgentType::GeminiCli => Style::default().fg(Color::Yellow),
                            AgentType::KiroCli => Style::default().fg(Color::LightMagenta),
                            AgentType::Unknown => Style::default().fg(Color::DarkGray),
                        };

                        let item_style = if is_cursor {
                            Style::default().bg(Color::Rgb(50, 50, 70))
                        } else if is_selected {
                            Style::default().bg(Color::Rgb(35, 35, 50))
                        } else {
                            Style::default()
                        };

                        // Jump number: 1-9 are reachable via digit keys
                        let number = *original_idx + 1;
                        let number_style = if number <= 9 {
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        };

                        // Main line: number + status + task summary (falls back to path)
                        let summary = agent
                            .task_summary()
                            .unwrap_or_else(|| agent.abbreviated_path());
                        let line = Line::from(vec![
                            Span::styled(
                                select_indicator,
                                if is_selected {
                                    Style::default().fg(Color::Cyan)
                                } else {
                                    Style::default().fg(Color::White)
                                },
                            ),
                            Span::raw(AGENT_INDENT),
                            Span::styled(format!("{:>2} ", number), number_style),
                            Span::styled(status_char, status_style),
                            Span::raw(" "),
                            Span::styled(summary, Style::default().fg(Color::White)),
                        ]);
                        rows.push(ListItem::new(line).style(item_style), owner);

                        // Info line: agent type + context bar
                        // (no path here — the window name above already shows the folder)
                        let mut info_parts = vec![
                            Span::styled(
                                detail_prefix.clone(),
                                Style::default().fg(Color::DarkGray),
                            ),
                            Span::styled(agent.agent_type.short_name(), type_style),
                        ];

                        // Context bar if available (percentage of the window *used*)
                        if let Some(ctx) = agent.context_used {
                            let bar_color = if ctx >= 80 {
                                Color::Red
                            } else if ctx >= 50 {
                                Color::Yellow
                            } else {
                                Color::Green
                            };
                            info_parts
                                .push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
                            info_parts.push(Span::styled(
                                context_bar(ctx),
                                Style::default().fg(bar_color),
                            ));
                        }

                        rows.push(
                            ListItem::new(Line::from(info_parts)).style(item_style),
                            owner,
                        );

                        // Status details
                        match &agent.status {
                            AgentStatus::AwaitingApproval {
                                approval_type,
                                details,
                            } => {
                                let approval_line = Line::from(vec![
                                    Span::styled(
                                        detail_prefix.clone(),
                                        Style::default().fg(Color::DarkGray),
                                    ),
                                    Span::styled("⚠ ", Style::default().fg(Color::Red)),
                                    Span::styled(
                                        format!("{}", approval_type),
                                        Style::default()
                                            .fg(Color::Red)
                                            .add_modifier(Modifier::BOLD),
                                    ),
                                ]);
                                rows.push(ListItem::new(approval_line).style(item_style), owner);

                                if !details.is_empty() {
                                    let detail_text =
                                        truncate_str(details, available_width.saturating_sub(11));
                                    let detail_line = Line::from(vec![
                                        Span::styled(
                                            detail_prefix.clone(),
                                            Style::default().fg(Color::DarkGray),
                                        ),
                                        Span::styled("→ ", Style::default().fg(Color::DarkGray)),
                                        Span::styled(
                                            detail_text,
                                            Style::default().fg(Color::White),
                                        ),
                                    ]);
                                    rows.push(ListItem::new(detail_line).style(item_style), owner);
                                }

                                if let ApprovalType::UserQuestion { choices, .. } = approval_type {
                                    for (i, choice) in choices.iter().take(4).enumerate() {
                                        let choice_text = truncate_str(
                                            choice,
                                            available_width.saturating_sub(11),
                                        );
                                        let choice_line = Line::from(vec![
                                            Span::styled(
                                                detail_prefix.clone(),
                                                Style::default().fg(Color::DarkGray),
                                            ),
                                            Span::styled(
                                                format!("{}. ", i + 1),
                                                Style::default().fg(Color::Yellow),
                                            ),
                                            Span::styled(
                                                choice_text,
                                                Style::default().fg(Color::White),
                                            ),
                                        ]);
                                        rows.push(
                                            ListItem::new(choice_line).style(item_style),
                                            owner,
                                        );
                                    }
                                    if choices.len() > 4 {
                                        let more_line = Line::from(vec![
                                            Span::styled(
                                                detail_prefix.clone(),
                                                Style::default().fg(Color::DarkGray),
                                            ),
                                            Span::styled(
                                                format!("...+{} more", choices.len() - 4),
                                                Style::default().fg(Color::DarkGray),
                                            ),
                                        ]);
                                        rows.push(
                                            ListItem::new(more_line).style(item_style),
                                            owner,
                                        );
                                    }
                                }
                            }
                            AgentStatus::Processing { activity } => {
                                // The spinner already lives on the main line, so the
                                // activity text is printed without a second one.
                                if !activity.is_empty() {
                                    let activity_text =
                                        truncate_str(activity, available_width.saturating_sub(9));
                                    let activity_line = Line::from(vec![
                                        Span::styled(
                                            detail_prefix.clone(),
                                            Style::default().fg(Color::DarkGray),
                                        ),
                                        Span::styled(
                                            activity_text,
                                            Style::default().fg(Color::DarkGray),
                                        ),
                                    ]);
                                    rows.push(
                                        ListItem::new(activity_line).style(item_style),
                                        owner,
                                    );
                                }
                            }
                            AgentStatus::Error { message } => {
                                let error_text =
                                    truncate_str(message, available_width.saturating_sub(11));
                                let error_line = Line::from(vec![
                                    Span::styled(
                                        detail_prefix.clone(),
                                        Style::default().fg(Color::DarkGray),
                                    ),
                                    Span::styled("✗ ", Style::default().fg(Color::Red)),
                                    Span::styled(error_text, Style::default().fg(Color::Red)),
                                ]);
                                rows.push(ListItem::new(error_line).style(item_style), owner);
                            }
                            _ => {}
                        }

                        // Subagents
                        for subagent in agent.subagents.iter() {
                            let (sub_char, sub_style) = match subagent.status {
                                SubagentStatus::Running => {
                                    (state.spinner_frame(), Style::default().fg(Color::Cyan))
                                }
                                SubagentStatus::Completed => {
                                    ("✓", Style::default().fg(Color::Green))
                                }
                                SubagentStatus::Failed => ("✗", Style::default().fg(Color::Red)),
                                SubagentStatus::Unknown => {
                                    ("?", Style::default().fg(Color::DarkGray))
                                }
                            };

                            let duration = if matches!(subagent.status, SubagentStatus::Running) {
                                format!(" ({})", subagent.duration_str())
                            } else {
                                String::new()
                            };

                            let sub_line = Line::from(vec![
                                Span::styled(
                                    subagent_prefix.clone(),
                                    Style::default().fg(Color::DarkGray),
                                ),
                                Span::styled("↳ ", Style::default().fg(Color::DarkGray)),
                                Span::styled(sub_char, sub_style),
                                Span::raw(" "),
                                Span::styled(
                                    subagent.subagent_type.display_name(),
                                    Style::default()
                                        .fg(Color::White)
                                        .add_modifier(Modifier::BOLD),
                                ),
                                Span::styled(duration, Style::default().fg(Color::Yellow)),
                            ]);
                            rows.push(ListItem::new(sub_line).style(item_style), owner);

                            if !subagent.description.is_empty() {
                                let desc_text = truncate_str(
                                    &subagent.description,
                                    available_width.saturating_sub(12),
                                );
                                let desc_line = Line::from(vec![
                                    Span::styled(
                                        subagent_prefix.clone(),
                                        Style::default().fg(Color::DarkGray),
                                    ),
                                    Span::raw("  "),
                                    Span::styled(desc_text, Style::default().fg(Color::DarkGray)),
                                ]);
                                rows.push(ListItem::new(desc_line).style(item_style), owner);
                            }
                        }

                        if is_cursor {
                            cursor_span = Some((block_start, rows.len()));
                        }
                    }
                }
            }

            let Rows {
                items,
                agent_of_row,
            } = rows;

            let max_offset = items.len().saturating_sub(visible_rows);
            offset = requested_offset.min(max_offset);

            // Keyboard navigation pulls the viewport along; wheel scrolling does not
            if follow_cursor {
                if let Some((start, end)) = cursor_span {
                    if end.saturating_sub(start) <= visible_rows {
                        if start < offset {
                            offset = start;
                        } else if end > offset + visible_rows {
                            offset = end - visible_rows;
                        }
                    } else {
                        offset = start;
                    }
                }
                offset = offset.min(max_offset);
            }

            let list = List::new(items).block(block);
            let mut list_state = ListState::default();
            // No item is "selected": the cursor is drawn by the row background,
            // so the offset is ours to control.
            list_state.select(None);
            *list_state.offset_mut() = offset;
            frame.render_stateful_widget(list, area, &mut list_state);

            row_agents = agent_of_row;
        }

        state.sidebar_scroll = offset;
        state.sidebar_follow_cursor = false;
        state.sidebar_row_agents = row_agents;
    }
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        format!(
            "{}..",
            s.chars()
                .take(max_len.saturating_sub(2))
                .collect::<String>()
        )
    }
}

/// Renders the fraction of the context window consumed, e.g. `ctx: 44% ████░░░░░░`.
/// The bar fills up as context is spent, so a fuller bar always means less headroom.
fn context_bar(percent: u8) -> String {
    let total_blocks = 10;
    let filled = (percent as usize * total_blocks) / 100;
    let empty = total_blocks - filled;
    format!(
        "ctx:{:>3}% {}{}",
        percent,
        "█".repeat(filled),
        "░".repeat(empty)
    )
}
