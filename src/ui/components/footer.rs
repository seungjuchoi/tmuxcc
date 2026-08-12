use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::app::AppState;

/// Footer widget: shown only when there is something to say
/// (an error or an active multi-selection). Key hints live in the ? help.
pub struct FooterWidget;

impl FooterWidget {
    /// Whether the footer line should be displayed at all
    pub fn is_visible(state: &AppState) -> bool {
        state.last_error.is_some() || !state.selected_agents.is_empty()
    }

    pub fn render(frame: &mut Frame, area: Rect, state: &AppState) {
        let mut spans: Vec<Span> = Vec::new();

        if !state.selected_agents.is_empty() {
            spans.push(Span::styled(
                format!(" {} selected", state.selected_agents.len()),
                Style::default().fg(Color::Cyan),
            ));
        }

        if let Some(error) = &state.last_error {
            if !spans.is_empty() {
                spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
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
