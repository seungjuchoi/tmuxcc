use regex::Regex;

use crate::agents::{AgentStatus, AgentType, ApprovalType, Subagent, SubagentStatus, SubagentType};

use super::AgentParser;

/// Marker left on the pane shell by Kiro CLI's shell integration
/// (the pane process shows up as `fish (kiro-cli-term)`).
///
/// This appears in **every** pane spawned from a Kiro-integrated terminal —
/// including plain shells and panes running a different agent — so it must
/// never be treated as evidence of a running Kiro CLI session.
const SHELL_INTEGRATION_MARKER: &str = "kiro-cli-term";

/// Executable names that mean "a Kiro CLI chat session lives in this pane".
///
/// Observed process tree for a Kiro pane:
///   fish (kiro-cli-term)                      <- pane process (marker only)
///     fish --login
///       kiro-cli chat --trust-all-tools --v3  <- matched here
///         kiro-cli-chat chat ...              <- and here
///           .../kiro-cli/bun .../tui.js chat ...
const KIRO_BINARIES: [&str; 4] = ["kiro", "kiro-cli", "kiro-cli-chat", "kiro_cli"];

/// Hint-line prefixes Kiro CLI renders above the prompt while it is busy.
const PROCESSING_HINTS: [&str; 6] = [
    "Kiro is working",
    "Compacting conversation",
    "Initializing",
    "running shell command",
    "Goal Active:",
    "Editing queued message",
];

/// Hint lines that only render when the turn is finished and input is accepted.
const IDLE_HINTS: [&str; 4] = [
    "ask a question or describe a task",
    "ask a question, or describe a task",
    "/copy to clipboard",
    "describe what",
];

/// Option labels of the tool-approval dropdown.
const APPROVAL_OPTIONS: [&str; 6] = [
    "Yes, single permission",
    "No (Tab to edit)",
    "Trust, always allow in this session",
    "Trust, allow all for this session",
    "Trust entire tool",
    "(a) Approve all pending",
];

/// Last option of the question panel — a stable marker that Kiro is blocked
/// on an `AskUserQuestion`-style prompt.
const QUESTION_MARKER: &str = "Type a different answer";

/// True when `token` names a Kiro CLI executable (and not the shell marker).
fn is_kiro_binary(token: &str) -> bool {
    let base = token
        .rsplit('/')
        .next()
        .unwrap_or(token)
        .trim_matches(|c: char| matches!(c, '(' | ')' | '"' | '\'' | ',' | ';' | '[' | ']'))
        .to_ascii_lowercase();

    if base.starts_with(SHELL_INTEGRATION_MARKER) {
        return false;
    }

    KIRO_BINARIES.contains(&base.as_str())
}

/// Returns the last `n` lines of `content`, rejoined.
fn tail_lines(content: &str, n: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Strips a select-list selection indicator (`❯`, `>`) plus surrounding
/// whitespace, so a rendered option row can be compared against its label.
fn strip_row_prefix(line: &str) -> &str {
    line.trim()
        .trim_start_matches(['❯', '›', '▸', '>', '*'])
        .trim()
}

/// Counts lines that *are* an approval-dropdown option row.
///
/// Anchoring to the start of the line matters: an agent writing *about*
/// approvals ("... the `Yes, single permission` option ...") must not be
/// mistaken for a live prompt. Requiring two distinct rows adds another guard —
/// the dropdown always renders at least an accept and a reject row.
fn approval_option_rows(tail: &str) -> usize {
    let mut matched: Vec<&str> = Vec::new();
    for line in tail.lines() {
        let row = strip_row_prefix(line);
        if let Some(option) = APPROVAL_OPTIONS
            .iter()
            .find(|o| row.starts_with(**o) && !matched.contains(o))
        {
            matched.push(option);
        }
    }
    matched.len()
}

/// Maps a Kiro tool display name (`Shell`, `Write`, `Read`, ...) to an approval type.
fn classify_tool(tool: &str) -> ApprovalType {
    let lower = tool.to_lowercase();
    match lower.as_str() {
        "shell" => ApprovalType::ShellCommand,
        "write" => ApprovalType::FileEdit,
        "read" | "glob" | "grep" | "ls" | "imageread" => ApprovalType::Other("Read".to_string()),
        "webfetch" | "websearch" => ApprovalType::Other("Web".to_string()),
        "subagent" | "task" => ApprovalType::Other("Subagent".to_string()),
        "" => ApprovalType::Other("Pending approval".to_string()),
        _ if lower.contains("mcp") || lower.contains('@') => ApprovalType::McpTool,
        _ => ApprovalType::Other(tool.to_string()),
    }
}

/// Parser for Kiro CLI (`kiro-cli chat`) output
pub struct KiroCliParser {
    /// `Shell · <detail> requires approval` snackbar shown while a tool waits
    approval_banner: Regex,
    /// Numbered choices of the question panel (`1. Option`)
    question_choice: Regex,
    /// In-progress tool line, prefixed with one of Kiro's spinner glyphs
    running_tool: Regex,
    /// `subagent <status>` line rendered for subagent/pipeline tool calls
    subagent_line: Regex,
    /// `    ├─ [stage-name]` pipeline stage line
    pipeline_stage: Regex,
    /// Status bar context indicator (`◔ 6%`) — percentage of context **used**
    context_used: Regex,
    /// `12% context used` from the /context breakdown panel
    context_used_verbose: Regex,
}

impl KiroCliParser {
    pub fn new() -> Self {
        Self {
            // Title is `${tool}${" · " + detail} requires approval`.
            // smallDot is "·" in unicode mode and "." with ASCII glyphs.
            approval_banner: Regex::new(
                r"(?m)^\s*([A-Za-z_][\w@.-]*)\s*(?:[·.]\s*(.*?))?\s*requires approval\b",
            )
            .unwrap(),
            question_choice: Regex::new(r"(?m)^\s*(?:[❯>]\s*)?(\d+)\.\s+(.+?)\s*$").unwrap(),
            // pie / quarter / braille spinner frames. `●` is excluded: it is
            // also the "tool finished" marker.
            running_tool: Regex::new(r"(?m)^\s*[◔◑◕◐◓◒⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏⠁⠉⢹⣹⣽⣿]\s+(\S.*?)\s*$").unwrap(),
            subagent_line: Regex::new(r"(?m)^\s*subagent\b[ \t]*(.*?)\s*$").unwrap(),
            pipeline_stage: Regex::new(r"(?m)^[\s\x{2500}-\x{257F}|+\-`]*\[([^\]]+)\]").unwrap(),
            context_used: Regex::new(r"[·.]\s*[◔◑◕●○]\s*(\d{1,3})\s*%").unwrap(),
            context_used_verbose: Regex::new(r"(\d{1,3})\s*%\s*context\s+used").unwrap(),
        }
    }

    /// Detects the question panel and extracts its choices.
    fn detect_question(&self, tail: &str) -> Option<(ApprovalType, String)> {
        // The free-form option is a row of its own; matching it mid-line would
        // trip on transcript text that merely names it.
        if !tail
            .lines()
            .any(|line| strip_row_prefix(line).starts_with(QUESTION_MARKER))
        {
            return None;
        }

        let mut choices: Vec<String> = Vec::new();
        let mut first_choice_line: Option<usize> = None;

        for (idx, line) in tail.lines().enumerate() {
            if let Some(cap) = self.question_choice.captures(line) {
                let num: usize = cap[1].parse().unwrap_or(0);
                if num == choices.len() + 1 {
                    let label = cap[2].trim_end_matches("(recommended)").trim().to_string();
                    if first_choice_line.is_none() {
                        first_choice_line = Some(idx);
                    }
                    choices.push(label);
                }
            }
        }

        if choices.is_empty() {
            return None;
        }

        // The question text is the closest non-empty, non-border line above the
        // first choice.
        let lines: Vec<&str> = tail.lines().collect();
        let question = first_choice_line
            .and_then(|first| {
                lines[..first].iter().rev().find_map(|line| {
                    let t = line
                        .trim()
                        .trim_start_matches(|c: char| {
                            matches!(c, '╭' | '│' | '╰' | '─' | '├' | '┤' | '┌' | '└')
                        })
                        .trim();
                    if t.is_empty() || t == "Question" || t.ends_with("> Question") {
                        None
                    } else {
                        Some(t.to_string())
                    }
                })
            })
            .unwrap_or_default();

        Some((
            ApprovalType::UserQuestion {
                choices,
                // Sub-option questions render a checkbox list plus a
                // "Submit answer" row.
                multi_select: tail.contains("Submit answer"),
            },
            question,
        ))
    }

    /// Detects a pending tool approval and its type.
    fn detect_approval(&self, tail: &str) -> Option<(ApprovalType, String)> {
        // Signal 1: the snackbar that replaces the status bar. It puts the title
        // on the left and "esc to cancel" flush right on the *same* line, so
        // demanding both anchors the match to a live prompt.
        let banner = tail.lines().rev().find(|line| {
            let t = line.trim();
            t.contains("requires approval") && t.ends_with("esc to cancel")
        });

        // Signal 2: the dropdown itself, in case a narrow pane truncated the
        // snackbar title away.
        let dropdown_open = approval_option_rows(tail) >= 2;

        if banner.is_none() && !dropdown_open {
            return None;
        }

        // Fall back to the dropdown's own title line for the tool name.
        let title = banner.or_else(|| {
            tail.lines()
                .rev()
                .find(|line| line.contains("requires approval"))
        });

        let (tool, detail) = title
            .and_then(|line| self.approval_banner.captures(line))
            .map(|cap| {
                (
                    cap.get(1)
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_default(),
                    cap.get(2)
                        .map(|m| m.as_str().trim().to_string())
                        .unwrap_or_default(),
                )
            })
            .unwrap_or_default();

        let details = if detail.is_empty() {
            tool.clone()
        } else {
            detail
        };

        Some((classify_tool(&tool), details))
    }

    /// Short label for the currently running tool, if one is on screen.
    fn running_activity(&self, tail: &str) -> Option<String> {
        let raw = self
            .running_tool
            .captures_iter(tail)
            .last()
            .and_then(|cap| cap.get(1))
            .map(|m| m.as_str().trim().to_string())?;

        if raw.is_empty() {
            return None;
        }

        let truncated: String = raw.chars().take(40).collect();
        Some(truncated)
    }
}

impl Default for KiroCliParser {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentParser for KiroCliParser {
    fn agent_name(&self) -> &str {
        "Kiro CLI"
    }

    fn agent_type(&self) -> AgentType {
        AgentType::KiroCli
    }

    fn matches(&self, detection_strings: &[&str]) -> bool {
        detection_strings
            .iter()
            .any(|s| s.split_whitespace().any(is_kiro_binary))
    }

    fn parse_status(&self, content: &str) -> AgentStatus {
        if content.trim().is_empty() {
            return AgentStatus::Unknown;
        }

        // Kiro CLI never writes the pane title, so everything is derived from
        // the captured content. Prompts live in the bottom rows of the pane.
        let tail = tail_lines(content, 40);

        if let Some((approval_type, details)) = self.detect_question(&tail) {
            return AgentStatus::AwaitingApproval {
                approval_type,
                details,
            };
        }

        if let Some((approval_type, details)) = self.detect_approval(&tail) {
            return AgentStatus::AwaitingApproval {
                approval_type,
                details,
            };
        }

        // The input hint line is authoritative for busy vs. ready. Scan upward
        // and take the first hint we recognise.
        for line in tail.lines().rev() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if PROCESSING_HINTS.iter().any(|h| t.starts_with(h)) {
                return AgentStatus::Processing {
                    activity: self
                        .running_activity(&tail)
                        .unwrap_or_else(|| "Working...".to_string()),
                };
            }
            if IDLE_HINTS.iter().any(|h| t.starts_with(h)) {
                return AgentStatus::Idle;
            }
        }

        AgentStatus::Idle
    }

    fn parse_subagents(&self, content: &str) -> Vec<Subagent> {
        let mut subagents = Vec::new();

        for cap in self.subagent_line.captures_iter(content) {
            let suffix = cap.get(1).map(|m| m.as_str()).unwrap_or("").trim();
            let status = if suffix.contains("FAILED")
                || suffix.contains("DENIED")
                || suffix.contains("cancelled")
            {
                SubagentStatus::Failed
            } else if suffix == "done" || suffix.ends_with('s') || suffix.ends_with('m') {
                SubagentStatus::Completed
            } else {
                SubagentStatus::Running
            };

            // Pipeline stages are listed below the subagent line as `[stage]`.
            let rest = &content[cap.get(0).map(|m| m.end()).unwrap_or(0)..];
            let stage_block: String = rest
                .lines()
                .skip_while(|l| l.trim().is_empty())
                .take_while(|l| {
                    let t = l.trim();
                    t.starts_with("pipeline:") || self.pipeline_stage.is_match(l) || t.is_empty()
                })
                .collect::<Vec<_>>()
                .join("\n");

            let stages: Vec<String> = self
                .pipeline_stage
                .captures_iter(&stage_block)
                .map(|c| c[1].to_string())
                .collect();

            if stages.is_empty() {
                subagents.push(
                    Subagent::new(
                        format!("subagent-{}", subagents.len() + 1),
                        SubagentType::parse("subagent"),
                        String::new(),
                    )
                    .with_status(status),
                );
            } else {
                for stage in stages {
                    subagents.push(
                        Subagent::new(
                            format!("subagent-{}", subagents.len() + 1),
                            SubagentType::parse(&stage),
                            String::new(),
                        )
                        .with_status(status.clone()),
                    );
                }
            }
        }

        subagents
    }

    fn parse_context_remaining(&self, content: &str) -> Option<u8> {
        // Kiro's status bar reports context *used* (`◔ 6%`), while tmuxcc
        // tracks context *remaining*.
        let tail = tail_lines(content, 20);
        let used = self
            .context_used
            .captures(&tail)
            .or_else(|| self.context_used_verbose.captures(&tail))
            .and_then(|cap| cap.get(1))
            .and_then(|m| m.as_str().parse::<u16>().ok())?;

        Some(100u16.saturating_sub(used.min(100)) as u8)
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

    /// Status bar + input hint as captured from a real idle Kiro CLI pane.
    const IDLE_PANE: &str = "  \u{25CF} Read /Users/timer/project/src/main.rs\n\n\u{25B8} Credits: 2.78 \u{2022} Time: 3m 13s\n\n\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n Trust All Tools active, confirmations are off \u{B7} /quit to exit\n\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\nDefault \u{B7} Claude Opus 5 \u{B7} High \u{B7} \u{25D4} 6%                    ~/project \u{B7} (main)\n\n ask a question or describe a task \u{21B5}\n                                                        /copy to clipboard\n";

    /// Same pane while a tool call is in flight.
    const WORKING_PANE: &str = "  \u{25CF} Read /Users/timer/project/src/main.rs\n  \u{25D1} Shell cargo test --all\n\n\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\nDefault \u{B7} Claude Opus 5 \u{B7} High \u{B7} \u{25D4} 8%                    ~/project \u{B7} (main)\n\n Kiro is working \u{B7} Type to steer \u{B7} Ctrl+S to queue\n";

    #[test]
    fn test_matches_kiro_process() {
        let parser = KiroCliParser::new();
        // Child process of the pane shell
        assert!(parser.matches(&[
            "fish",
            "kr ~/D/C/t/tmuxcc",
            "fish (kiro-cli-term)",
            "kiro-cli chat --trust-all-tools --v3",
            "kiro-cli",
        ]));
        // Wrapper binary with an absolute path
        assert!(parser.matches(&[
            "fish",
            "",
            "fish (kiro-cli-term)",
            "/Users/timer/.local/bin/kiro-cli-chat chat --v3",
        ]));
        // Run directly as the pane command
        assert!(parser.matches(&["kiro-cli", "", "kiro-cli chat"]));
    }

    #[test]
    fn test_shell_integration_marker_is_not_an_agent() {
        let parser = KiroCliParser::new();
        // Every pane opened from a Kiro terminal carries this marker, even
        // plain shells — it must not be detected as an agent.
        assert!(!parser.matches(&["fish", "~/D/C/t/lupa", "fish (kiro-cli-term)"]));
        assert!(!parser.matches(&[
            "fish",
            "~",
            "fish (kiro-cli-term)",
            "/opt/homebrew/Cellar/fish/4.8.1/bin/fish --login",
        ]));
        // ...and neither must a Claude Code pane that happens to sit under one.
        assert!(!parser.matches(&[
            "fish",
            "\u{2733} some task",
            "fish (kiro-cli-term)",
            "claude --dangerously-skip-permissions",
        ]));
    }

    #[test]
    fn test_no_match_for_other_agents() {
        let parser = KiroCliParser::new();
        assert!(!parser.matches(&["claude", "Claude Code", "claude -c"]));
        assert!(!parser.matches(&["grok-1.0.3-maco", "... - grok", "grok"]));
        assert!(!parser.matches(&["opencode", "OpenCode", "opencode"]));
        // A file that merely looks like the binary name
        assert!(!parser.matches(&["nvim", "", "nvim kiro-cli.md"]));
    }

    #[test]
    fn test_parse_idle() {
        let parser = KiroCliParser::new();
        assert!(
            matches!(parser.parse_status(IDLE_PANE), AgentStatus::Idle),
            "expected Idle, got {:?}",
            parser.parse_status(IDLE_PANE)
        );
    }

    #[test]
    fn test_parse_processing() {
        let parser = KiroCliParser::new();
        match parser.parse_status(WORKING_PANE) {
            AgentStatus::Processing { activity } => {
                assert_eq!(activity, "Shell cargo test --all");
            }
            other => panic!("expected Processing, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_approval_shell() {
        let parser = KiroCliParser::new();
        let content = format!(
            "{}\n Shell \u{B7} rm -rf build requires approval                    esc to cancel\n",
            WORKING_PANE
        );
        match parser.parse_status(&content) {
            AgentStatus::AwaitingApproval {
                approval_type,
                details,
            } => {
                assert_eq!(approval_type, ApprovalType::ShellCommand);
                assert_eq!(details, "rm -rf build");
            }
            other => panic!("expected AwaitingApproval, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_approval_via_dropdown_options() {
        let parser = KiroCliParser::new();
        let content = " Write \u{B7} src/main.rs requires approval\n\n\u{276F} Yes, single permission\n  Trust, always allow in this session\n  No (Tab to edit)\n\n Kiro is working \u{B7} Type to steer \u{B7} Ctrl+S to queue\n";
        match parser.parse_status(content) {
            AgentStatus::AwaitingApproval { approval_type, .. } => {
                assert_eq!(approval_type, ApprovalType::FileEdit);
            }
            other => panic!("expected AwaitingApproval, got {:?}", other),
        }
    }

    #[test]
    fn test_transcript_prose_is_not_an_approval() {
        let parser = KiroCliParser::new();
        // The agent talking *about* approvals must not flip the state.
        let content = "  \u{25CF} Read src/lib.rs\n  The dropdown appears when a tool requires approval, and esc to cancel dismisses it.\n\n ask a question or describe a task \u{21B5}\n";
        assert!(
            matches!(parser.parse_status(content), AgentStatus::Idle),
            "expected Idle, got {:?}",
            parser.parse_status(content)
        );
    }

    /// Regression: an earlier version keyed off "any option label appears
    /// anywhere in the tail", which reported this very pane as awaiting
    /// approval while it was documenting the parser.
    #[test]
    fn test_documenting_the_parser_is_not_an_approval() {
        let parser = KiroCliParser::new();
        let content = "- **Approval**: the `<Tool> \u{B7} <detail> requires approval` snackbar, or the\n  approval dropdown options (`Yes, single permission` / `No (Tab to edit)`)\n- **Question**: the question panel, recognised by its `Type a different answer\u{2026}`\n  option; the choices are listed in the sidebar\n\n\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\nDefault \u{B7} Claude Opus 5 \u{B7} High \u{B7} \u{25D4} 19%                    ~/project \u{B7} (master)\n\n ask a question or describe a task \u{21B5}\n                                                        /copy to clipboard\n";
        assert!(
            matches!(parser.parse_status(content), AgentStatus::Idle),
            "expected Idle, got {:?}",
            parser.parse_status(content)
        );
    }

    /// A single option label on its own line is still not enough: the dropdown
    /// always renders both an accept and a reject row.
    #[test]
    fn test_single_option_row_is_not_an_approval() {
        let parser = KiroCliParser::new();
        let content = "  Yes, single permission\n\n ask a question or describe a task \u{21B5}\n";
        assert!(
            matches!(parser.parse_status(content), AgentStatus::Idle),
            "expected Idle, got {:?}",
            parser.parse_status(content)
        );
    }

    /// ...and a mid-line mention of the question marker is not a question.
    #[test]
    fn test_question_marker_mid_line_is_not_a_question() {
        let parser = KiroCliParser::new();
        let content = "  The panel is recognised by its `Type a different answer\u{2026}` row.\n  1. first\n  2. second\n\n ask a question or describe a task \u{21B5}\n";
        assert!(
            matches!(parser.parse_status(content), AgentStatus::Idle),
            "expected Idle, got {:?}",
            parser.parse_status(content)
        );
    }

    #[test]
    fn test_parse_question() {
        let parser = KiroCliParser::new();
        let content = "\u{256D}\u{2500} Question \u{2500}\u{256E}\n\u{2502} Which database should we use?\n\n\u{276F} 1. Postgres (recommended)\n  2. SQLite\n  3. MySQL\n  Type a different answer\u{2026}\n\n Kiro is working \u{B7} Type to steer \u{B7} Ctrl+S to queue\n";
        match parser.parse_status(content) {
            AgentStatus::AwaitingApproval {
                approval_type,
                details,
            } => match approval_type {
                ApprovalType::UserQuestion { choices, .. } => {
                    assert_eq!(choices, vec!["Postgres", "SQLite", "MySQL"]);
                    assert_eq!(details, "Which database should we use?");
                }
                other => panic!("expected UserQuestion, got {:?}", other),
            },
            other => panic!("expected AwaitingApproval, got {:?}", other),
        }
    }

    #[test]
    fn test_context_remaining_is_inverted() {
        let parser = KiroCliParser::new();
        // Status bar shows 6% *used* -> 94% remaining.
        assert_eq!(parser.parse_context_remaining(IDLE_PANE), Some(94));
        assert_eq!(parser.parse_context_remaining(WORKING_PANE), Some(92));
        assert_eq!(
            parser.parse_context_remaining("  42% context used\n"),
            Some(58)
        );
        assert_eq!(parser.parse_context_remaining("no status bar here"), None);
    }

    #[test]
    fn test_parse_subagents() {
        let parser = KiroCliParser::new();
        let content = "subagent \u{25D1}\n  pipeline:\n    \u{251C}\u{2500} [research]\n    \u{2570}\u{2500} [implement]\n";
        let subagents = parser.parse_subagents(content);
        assert_eq!(subagents.len(), 2);
        assert_eq!(subagents[0].subagent_type.display_name(), "research");
        assert_eq!(subagents[0].status, SubagentStatus::Running);

        let done = parser.parse_subagents("subagent done\n");
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].status, SubagentStatus::Completed);

        let failed = parser.parse_subagents("subagent FAILED\n");
        assert_eq!(failed[0].status, SubagentStatus::Failed);
    }
}
