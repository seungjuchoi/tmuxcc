use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::app::KillRequest;

/// y/n dialog shown when `D` would close a pane that still has work running
pub struct KillConfirmWidget;

impl KillConfirmWidget {
    pub fn render(frame: &mut Frame, area: Rect, request: &KillRequest) {
        let dim = Style::default().fg(Color::DarkGray);
        let key = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);

        let mut lines = vec![
            Line::from(vec![
                Span::styled("Close pane ", Style::default().fg(Color::White)),
                Span::styled(
                    request.target.clone(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("?", Style::default().fg(Color::White)),
            ]),
            Line::from(Span::styled(request.label.clone(), dim)),
            Line::from(vec![]),
            Line::from(Span::styled(
                "Still running:",
                Style::default().fg(Color::Red),
            )),
        ];
        for reason in &request.reasons {
            lines.push(Line::from(vec![
                Span::styled("  • ", Style::default().fg(Color::Red)),
                Span::styled(reason.clone(), Style::default().fg(Color::White)),
            ]));
        }
        lines.push(Line::from(vec![]));
        lines.push(Line::from(vec![
            Span::styled("y", key),
            Span::styled(" / Enter: close    ", dim),
            Span::styled("n", key),
            Span::styled(" / Esc: keep", dim),
        ]));

        let width = area.width.saturating_sub(4).clamp(20, 60);
        let height = (lines.len() as u16 + 2).min(area.height);
        let popup = Rect {
            x: area.x + (area.width.saturating_sub(width)) / 2,
            y: area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height,
        };

        frame.render_widget(Clear, popup);
        let block = Block::default()
            .title(" Close agent ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Red));
        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, popup);
    }
}
