use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::ansi;
use crate::app::{match_ranges, AppState};

/// Widget for previewing the selected pane content
pub struct PanePreviewWidget;

impl PanePreviewWidget {
    /// Renders the selected pane, keeping the colours the agent printed.
    ///
    /// Pane content is hard-wrapped here instead of by `Paragraph`, so a scroll
    /// offset always maps to an exact row and the newest line stays pinned to
    /// the bottom while following live output.
    pub fn render_detailed(frame: &mut Frame, area: Rect, state: &mut AppState) {
        let inner_width = area.width.saturating_sub(2) as usize;
        let inner_height = area.height.saturating_sub(2) as usize;
        let requested_scroll = state.preview_scroll;
        let mut scroll = 0usize;

        {
            let terms = state.search.terms().to_vec();
            let agent = state.selected_agent();

            let (title, border_color, visible_rows) = match agent {
                Some(agent) => {
                    // Fall back to the stripped copy when the pane had no colours
                    let source = if agent.styled_content.is_empty() {
                        agent.last_content.as_str()
                    } else {
                        agent.styled_content.as_str()
                    };

                    let mut rows: Vec<Line> = Vec::new();
                    for mut line in ansi::to_lines(source) {
                        highlight_unstyled(&mut line);
                        let line = highlight_matches(line, &terms);
                        ansi::wrap_line(&line, inner_width, &mut rows);
                    }

                    let max_scroll = rows.len().saturating_sub(inner_height);
                    scroll = requested_scroll.min(max_scroll);

                    let end = rows.len() - scroll;
                    let start = end.saturating_sub(inner_height);
                    let visible: Vec<Line> = rows[start..end].to_vec();

                    let title = if scroll > 0 {
                        format!(
                            " {} ({}) │ ↑{} of {} ",
                            agent.target, agent.agent_type, scroll, max_scroll
                        )
                    } else {
                        format!(" {} ({}) ", agent.target, agent.agent_type)
                    };

                    // A yellow border is the reminder that this is history, not live
                    let border = if scroll > 0 {
                        Color::Yellow
                    } else {
                        Color::Gray
                    };

                    (title, border, visible)
                }
                None => (
                    " Preview ".to_string(),
                    Color::Gray,
                    vec![Line::from(vec![Span::styled(
                        "No agent selected",
                        Style::default().fg(Color::DarkGray),
                    )])],
                ),
            };

            let block = Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(border_color));

            frame.render_widget(Paragraph::new(visible_rows).block(block), area);
        }

        state.preview_scroll = scroll;
    }
}

/// Marks every occurrence of the search terms so a match found through the
/// pane content can be spotted in the preview. Spans are split around the
/// hits; the rest of the line keeps its own colours.
fn highlight_matches<'a>(line: Line<'a>, terms: &[String]) -> Line<'a> {
    if terms.is_empty() {
        return line;
    }
    let mark = Style::default()
        .fg(Color::Black)
        .bg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    let mut spans: Vec<Span<'a>> = Vec::with_capacity(line.spans.len());
    for span in line.spans {
        let ranges = match_ranges(&span.content, terms);
        if ranges.is_empty() {
            spans.push(span);
            continue;
        }
        let text = span.content.to_string();
        let mut cursor = 0;
        for (start, end) in ranges {
            if start > cursor {
                spans.push(Span::styled(text[cursor..start].to_string(), span.style));
            }
            spans.push(Span::styled(text[start..end].to_string(), mark));
            cursor = end;
        }
        if cursor < text.len() {
            spans.push(Span::styled(text[cursor..].to_string(), span.style));
        }
    }
    Line::from(spans)
}

/// Applies fallback highlighting to lines the agent left uncoloured.
fn highlight_unstyled(line: &mut Line<'_>) {
    if !ansi::line_is_unstyled(line) {
        return;
    }
    // An uncoloured line is a single text span
    let Some(span) = line.spans.first_mut() else {
        return;
    };
    if let Some(style) = fallback_style(span.content.as_ref()) {
        span.style = style;
    }
}

/// Guesses a colour for plain-text output (diffs, prompts, errors).
fn fallback_style(text: &str) -> Option<Style> {
    let trimmed = text.trim_start();

    if trimmed.starts_with("@@") {
        return Some(Style::default().fg(Color::Cyan));
    }
    if trimmed.starts_with("+++") || trimmed.starts_with("---") {
        return Some(Style::default().fg(Color::DarkGray));
    }
    if trimmed.starts_with('+') {
        return Some(Style::default().fg(Color::Green));
    }
    if trimmed.starts_with('-') && !trimmed.starts_with("--") {
        return Some(Style::default().fg(Color::Red));
    }
    if trimmed.starts_with('❯') || trimmed.starts_with('>') || trimmed.starts_with('$') {
        return Some(Style::default().fg(Color::Cyan));
    }
    if trimmed.starts_with('✓') || trimmed.starts_with('✔') {
        return Some(Style::default().fg(Color::Green));
    }
    if text.contains("[y/n]") || text.contains("[Y/n]") || text.contains("(y/N)") {
        return Some(Style::default().fg(Color::Yellow));
    }
    if text.contains('⚠') || text.contains("Warning") || text.contains("warning") {
        return Some(Style::default().fg(Color::Yellow));
    }
    if text.contains('✗')
        || text.contains("Error")
        || text.contains("error")
        || text.contains("failed")
        || text.contains("Failed")
    {
        return Some(Style::default().fg(Color::Red));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colored_lines_keep_their_own_style() {
        let mut line = ansi::to_lines("\x1b[34merror in file\x1b[39m")
            .pop()
            .unwrap();
        highlight_unstyled(&mut line);
        // The fallback must not override what the agent printed
        assert_eq!(line.spans[0].style.fg, Some(Color::Blue));
    }

    #[test]
    fn search_hits_are_marked_without_losing_the_rest() {
        let line = ansi::to_lines("\x1b[32mcargo test passed\x1b[39m")
            .pop()
            .unwrap();
        let line = highlight_matches(line, &["TEST".to_string()]);
        let texts: Vec<&str> = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(texts, vec!["cargo ", "test", " passed"]);
        assert_eq!(line.spans[0].style.fg, Some(Color::Green));
        assert_eq!(line.spans[1].style.bg, Some(Color::Yellow));
        assert_eq!(line.spans[2].style.fg, Some(Color::Green));
    }

    #[test]
    fn plain_lines_get_fallback_colors() {
        let mut line = ansi::to_lines("+added line").pop().unwrap();
        highlight_unstyled(&mut line);
        assert_eq!(line.spans[0].style.fg, Some(Color::Green));
    }
}
