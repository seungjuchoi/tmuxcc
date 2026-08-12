use ratatui::layout::{Constraint, Direction, Rect};

/// Layout manager for the application
pub struct Layout;

impl Layout {
    /// Creates the main layout with header, content, and an optional
    /// footer line (used only for errors / selection count)
    pub fn main_layout(area: Rect, show_footer: bool) -> Vec<Rect> {
        let footer_height = if show_footer { 1 } else { 0 };
        ratatui::layout::Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),             // Header
                Constraint::Min(10),               // Content area
                Constraint::Length(footer_height), // Footer (errors/selection only)
            ])
            .split(area)
            .to_vec()
    }

    /// Splits the content area into 2 columns: agent list (left) and preview (right)
    pub fn content_layout(area: Rect, sidebar_width: u16) -> (Rect, Rect) {
        let chunks = ratatui::layout::Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(sidebar_width),
                Constraint::Percentage(100 - sidebar_width),
            ])
            .split(area);
        (chunks[0], chunks[1])
    }

    /// Splits the content area with preview and input
    /// Returns (sidebar, preview, input)
    pub fn content_layout_with_input(
        area: Rect,
        sidebar_width: u16,
        input_height: u16,
    ) -> (Rect, Rect, Rect) {
        let columns = ratatui::layout::Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(sidebar_width),
                Constraint::Percentage(100 - sidebar_width),
            ])
            .split(area);

        let right_side = ratatui::layout::Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),                   // Preview (pane content)
                Constraint::Length(input_height + 2), // Input area (+ border)
            ])
            .split(columns[1]);

        (columns[0], right_side[0], right_side[1])
    }

    /// Splits the content area with subagent log (2 columns, right side split vertically)
    pub fn content_layout_with_log(area: Rect, sidebar_width: u16) -> (Rect, Rect, Rect) {
        let columns = ratatui::layout::Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(sidebar_width),
                Constraint::Percentage(100 - sidebar_width),
            ])
            .split(area);

        let right_side = ratatui::layout::Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(60), // Preview
                Constraint::Percentage(40), // Subagent log
            ])
            .split(columns[1]);

        (columns[0], right_side[0], right_side[1])
    }

    /// Creates a centered popup area
    pub fn centered_popup(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
        let vertical = ratatui::layout::Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage((100 - height_percent) / 2),
                Constraint::Percentage(height_percent),
                Constraint::Percentage((100 - height_percent) / 2),
            ])
            .split(area);

        ratatui::layout::Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage((100 - width_percent) / 2),
                Constraint::Percentage(width_percent),
                Constraint::Percentage((100 - width_percent) / 2),
            ])
            .split(vertical[1])[1]
    }
}
