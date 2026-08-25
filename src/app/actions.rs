/// Actions that can be performed in the application
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Quit the application
    Quit,
    /// Navigate to next agent
    NextAgent,
    /// Navigate to previous agent
    PrevAgent,
    /// Park the current agent in the dim hidden section, or bring it back
    ToggleHidden,
    /// Approve the current agent's pending request
    Approve,
    /// Reject the current agent's pending request
    Reject,
    /// Approve all pending requests
    ApproveAll,
    /// Jump to the selected tmux pane and quit
    JumpToPane,
    /// Toggle subagent log view
    ToggleSubagentLog,
    /// Refresh agent list
    Refresh,
    /// Show help
    ShowHelp,
    /// Hide help
    HideHelp,
    /// Increase sidebar width
    SidebarWider,
    /// Decrease sidebar width
    SidebarNarrower,
    /// Select agent by index (mouse click)
    SelectAgent(usize),
    /// Scroll the preview towards older output
    PreviewScrollBack(usize),
    /// Scroll the preview towards the live output
    PreviewScrollForward(usize),
    /// Jump the preview to the oldest captured output
    PreviewToTop,
    /// Jump the preview back to the live output
    PreviewToBottom,
    /// Scroll the preview back by half a screen
    PreviewPageBack,
    /// Scroll the preview forward by half a screen
    PreviewPageForward,
    /// Scroll the agent list viewport up (cursor stays put)
    SidebarScrollUp(usize),
    /// Scroll the agent list viewport down (cursor stays put)
    SidebarScrollDown(usize),
    /// No action (used for unbound keys)
    None,
}

impl Action {
    /// Returns a description of the action for help display
    pub fn description(&self) -> &str {
        match self {
            Action::Quit => "Quit application",
            Action::NextAgent => "Select next agent",
            Action::PrevAgent => "Select previous agent",
            Action::ToggleHidden => "Hide / unhide current agent",
            Action::Approve => "Approve pending request",
            Action::Reject => "Reject pending request",
            Action::ApproveAll => "Approve all pending requests",
            Action::JumpToPane => "Jump to selected pane and quit",
            Action::ToggleSubagentLog => "Toggle subagent log",
            Action::Refresh => "Refresh agent list",
            Action::ShowHelp => "Show help",
            Action::HideHelp => "Hide help",
            Action::SidebarWider => "Widen sidebar",
            Action::SidebarNarrower => "Narrow sidebar",
            Action::SelectAgent(_) => "Select agent",
            Action::PreviewScrollBack(_) => "Scroll preview back",
            Action::PreviewScrollForward(_) => "Scroll preview forward",
            Action::PreviewToTop => "Scroll preview to oldest output",
            Action::PreviewToBottom => "Follow live preview output",
            Action::PreviewPageBack => "Scroll preview back half a screen",
            Action::PreviewPageForward => "Scroll preview forward half a screen",
            Action::SidebarScrollUp(_) => "Scroll agent list up",
            Action::SidebarScrollDown(_) => "Scroll agent list down",
            Action::None => "",
        }
    }
}
