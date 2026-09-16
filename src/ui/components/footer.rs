use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::app::AppState;

/// Footer widget: the `/` search line and, when there is one, the error to
/// report. Key hints live in the ? help.
pub struct FooterWidget;

impl FooterWidget {
    /// Whether the footer line should be displayed at all
    pub fn is_visible(state: &AppState) -> bool {
        state.last_error.is_some() || state.search.editing || state.search.is_active()
    }

    pub fn render(frame: &mut Frame, area: Rect, state: &AppState) {
        let mut spans: Vec<Span> = Vec::new();

        if state.search.editing || state.search.is_active() {
            let matches = state.match_count();
            let total = state.agents.root_agents.len();
            let count_style = if matches == 0 {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let hint = Style::default().fg(Color::DarkGray);
            if state.search.editing {
                spans.push(Span::styled(" /", Style::default().fg(Color::Yellow)));
                spans.push(Span::styled(
                    state.search.query().to_string(),
                    Style::default().fg(Color::White),
                ));
                spans.push(Span::styled("▌", Style::default().fg(Color::Yellow)));
                spans.push(Span::styled(
                    format!("  {}/{}", matches, total),
                    count_style,
                ));
                spans.push(Span::styled(
                    "  enter: keep filter  esc: clear  ↑↓: move",
                    hint,
                ));
            } else {
                spans.push(Span::styled(
                    " filter: ",
                    Style::default().fg(Color::Yellow),
                ));
                spans.push(Span::styled(
                    state.search.query().to_string(),
                    Style::default().fg(Color::White),
                ));
                spans.push(Span::styled(
                    format!("  {}/{}", matches, total),
                    count_style,
                ));
                spans.push(Span::styled("  /: edit  esc: clear", hint));
            }
        }

        if let Some(error) = &state.last_error {
            if !spans.is_empty() {
                spans.push(Span::styled("  │", Style::default().fg(Color::DarkGray)));
            }
            spans.push(Span::styled(
                format!(" ✗ {}", truncate_error(error, 60)),
                Style::default().fg(Color::Red),
            ));
        }

        let paragraph = Paragraph::new(Line::from(spans));
        frame.render_widget(paragraph, area);
    }
}

fn truncate_error(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max_len - 1).collect::<String>())
    }
}
