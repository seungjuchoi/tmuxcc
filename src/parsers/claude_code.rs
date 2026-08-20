use regex::Regex;

use crate::agents::{AgentStatus, AgentType, ApprovalType, Subagent, SubagentStatus, SubagentType};

use super::{safe_tail, AgentParser};

/// Check if a string looks like a version number (e.g., "2.1.11")
/// Claude Code's pane_current_command often shows version number
fn is_version_like(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Version pattern: digits and dots only, at least one dot
    let has_dot = s.contains('.');
    let all_valid = s.chars().all(|c| c.is_ascii_digit() || c == '.');
    has_dot
        && all_valid
        && s.chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
}

/// Parser for Claude Code CLI output
pub struct ClaudeCodeParser {
    // Approval patterns
    file_edit_pattern: Regex,
    file_create_pattern: Regex,
    file_delete_pattern: Regex,
    bash_pattern: Regex,
    mcp_pattern: Regex,
    general_approval_pattern: Regex,

    // Subagent patterns
    task_start_pattern: Regex,
    task_running_pattern: Regex,
    task_complete_pattern: Regex,

    // Busy line printed above the input box while a turn is running
    busy_pattern: Regex,

    // Auto-compact warning, which reports context *left*
    context_left_pattern: Regex,
    // Status-line pattern reporting context *used* (e.g. "ctx:44%")
    context_used_pattern: Regex,
}

impl ClaudeCodeParser {
    pub fn new() -> Self {
        Self {
            // Approval patterns - detect pending approval prompts
            // Claude Code uses formats like: "Yes / No", "(Y)es / (N)o", "[y/n]", "y/n"
            file_edit_pattern: Regex::new(
                r"(?i)(Edit|Write|Modify)\s+.*?\?|Do you want to (edit|write|modify)|Allow.*?edit"
            ).unwrap(),
            file_create_pattern: Regex::new(
                r"(?i)Create\s+.*?\?|Do you want to create|Allow.*?create"
            ).unwrap(),
            file_delete_pattern: Regex::new(
                r"(?i)Delete\s+.*?\?|Do you want to delete|Allow.*?delete"
            ).unwrap(),
            bash_pattern: Regex::new(
                r"(?i)(Run|Execute)\s+(command|bash|shell)|Do you want to run|Allow.*?(command|bash)|run this command"
            ).unwrap(),
            mcp_pattern: Regex::new(
                r"(?i)MCP\s+tool|Do you want to use.*?MCP|Allow.*?MCP"
            ).unwrap(),
            general_approval_pattern: Regex::new(
                r"(?i)\[y/n\]|\[Y/n\]|\[yes/no\]|\(Y\)es\s*/\s*\(N\)o|Yes\s*/\s*No|y/n|Allow\?|Do you want to (allow|proceed|continue|run|execute)"
            ).unwrap(),

            // Subagent patterns for Claude Code's Task tool
            // Match: ⏺ Task(...subagent_type="Explore"...description="..."...)
            task_start_pattern: Regex::new(
                r#"(?m)[⏺⠿⠇⠋⠙⠸⠴⠦⠧⠖⠏]\s*Task\s*\([^)]*subagent_type\s*[:=]\s*["']?(\w[\w-]*)["']?[^)]*description\s*[:=]\s*["']([^"']+)["']"#
            ).unwrap(),
            // Match running spinner indicators with agent type
            task_running_pattern: Regex::new(
                r"(?m)^[^│]*[▶►⠿⠇⠋⠙⠸⠴⠦⠧⠖⠏]\s*(\w+)(?:\s*agent)?:?\s*(.*)$"
            ).unwrap(),
            // Match completed indicators
            task_complete_pattern: Regex::new(
                r"(?m)[✓✔]\s*(\w+).*?(?:completed|finished|done|returned)"
            ).unwrap(),

            // The line Claude Code redraws above the input box while a turn is
            // running, e.g. "✳ Manifesting… (32s · ↓ 1.6k tokens)", which grows an
            // "h"/"m" prefix on longer turns ("(3m 18s · ↓ 12.9k tokens)"), or
            // "✻ Thinking… (esc to interrupt)". Once the turn ends the same slot
            // reads "✻ Crunched for 36s" — past tense and, crucially, with no
            // parenthesised timer — so the parentheses are what separate busy
            // from done. The leading glyph is deliberately not matched: Claude
            // Code cycles it (· ✢ ✳ ∗ ✻ ✽ …) and has changed the set before.
            busy_pattern: Regex::new(
                r"^\s*\S[^\n]*?\((?:((?:\d+h\s+)?(?:\d+m\s+)?\d+s)\b[^)]*|[^)]*esc to interrupt[^)]*)\)"
            ).unwrap(),

            // Context *left* pattern (e.g., "Context left until auto-compact: 42%")
            context_left_pattern: Regex::new(
                r"(?i)Context\s+(?:left|remaining).*?(\d+)%"
            ).unwrap(),

            // Context *used* as rendered by status lines (e.g., "ctx:44%")
            context_used_pattern: Regex::new(r"(?i)\bctx:\s*(\d{1,3})\s*%").unwrap(),
        }
    }

    /// Detect the "turn in progress" line near the bottom of the pane.
    ///
    /// Claude Code used to advertise this in the pane title with a spinner
    /// glyph, which is what `monitor::task` watches for; current versions keep
    /// the title fixed at `✳ <task summary>`, so the pane body is the only
    /// remaining signal.
    fn detect_busy(&self, content: &str) -> Option<AgentStatus> {
        // A wide window, because a long draft in the input box can push the
        // busy line well above the bottom of the pane.
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(40);

        for line in lines[start..].iter().rev() {
            if let Some(cap) = self.busy_pattern.captures(line) {
                let activity = match cap.get(1) {
                    Some(elapsed) => format!("Working… {}", elapsed.as_str()),
                    None => "Working…".to_string(),
                };
                return Some(AgentStatus::Processing { activity });
            }
        }

        None
    }

    fn detect_approval(&self, content: &str) -> Option<(ApprovalType, String)> {
        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() {
            return None;
        }

        // Check the last ~20 lines for active approval prompts
        let check_start = lines.len().saturating_sub(20);
        let recent_lines = &lines[check_start..];
        let recent = recent_lines.join("\n");

        // Check for user question with choices first (AskUserQuestion)
        if let Some((choices, question)) = self.extract_user_question(&recent) {
            if !choices.is_empty() {
                return Some((
                    ApprovalType::UserQuestion {
                        choices,
                        multi_select: false,
                    },
                    question,
                ));
            }
        }

        // Check for Claude Code's button-style approval (Yes / Yes, and... / No on separate lines)
        let has_yes_no_buttons = self.detect_yes_no_buttons(recent_lines);

        // Check if there's an active Yes/No prompt in the last few lines (text format)
        let last_lines: Vec<&str> = recent_lines.iter().rev().take(10).copied().collect();
        let last_text = last_lines.join("\n");
        let has_text_approval = self.general_approval_pattern.is_match(&last_text);

        if !has_yes_no_buttons && !has_text_approval {
            return None;
        }

        // Now determine the type of approval
        // Look in a slightly larger context for the type
        let context = safe_tail(content, 1500);

        if self.file_edit_pattern.is_match(context) {
            let details = self.extract_file_path(context).unwrap_or_default();
            return Some((ApprovalType::FileEdit, details));
        }

        if self.file_create_pattern.is_match(context) {
            let details = self.extract_file_path(context).unwrap_or_default();
            return Some((ApprovalType::FileCreate, details));
        }

        if self.file_delete_pattern.is_match(context) {
            let details = self.extract_file_path(context).unwrap_or_default();
            return Some((ApprovalType::FileDelete, details));
        }

        if self.bash_pattern.is_match(context) {
            let details = self.extract_command(context).unwrap_or_default();
            return Some((ApprovalType::ShellCommand, details));
        }

        if self.mcp_pattern.is_match(context) {
            return Some((ApprovalType::McpTool, "MCP tool call".to_string()));
        }

        // Generic approval
        Some((
            ApprovalType::Other("Pending approval".to_string()),
            String::new(),
        ))
    }

    /// Detect Claude Code's button-style Yes/No approval
    /// Looks for patterns like:
    ///   Yes
    ///   Yes, and don't ask again...
    ///   No
    fn detect_yes_no_buttons(&self, lines: &[&str]) -> bool {
        // Check last 8 lines for Yes/No buttons
        let check_lines: Vec<&str> = lines.iter().rev().take(8).copied().collect();

        let mut has_yes = false;
        let mut has_no = false;
        let mut yes_line_idx: Option<usize> = None;
        let mut no_line_idx: Option<usize> = None;

        for (idx, line) in check_lines.iter().enumerate() {
            let trimmed = line.trim();

            // Skip empty lines and very long lines (not buttons)
            if trimmed.is_empty() || trimmed.len() > 50 {
                continue;
            }

            // Check for "Yes" button-style lines
            // Must be short line starting with "Yes" (button format)
            if (trimmed == "Yes" || trimmed.starts_with("Yes,") || trimmed.starts_with("Yes "))
                && trimmed.len() < 40
            {
                has_yes = true;
                yes_line_idx = Some(idx);
            }

            // Check for "No" button-style lines
            if (trimmed == "No" || trimmed.starts_with("No,") || trimmed.starts_with("No "))
                && trimmed.len() < 40
            {
                has_no = true;
                no_line_idx = Some(idx);
            }
        }

        // Both Yes and No must be present and close together (within 4 lines)
        if has_yes && has_no {
            if let (Some(y_idx), Some(n_idx)) = (yes_line_idx, no_line_idx) {
                let distance = y_idx.abs_diff(n_idx);
                return distance <= 4;
            }
        }

        false
    }

    /// Extract user question with numbered choices
    /// Only detects choices at the END of content (active prompt waiting for input)
    fn extract_user_question(&self, content: &str) -> Option<(Vec<String>, String)> {
        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() {
            return None;
        }

        // Find the last prompt marker (❯ or >) - anything after this is user input area
        let last_prompt_idx = lines.iter().rposition(|line| {
            let trimmed = line.trim();
            trimmed.starts_with('❯') || (trimmed.starts_with('>') && trimmed.len() < 3)
        });

        // If there's a prompt marker, only look BEFORE it for choices
        // (Choices after the prompt are past responses, not active questions)
        let search_end = last_prompt_idx.unwrap_or(lines.len());

        // Only check the last 25 lines before the prompt
        let search_start = search_end.saturating_sub(25);
        let check_lines = &lines[search_start..search_end];

        if check_lines.is_empty() {
            return None;
        }

        let mut choices = Vec::new();
        let mut question = String::new();
        let mut first_choice_idx = None;
        let mut last_choice_idx = None;

        // Pattern for numbered choices: "1. Option text" or "  1. Option text"
        let choice_pattern = Regex::new(r"^\s*(\d+)\.\s+(.+)$").ok()?;

        for (i, line) in check_lines.iter().enumerate() {
            let trimmed = line.trim();

            // Skip lines that are clearly not choices (table borders, etc.)
            if trimmed.starts_with('│')
                || trimmed.starts_with('├')
                || trimmed.starts_with('└')
                || trimmed.starts_with('┌')
                || trimmed.starts_with('─')
                || trimmed.starts_with('✻')
            {
                if !choices.is_empty() {
                    // Non-choice content after we started - reset
                    choices.clear();
                    first_choice_idx = None;
                    last_choice_idx = None;
                }
                continue;
            }

            if let Some(cap) = choice_pattern.captures(line) {
                if let Ok(num) = cap[1].parse::<u32>() {
                    let choice_text = cap[2].trim();

                    // Accept sequential numbers starting from 1
                    if num as usize == choices.len() + 1 {
                        // Clean up choice text - remove trailing description markers
                        let label = choice_text
                            .split('（') // Japanese parenthesis
                            .next()
                            .unwrap_or(choice_text)
                            .trim();

                        choices.push(label.to_string());

                        if first_choice_idx.is_none() {
                            first_choice_idx = Some(i);
                        }
                        last_choice_idx = Some(i);
                    } else if !choices.is_empty() {
                        // Non-sequential number after we started - reset
                        choices.clear();
                        first_choice_idx = None;
                        last_choice_idx = None;
                    }
                }
            } else if !choices.is_empty() {
                // Non-choice line after choices started
                // Allow empty lines and very short lines
                if !trimmed.is_empty() && trimmed.len() > 30 {
                    // Longer content after choices - not an active question prompt
                    choices.clear();
                    first_choice_idx = None;
                    last_choice_idx = None;
                }
            }
        }

        // Choices must be near the end of check_lines (within last 8 lines)
        if let Some(last_idx) = last_choice_idx {
            if check_lines.len() - last_idx > 8 {
                return None; // Choices too far from end/prompt
            }
        }

        // Look for question text before the first choice
        if let Some(first_idx) = first_choice_idx {
            for j in (0..first_idx).rev() {
                let prev = check_lines[j].trim();
                if prev.is_empty() {
                    continue;
                }
                // Question usually ends with ? or ？
                if prev.ends_with('?')
                    || prev.ends_with('？')
                    || prev.contains('?')
                    || prev.contains('？')
                {
                    question = prev.to_string();
                    break;
                }
                // If we find a non-empty line that's not a question, use it anyway
                if question.is_empty() {
                    question = prev.to_string();
                }
                // Only look back a few lines
                if first_idx - j > 5 {
                    break;
                }
            }
        }

        if choices.len() >= 2 {
            Some((choices, question))
        } else {
            None
        }
    }

    fn extract_file_path(&self, content: &str) -> Option<String> {
        let path_pattern =
            Regex::new(r"(?m)(?:file|path)[:\s]+([^\s\n]+)|([./][\w/.-]+\.\w+)").ok()?;
        path_pattern
            .captures(content)
            .and_then(|c| c.get(1).or(c.get(2)))
            .map(|m| m.as_str().to_string())
    }

    fn extract_command(&self, content: &str) -> Option<String> {
        let cmd_pattern =
            Regex::new(r"(?m)(?:command|run)[:\s]+`([^`]+)`|```(?:bash|sh)?\n([^`]+)```").ok()?;
        cmd_pattern
            .captures(content)
            .and_then(|c| c.get(1).or(c.get(2)))
            .map(|m| m.as_str().trim().to_string())
    }
}

impl Default for ClaudeCodeParser {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentParser for ClaudeCodeParser {
    fn agent_name(&self) -> &str {
        "Claude Code"
    }

    fn agent_type(&self) -> AgentType {
        AgentType::ClaudeCode
    }

    fn matches(&self, detection_strings: &[&str]) -> bool {
        detection_strings.iter().any(|s| {
            let exe = super::executable_name(s).to_lowercase();
            // Match by executable name. Arguments are ignored on purpose: a
            // shell tool call such as `ls ~/.claude`, spawned by *another*
            // agent, used to claim the pane from here.
            exe == "claude"
                || exe.starts_with("claude-")
                || exe.starts_with("claude.")
                || exe.contains("anthropic")
                // Claude Code's pane_current_command is its version number
                || is_version_like(&exe)
        })
    }

    fn matches_title(&self, title: &str) -> bool {
        // Claude Code prefixes its pane title with `✳` (or a spinner glyph
        // while busy). Used only when the process tree yielded nothing — e.g.
        // Claude running behind an ssh/mosh hop the local `ps` cannot see.
        title.contains('✳')
    }

    fn parse_status(&self, content: &str) -> AgentStatus {
        // Check for approval prompts (highest priority)
        if let Some((approval_type, details)) = self.detect_approval(content) {
            return AgentStatus::AwaitingApproval {
                approval_type,
                details,
            };
        }

        // A running turn is read off the pane body; the title spinner check in
        // monitor/task.rs stays as a fallback for older Claude Code builds.
        if let Some(status) = self.detect_busy(content) {
            return status;
        }

        if content.trim().is_empty() {
            AgentStatus::Unknown
        } else {
            AgentStatus::Idle
        }
    }

    fn parse_subagents(&self, content: &str) -> Vec<Subagent> {
        let mut subagents = Vec::new();
        let mut id_counter = 0;

        // Find task starts
        for cap in self.task_start_pattern.captures_iter(content) {
            let subagent_type = SubagentType::parse(&cap[1]);
            let description = cap[2].to_string();
            id_counter += 1;

            subagents.push(Subagent::new(
                format!("subagent-{}", id_counter),
                subagent_type,
                description,
            ));
        }

        // Find running indicators
        for cap in self.task_running_pattern.captures_iter(content) {
            let type_name = &cap[1];
            let desc = cap.get(2).map(|m| m.as_str()).unwrap_or("");

            // Check if we already have this subagent
            let existing = subagents
                .iter()
                .any(|s| s.subagent_type.display_name().to_lowercase() == type_name.to_lowercase());

            if !existing {
                id_counter += 1;
                subagents.push(Subagent::new(
                    format!("subagent-{}", id_counter),
                    SubagentType::parse(type_name),
                    desc.to_string(),
                ));
            }
        }

        // Mark completed ones
        for cap in self.task_complete_pattern.captures_iter(content) {
            let type_name = &cap[1];
            for subagent in &mut subagents {
                if subagent.subagent_type.display_name().to_lowercase() == type_name.to_lowercase()
                {
                    subagent.status = SubagentStatus::Completed;
                }
            }
        }

        subagents
    }

    fn parse_context_used(&self, content: &str) -> Option<u8> {
        // Look for context percentage in the last portion of content
        let recent = safe_tail(content, 1000);

        // Claude Code's auto-compact warning reports what is *left*, so it has
        // to be inverted into a used reading.
        if let Some(left) = self
            .context_left_pattern
            .captures(recent)
            .and_then(|cap| cap.get(1))
            .and_then(|m| m.as_str().parse::<u16>().ok())
        {
            return Some(100u16.saturating_sub(left.min(100)) as u8);
        }

        // Custom status lines commonly surface `ctx:NN%` from the status line
        // hook's `context_window.used_percentage`, i.e. context *used*. Unlike
        // the warning above, this is present for the whole session.
        let used = self
            .context_used_pattern
            .captures(recent)
            .and_then(|cap| cap.get(1))
            .and_then(|m| m.as_str().parse::<u16>().ok())?;

        Some(used.min(100) as u8)
    }

    fn approval_keys(&self) -> &str {
        "y"
    }

    fn rejection_keys(&self) -> &str {
        "n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches() {
        let parser = ClaudeCodeParser::new();
        // Match via pane command
        assert!(parser.matches(&["claude", ""]));
        assert!(parser.matches(&["Claude", ""]));
        // Match via cmdline
        assert!(parser.matches(&["node", "/usr/bin/claude -c"]));
        // Match via version number as command (Claude Code shows version)
        assert!(parser.matches(&["2.1.11", ""]));
        assert!(parser.matches(&["1.0.0", ""]));
        // No match
        assert!(!parser.matches(&["opencode", "opencode"]));
        assert!(!parser.matches(&["fish", "fish"]));
    }

    #[test]
    fn test_a_path_mention_is_not_the_executable() {
        let parser = ClaudeCodeParser::new();
        // Regression: Grok's shell tool runs these, and every one of them used
        // to register the pane as Claude Code.
        assert!(!parser.matches(&["ls ~/.claude/"]));
        assert!(!parser.matches(&["ls ~/.claude"]));
        assert!(!parser.matches(&["cat /Users/timer/.claude/settings.json"]));
        assert!(!parser.matches(&["grep -r claude ."]));
        assert!(!parser.matches(&["/bin/bash -O extglob -c ls ~/.claude"]));
        // The real binary, wherever it lives, still matches.
        assert!(parser.matches(&["/Users/timer/.claude/local/claude --resume"]));
    }

    #[test]
    fn test_matches_title_is_the_icon_only() {
        let parser = ClaudeCodeParser::new();
        // The ✳ glyph is written by Claude Code itself.
        assert!(parser.matches_title("✳ Some Task"));
        assert!(parser.matches_title("✳ CLI取得の改善"));
        // A task summary that merely *names* Claude Code is not evidence:
        // this exact title belongs to a Grok pane.
        assert!(!parser.matches_title("Claude Code Proxy Open Source GitHub Sea… - grok"));
        assert!(!parser.matches_title("Claude Code"));
        assert!(!parser.matches_title("~"));
    }

    #[test]
    fn test_is_version_like() {
        assert!(is_version_like("2.1.11"));
        assert!(is_version_like("1.0.0"));
        assert!(is_version_like("0.1"));
        assert!(!is_version_like("fish"));
        assert!(!is_version_like("node"));
        assert!(!is_version_like(""));
        assert!(!is_version_like("2")); // No dot
    }

    #[test]
    fn test_parse_approval_file_edit() {
        let parser = ClaudeCodeParser::new();
        let content = "Do you want to edit src/main.rs? [y/n]";
        let status = parser.parse_status(content);

        match status {
            AgentStatus::AwaitingApproval { approval_type, .. } => {
                assert_eq!(approval_type, ApprovalType::FileEdit);
            }
            _ => panic!("Expected AwaitingApproval status"),
        }
    }

    #[test]
    fn test_busy_line_marks_processing() {
        let parser = ClaudeCodeParser::new();

        // Current Claude Code: gerund + elapsed timer + token counter.
        let busy = "✽ Manifesting… (32s · ↓ 1.6k tokens)\n  ⎺  Tip: use /btw\n\n────\n❯ \n";
        match parser.parse_status(busy) {
            AgentStatus::Processing { activity } => assert_eq!(activity, "Working… 32s"),
            other => panic!("expected Processing, got {:?}", other),
        }

        // Long turns grow minute (and hour) parts.
        let long = "✢ Manifesting… (3m 18s · ↓ 12.9k tokens)\n❯ \n";
        match parser.parse_status(long) {
            AgentStatus::Processing { activity } => assert_eq!(activity, "Working… 3m 18s"),
            other => panic!("expected Processing, got {:?}", other),
        }

        // Older phrasing without a timer.
        let interrupt = "✻ Thinking… (esc to interrupt)\n❯ \n";
        assert!(matches!(
            parser.parse_status(interrupt),
            AgentStatus::Processing { .. }
        ));
    }

    #[test]
    fn test_finished_turn_is_idle() {
        let parser = ClaudeCodeParser::new();

        // The same slot after the turn ends: past tense, no parenthesised timer.
        let done =
            "✻ Crunched for 36s\n\n────\n❯ \n────\n  ~/Documents/Code/tz | Opus 5 | ctx:6%\n";
        assert!(
            matches!(parser.parse_status(done), AgentStatus::Idle),
            "a finished turn must not read as Processing"
        );

        // A tool call that merely mentions a duration is not a busy line.
        let recap = "⏺ Bash(sleep 10s)\n❯ \n";
        assert!(matches!(parser.parse_status(recap), AgentStatus::Idle));
    }

    #[test]
    fn test_parse_thinking() {
        // Note: parse_status returns Idle for non-approval content.
        // Processing state is detected by spinner in pane title (monitor/task.rs).
        let parser = ClaudeCodeParser::new();
        let content = "Thinking about the problem...";
        let status = parser.parse_status(content);

        // Without approval prompt, content returns Idle
        assert!(matches!(status, AgentStatus::Idle));
    }

    #[test]
    fn test_parse_subagents() {
        let parser = ClaudeCodeParser::new();
        let content = r#"
            Task subagent_type="Explore" description="searching codebase"
            ▶ Plan: designing API
            ✓ Explore completed
        "#;

        let subagents = parser.parse_subagents(content);
        assert!(!subagents.is_empty());
    }

    #[test]
    fn test_yes_no_button_approval() {
        let parser = ClaudeCodeParser::new();
        // Claude Code button-style approval
        let content = r#"
Do you want to allow this action?

  Yes
  Yes, and don't ask again for this session
  No
"#;
        let status = parser.parse_status(content);
        match status {
            AgentStatus::AwaitingApproval { .. } => {}
            _ => panic!(
                "Expected AwaitingApproval for Yes/No buttons, got {:?}",
                status
            ),
        }
    }

    #[test]
    fn test_idle_with_prompt() {
        let parser = ClaudeCodeParser::new();
        // Content ending with prompt should be idle
        let content = "Some previous output\n\n❯ ";
        let status = parser.parse_status(content);
        assert!(
            matches!(status, AgentStatus::Idle),
            "Expected Idle, got {:?}",
            status
        );
    }

    #[test]
    fn test_parse_context_used() {
        let parser = ClaudeCodeParser::new();

        // Claude Code's own warning reports what is left, so it gets inverted.
        assert_eq!(
            parser.parse_context_used("Context left until auto-compact: 42%\n"),
            Some(58)
        );

        // A status line reporting `ctx:NN%` already reports what is *used*.
        let statusline =
            "  \\~/Documents/Code/tz/libalgo_npy_viewer master [Fable 5] ctx:44%\n  \u{23F5}\u{23F5} bypass permissions on (shift+tab to cycle)\n";
        assert_eq!(parser.parse_context_used(statusline), Some(44));

        // The explicit "left" reading wins when both are on screen.
        let both = format!("Context left until auto-compact: 12%\n{}", statusline);
        assert_eq!(parser.parse_context_used(&both), Some(88));

        assert_eq!(parser.parse_context_used("no context info here"), None);
    }

    #[test]
    fn test_no_false_positive_approval() {
        let parser = ClaudeCodeParser::new();
        // Regular text with "Yes" and "No" should NOT trigger approval
        let content = r#"
The answer is Yes or No depending on the context.
This is just normal text.
❯ "#;
        let status = parser.parse_status(content);
        assert!(
            matches!(status, AgentStatus::Idle),
            "Expected Idle (no false positive), got {:?}",
            status
        );
    }
}
