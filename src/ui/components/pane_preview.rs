use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};

use crate::agents::AgentStatus;
use crate::app::AppState;

/// Widget for previewing the selected pane content
pub struct PanePreviewWidget;

impl PanePreviewWidget {
    pub fn render(frame: &mut Frame, area: Rect, state: &AppState) {
        let agent = state.selected_agent();

        let (title, content) = if let Some(agent) = agent {
            let title = format!(" Preview: {} ({}) ", agent.target, agent.agent_type);

            // Show approval details if awaiting
            let content = if let AgentStatus::AwaitingApproval {
                approval_type,
                details,
            } = &agent.status
            {
                format!(
                    "⚠ {} wants: {}\n\nDetails: {}\n\nPress [Y] to approve or [N] to reject",
                    agent.agent_type, approval_type, details
                )
            } else {
                // Show last portion of pane content
                let lines: Vec<&str> = agent.last_content.lines().collect();
                let start = lines.len().saturating_sub(20);
                lines[start..].join("\n")
            };

            (title, content)
        } else {
            (" Preview ".to_string(), "No agent selected".to_string())
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Gray));

        let paragraph = Paragraph::new(content)
            .block(block)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White));

        frame.render_widget(paragraph, area);
    }

    /// Renders a detailed preview with syntax highlighting for diffs
    pub fn render_detailed(frame: &mut Frame, area: Rect, state: &AppState) {
        let agent = state.selected_agent();

        // Calculate available lines (area height minus border)
        let available_lines = area.height.saturating_sub(2) as usize;

        let (title, lines) = if let Some(agent) = agent {
            let title = format!(" {} ({}) ", agent.target, agent.agent_type);

            let mut styled_lines: Vec<Line> = Vec::new();

            // Take enough lines to fill the area
            let content_lines: Vec<&str> = agent.last_content.lines().collect();
            let start = content_lines.len().saturating_sub(available_lines);

            for line in &content_lines[start..] {
                let spans = if line.starts_with('+') && !line.starts_with("+++") {
                    vec![Span::styled(*line, Style::default().fg(Color::Green))]
                } else if line.starts_with('-') && !line.starts_with("---") {
                    vec![Span::styled(*line, Style::default().fg(Color::Red))]
                } else if line.starts_with("@@") {
                    vec![Span::styled(*line, Style::default().fg(Color::Cyan))]
                } else if line.contains("[y/n]") || line.contains("[Y/n]") {
                    vec![Span::styled(*line, Style::default().fg(Color::Yellow))]
                } else if line.contains("⚠") || line.contains("Error") || line.contains("error") {
                    vec![Span::styled(*line, Style::default().fg(Color::Red))]
                } else if line.starts_with("❯") || line.starts_with(">") {
                    vec![Span::styled(*line, Style::default().fg(Color::Cyan))]
                } else {
                    vec![Span::raw(*line)]
                };

                styled_lines.push(Line::from(spans));
            }

            (title, styled_lines)
        } else {
            (
                " Preview ".to_string(),
                vec![Line::from(vec![Span::styled(
                    "No agent selected",
                    Style::default().fg(Color::DarkGray),
                )])],
            )
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Gray));

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });

        frame.render_widget(paragraph, area);
    }
}
