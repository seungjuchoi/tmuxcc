use regex::Regex;

use crate::agents::{AgentStatus, AgentType, ApprovalType, Subagent, SubagentStatus, SubagentType};

use super::{safe_tail, AgentParser};

/// Parser for Grok Build TUI (`grok` CLI) output
pub struct GrokParser {
    approval_pattern: Regex,
    processing_pattern: Regex,
    idle_pattern: Regex,
    // Subagent / tool-call patterns
    subagent_running_pattern: Regex,
    subagent_complete_pattern: Regex,
}

impl GrokParser {
    pub fn new() -> Self {
        Self {
            // Permission / approval prompts (without --always-approve)
            approval_pattern: Regex::new(
                r"(?i)\[y/n\]|\[Y/n\]|\[yes/no\]|\(Y\)es\s*/\s*\(N\)o|Yes\s*/\s*No|y/n|Allow\?|Do you want to (allow|proceed|continue|run|execute)|permission required|approve this|waiting for (approval|permission)",
            )
            .unwrap(),
            // Busy indicators in pane content / status line
            // e.g. "Running: Read ...", "Thought for 6.0s", "⠸ Run ...", "◆ Run ..."
            processing_pattern: Regex::new(
                r"(?i)Running:|Thought for|\bThinking\b|◈ Reading|◆ (Run|Thought)|[⠿⠇⠋⠙⠸⠴⠦⠧⠖⠏⠹⠼]\s+(Run|Read|Write|Agent|Search|Spawn)",
            )
            .unwrap(),
            // Idle: input prompt ready / previous turn finished
            idle_pattern: Regex::new(
                r"(?i)Build anything|Worked for\s+\d+|Grok\s+[\d.]+|always-approve|│\s*❯\s*│",
            )
            .unwrap(),
            // Subagents / background tasks shown as "Running: Agent ..." style lines
            subagent_running_pattern: Regex::new(
                r"(?i)(?:Running|Spawn(?:ing|ed)?|Subagent)[:\s]+(\w[\w-]*)\s*(.*)$",
            )
            .unwrap(),
            subagent_complete_pattern: Regex::new(
                r"(?i)[✓✔]\s*(\w[\w-]*).*?(?:completed|finished|done|returned)",
            )
            .unwrap(),
        }
    }
}

impl Default for GrokParser {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentParser for GrokParser {
    fn agent_name(&self) -> &str {
        "Grok"
    }

    fn agent_type(&self) -> AgentType {
        AgentType::Grok
    }

    fn matches(&self, detection_strings: &[&str]) -> bool {
        detection_strings.iter().any(|s| {
            // Executable only, so a command that merely mentions grok
            // (`ls ~/.grok`) does not register a pane as an agent.
            // Covers "grok", "grok --always-approve" and the truncated pane
            // command "grok-1.0.3-maco".
            super::executable_name(s).to_lowercase().starts_with("grok")
        })
    }

    fn matches_title(&self, title: &str) -> bool {
        // Grok appends its branding as a suffix: "<task> - grok". Anchoring to
        // the end keeps a task summary that merely says "grok" from matching.
        title.trim_end().to_lowercase().ends_with(" - grok")
    }

    fn parse_status(&self, content: &str) -> AgentStatus {
        let recent = safe_tail(content, 800);

        if self.approval_pattern.is_match(recent) {
            return AgentStatus::AwaitingApproval {
                approval_type: ApprovalType::Other("Pending".to_string()),
                details: String::new(),
            };
        }

        // Prefer processing when recent content shows active tool/thinking work.
        // Title-based braille spinner detection in monitor/task.rs also covers busy state.
        if self.processing_pattern.is_match(recent) {
            // If the most recent activity is only a finished "Worked for" footer
            // and there's an empty input prompt, treat as Idle.
            let last_lines: String = recent
                .lines()
                .rev()
                .take(12)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");

            let looks_idle_footer = last_lines.to_lowercase().contains("worked for")
                && !self.processing_pattern.is_match(&last_lines);

            if !looks_idle_footer {
                return AgentStatus::Processing {
                    activity: "Working...".to_string(),
                };
            }
        }

        if content.trim().is_empty() {
            return AgentStatus::Unknown;
        }

        if self.idle_pattern.is_match(recent) {
            return AgentStatus::Idle;
        }

        // Non-empty Grok pane without clear busy/approval signals → Idle
        AgentStatus::Idle
    }

    fn parse_subagents(&self, content: &str) -> Vec<Subagent> {
        let mut subagents: Vec<Subagent> = Vec::new();
        let mut id_counter = 0;

        for cap in self.subagent_running_pattern.captures_iter(content) {
            let type_name = &cap[1];
            let desc = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");

            let existing = subagents.iter().any(|s| {
                s.subagent_type
                    .display_name()
                    .eq_ignore_ascii_case(type_name)
            });

            if !existing {
                id_counter += 1;
                subagents.push(Subagent::new(
                    format!("subagent-{}", id_counter),
                    SubagentType::parse(type_name),
                    desc.to_string(),
                ));
            }
        }

        for cap in self.subagent_complete_pattern.captures_iter(content) {
            let type_name = &cap[1];
            for subagent in &mut subagents {
                if subagent
                    .subagent_type
                    .display_name()
                    .eq_ignore_ascii_case(type_name)
                {
                    subagent.status = SubagentStatus::Completed;
                }
            }
        }

        subagents
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
        let parser = GrokParser::new();
        // Process name
        assert!(parser.matches(&["grok", ""]));
        // Truncated binary name from pane_current_command
        assert!(parser.matches(&["grok-1.0.3-maco", ""]));
        // Cmdline
        assert!(parser.matches(&["fish", "grok --always-approve"]));
        // No match
        assert!(!parser.matches(&["claude", ""]));
        assert!(!parser.matches(&["opencode", "opencode"]));
        assert!(!parser.matches(&["fish", "fish"]));
    }

    #[test]
    fn test_matches_title_branding_suffix() {
        let parser = GrokParser::new();
        assert!(parser.matches_title("Recent Downloads Top 15 by Modification … - grok"));
        assert!(parser.matches_title("⠹ - Running: Read foo - tmuxcc Grok detection - grok"));
        // A task summary that merely mentions Grok is not the branding suffix.
        assert!(!parser.matches_title("✳ tmuxcc Grok detection"));
        assert!(!parser.matches_title("grok parser rewrite"));
        assert!(!parser.matches_title("~"));
    }

    #[test]
    fn test_parse_processing() {
        let parser = GrokParser::new();
        let content = r#"
  ┃  ◆ Run Inspect live grok tmux pane UI  [hooks: 4]
  ❙  ◆ Thought for 6.0s
  ┃  ◆ Run Capture Grok TUI states from panes  [hooks: 2]

    ⠸ Run Read `/Users/timer/Documents/Code/tz/tmuxcc/src/parsers/opencode.rs` 0.0s

  ╭────────────────────────────────────────╮
  │ ❯                                      │
  ╰──────────────────── Grok 4.5 (high) · always-approve ─╯
"#;
        let status = parser.parse_status(content);
        assert!(
            matches!(status, AgentStatus::Processing { .. }),
            "Expected Processing, got {:?}",
            status
        );
    }

    #[test]
    fn test_parse_idle() {
        let parser = GrokParser::new();
        let content = r#"
     Worked for 20s                                         stop  [hooks: 2]

  ╭────────────────────────────────────────╮
  │ ❯                                      │
  ╰──────────────────── Grok 4.5 (high) · always-approve ─╯

  Shift+Tab:mode  │  Ctrl+x:shortcuts
"#;
        let status = parser.parse_status(content);
        assert!(
            matches!(status, AgentStatus::Idle),
            "Expected Idle, got {:?}",
            status
        );
    }

    #[test]
    fn test_parse_idle_placeholder() {
        let parser = GrokParser::new();
        let content = r#"
  ╭────────────────────────────────────────╮
  │ ❯ Build anything                       │
  ╰──────────────────── Grok 4.5 (high) · always-approve ─╯
"#;
        let status = parser.parse_status(content);
        assert!(
            matches!(status, AgentStatus::Idle),
            "Expected Idle, got {:?}",
            status
        );
    }

    #[test]
    fn test_parse_approval() {
        let parser = GrokParser::new();
        let content = "Permission required to run shell command. Allow? [y/n]";
        let status = parser.parse_status(content);
        assert!(
            matches!(status, AgentStatus::AwaitingApproval { .. }),
            "Expected AwaitingApproval, got {:?}",
            status
        );
    }
}
