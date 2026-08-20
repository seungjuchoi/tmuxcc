use regex::Regex;

use crate::agents::{AgentStatus, AgentType, ApprovalType, Subagent};

use super::{safe_tail, AgentParser};

/// Parser for Codex CLI output
pub struct CodexCliParser {
    approval_pattern: Regex,
    processing_pattern: Regex,
    idle_pattern: Regex,
}

impl CodexCliParser {
    pub fn new() -> Self {
        Self {
            approval_pattern: Regex::new(r"(?i)\[y/n\]|\[yes/no\]|confirm|approve|run this")
                .unwrap(),
            processing_pattern: Regex::new(r"(?i)(thinking|running|executing|generating)").unwrap(),
            idle_pattern: Regex::new(r"(?i)(ready|waiting|>\s*$|\$\s*$)").unwrap(),
        }
    }
}

impl Default for CodexCliParser {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentParser for CodexCliParser {
    fn agent_name(&self) -> &str {
        "Codex CLI"
    }

    fn agent_type(&self) -> AgentType {
        AgentType::CodexCli
    }

    fn matches(&self, detection_strings: &[&str]) -> bool {
        detection_strings.iter().any(|s| {
            // Executable only: the surrounding shell environment mentions
            // agents it is not (a Claude Code tool call exports
            // CODEX_COMPANION_SESSION_ID, for instance).
            let exe = super::executable_name(s).to_lowercase();
            exe.starts_with("codex") || exe.contains("openai")
        })
    }

    fn parse_status(&self, content: &str) -> AgentStatus {
        let recent = safe_tail(content, 500);

        if self.approval_pattern.is_match(recent) {
            return AgentStatus::AwaitingApproval {
                approval_type: ApprovalType::Other("Pending".to_string()),
                details: String::new(),
            };
        }

        if self.processing_pattern.is_match(recent) {
            return AgentStatus::Processing {
                activity: "Processing...".to_string(),
            };
        }

        if self.idle_pattern.is_match(recent) {
            return AgentStatus::Idle;
        }

        AgentStatus::Unknown
    }

    fn parse_subagents(&self, _content: &str) -> Vec<Subagent> {
        // Codex CLI doesn't have subagents
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches() {
        let parser = CodexCliParser::new();
        assert!(parser.matches(&["codex", ""]));
        assert!(parser.matches(&["node", "/usr/bin/codex"]));
        assert!(!parser.matches(&["claude", "claude"]));
    }
}
