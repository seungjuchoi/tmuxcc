mod claude_code;
mod codex_cli;
mod gemini_cli;
mod grok;
mod kiro_cli;
mod opencode;

pub use claude_code::ClaudeCodeParser;
pub use codex_cli::CodexCliParser;
pub use gemini_cli::GeminiCliParser;
pub use grok::GrokParser;
pub use kiro_cli::KiroCliParser;
pub use opencode::OpenCodeParser;

use crate::agents::{AgentStatus, AgentType, Subagent};
use crate::tmux::PaneInfo;

/// Safely get the last N characters of a string (handles multi-byte chars)
pub(crate) fn safe_tail(s: &str, max_chars: usize) -> &str {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s;
    }
    let skip = char_count - max_chars;
    let byte_idx = s.char_indices().nth(skip).map(|(idx, _)| idx).unwrap_or(0);
    &s[byte_idx..]
}

/// The executable a detection string runs: the basename of its argv[0].
///
/// Arguments are deliberately dropped. An agent's shell tool call arrives here
/// as a full command line, and matching on the whole string made `ls ~/.claude`
/// look like Claude Code — the executable is `ls`.
pub(crate) fn executable_name(detection_string: &str) -> &str {
    let argv0 = detection_string.split_whitespace().next().unwrap_or("");
    argv0.rsplit('/').next().unwrap_or(argv0)
}

/// Trait for parsing agent output
pub trait AgentParser: Send + Sync {
    /// Returns the name of the agent
    fn agent_name(&self) -> &str;

    /// Returns the AgentType for this parser
    fn agent_type(&self) -> AgentType;

    /// Checks if any of the process-derived detection strings (pane command,
    /// cmdline, child commands) match this agent.
    ///
    /// The pane title is **not** included — see [`PaneInfo::process_strings`].
    ///
    /// Match on the *executable* ([`executable_name`]), not on arguments: the
    /// registry feeds this the full command line of every descendant process,
    /// tool calls included.
    fn matches(&self, detection_strings: &[&str]) -> bool;

    /// Fallback: recognises the agent from its pane-title branding alone.
    ///
    /// Only implement this where the title carries a marker the agent itself
    /// writes and no other tool would (Claude Code's `✳` glyph, Grok's
    /// ` - grok` suffix). It is consulted only when no parser matched on
    /// process evidence, so a real process always wins over a title that
    /// merely *mentions* another agent.
    fn matches_title(&self, title: &str) -> bool {
        let _ = title;
        false
    }

    /// Parses the pane content and returns the agent status
    fn parse_status(&self, content: &str) -> AgentStatus;

    /// Parses subagents from the content (default: empty)
    fn parse_subagents(&self, content: &str) -> Vec<Subagent> {
        let _ = content;
        Vec::new()
    }

    /// Parses the percentage of the context window *used* (default: None)
    fn parse_context_used(&self, content: &str) -> Option<u8> {
        let _ = content;
        None
    }

    /// Returns the key(s) to send for approval
    fn approval_keys(&self) -> &str {
        "y"
    }

    /// Returns the key(s) to send for rejection
    fn rejection_keys(&self) -> &str {
        "n"
    }
}

/// Registry of all available parsers
pub struct ParserRegistry {
    parsers: Vec<Box<dyn AgentParser>>,
}

impl ParserRegistry {
    /// Creates a new registry with all default parsers
    pub fn new() -> Self {
        Self {
            parsers: vec![
                // Kiro CLI first: it matches on exact executable names, so it
                // is the least likely to claim a pane that belongs to another
                // agent (Claude Code, for instance, also matches bare version
                // numbers).
                Box::new(KiroCliParser::new()),
                Box::new(ClaudeCodeParser::new()),
                Box::new(GrokParser::new()),
                Box::new(OpenCodeParser::new()),
                Box::new(CodexCliParser::new()),
                Box::new(GeminiCliParser::new()),
            ],
        }
    }

    /// Finds a parser that matches the given pane info.
    ///
    /// Process evidence is resolved first, across *all* parsers, before any
    /// title branding is considered. Pane titles hold a task summary now, so a
    /// title naming another agent must never outrank the process actually
    /// running in the pane.
    pub fn find_parser_for_pane(&self, pane: &PaneInfo) -> Option<&dyn AgentParser> {
        // Nearest-to-the-pane wins. `process_strings` is ordered by depth, and
        // an agent's tool calls always sit below the agent itself, so the first
        // command any parser recognises is the agent that owns the pane —
        // never a `claude`/`grok` the agent merely invoked.
        for candidate in pane.process_strings() {
            if let Some(parser) = self.parsers.iter().find(|p| p.matches(&[candidate])) {
                return Some(parser.as_ref());
            }
        }

        self.parsers
            .iter()
            .find(|p| p.matches_title(&pane.title))
            .map(|p| p.as_ref())
    }

    /// Returns all registered parsers
    pub fn all_parsers(&self) -> impl Iterator<Item = &dyn AgentParser> {
        self.parsers.iter().map(|p| p.as_ref())
    }
}

impl Default for ParserRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parser_registry() {
        let registry = ParserRegistry::new();

        // Test finding parsers with various detection strings
        let claude_pane = PaneInfo {
            session: "main".to_string(),
            window: 0,
            window_name: "code".to_string(),
            pane: 0,
            command: "node".to_string(),
            title: "Claude Code".to_string(),
            path: "/home/user/project".to_string(),
            pid: 1234,
            cmdline: "/usr/bin/claude".to_string(),
            child_commands: Vec::new(),
        };
        assert!(registry.find_parser_for_pane(&claude_pane).is_some());

        let opencode_pane = PaneInfo {
            session: "main".to_string(),
            window: 0,
            window_name: "code".to_string(),
            pane: 1,
            command: "opencode".to_string(),
            title: "".to_string(),
            path: "/home/user/project".to_string(),
            pid: 1235,
            cmdline: "opencode".to_string(),
            child_commands: Vec::new(),
        };
        assert!(registry.find_parser_for_pane(&opencode_pane).is_some());

        // Test detection via child processes
        let child_claude_pane = PaneInfo {
            session: "main".to_string(),
            window: 0,
            window_name: "code".to_string(),
            pane: 2,
            command: "zsh".to_string(),
            title: "~".to_string(),
            path: "/home/user/project".to_string(),
            pid: 1236,
            cmdline: "-zsh".to_string(),
            child_commands: vec!["claude -c".to_string(), "claude".to_string()],
        };
        assert!(registry.find_parser_for_pane(&child_claude_pane).is_some());

        // Grok via truncated pane command
        let grok_pane = PaneInfo {
            session: "main".to_string(),
            window: 3,
            window_name: "grok".to_string(),
            pane: 0,
            command: "grok-1.0.3-maco".to_string(),
            title: "⠹ - Running: Read foo - some task - grok".to_string(),
            path: "/home/user/project".to_string(),
            pid: 1237,
            cmdline: "grok --always-approve".to_string(),
            child_commands: Vec::new(),
        };
        let grok_parser = registry
            .find_parser_for_pane(&grok_pane)
            .expect("should detect Grok");
        assert_eq!(grok_parser.agent_type(), crate::agents::AgentType::Grok);
    }

    /// Regression (observed live on `main:2.3`): the pane title now holds a
    /// task summary, and that summary named Claude Code — so the ClaudeCode
    /// parser, which comes first in the registry, claimed a Grok pane.
    /// Process evidence must win over anything the title says.
    #[test]
    fn test_task_summary_naming_another_agent_loses_to_the_process() {
        let registry = ParserRegistry::new();

        let grok_pane_titled_claude = PaneInfo {
            session: "main".to_string(),
            window: 2,
            window_name: "proxy".to_string(),
            pane: 3,
            command: "fish".to_string(),
            title: "Claude Code Proxy Open Source GitHub Sea… - grok".to_string(),
            path: "/Users/timer/Documents/Code/tz/tmuxcc".to_string(),
            pid: 64479,
            cmdline: "fish".to_string(),
            child_commands: vec!["grok --always-approve".to_string(), "grok".to_string()],
        };
        let parser = registry
            .find_parser_for_pane(&grok_pane_titled_claude)
            .expect("should detect Grok");
        assert_eq!(parser.agent_type(), crate::agents::AgentType::Grok);

        // The mirror case: a Kiro CLI session whose task is "investigate the
        // grok parser" stays Kiro CLI.
        let kiro_pane_titled_grok = PaneInfo {
            session: "main".to_string(),
            window: 1,
            window_name: "tmuxcc".to_string(),
            pane: 1,
            command: "fish".to_string(),
            title: "grok 파서 오탐 조사 - claude vs grok".to_string(),
            path: "/Users/timer/Documents/Code/tz/tmuxcc".to_string(),
            pid: 21915,
            cmdline: "fish (kiro-cli-term)".to_string(),
            child_commands: vec![
                "fish".to_string(),
                "kiro-cli chat --trust-all-tools --v3".to_string(),
                "kiro-cli".to_string(),
            ],
        };
        let parser = registry
            .find_parser_for_pane(&kiro_pane_titled_grok)
            .expect("should detect Kiro CLI");
        assert_eq!(parser.agent_type(), crate::agents::AgentType::KiroCli);

        // A plain shell whose title merely names an agent is still not an agent.
        let shell_titled_claude = PaneInfo {
            session: "lupa".to_string(),
            window: 1,
            window_name: "lupa".to_string(),
            pane: 2,
            command: "fish".to_string(),
            title: "claude code 설치 메모".to_string(),
            path: "/Users/timer/Documents/Code/tz/lupa".to_string(),
            pid: 90915,
            cmdline: "fish (kiro-cli-term)".to_string(),
            child_commands: vec!["fish".to_string()],
        };
        assert!(registry
            .find_parser_for_pane(&shell_titled_claude)
            .is_none());
    }

    /// Regression (reproduced live on a `grok --always-approve` pane): the
    /// moment Grok ran a shell tool whose command line mentioned `~/.claude`,
    /// the pane flipped to Claude Code. An agent's own tool calls must never
    /// out-rank the agent, so `process_strings` is ordered by depth and the
    /// nearest recognised command wins.
    #[test]
    fn test_a_tool_call_does_not_steal_the_pane_from_its_agent() {
        let registry = ParserRegistry::new();

        let grok_running_a_shell_tool = PaneInfo {
            session: "Dog-detection".to_string(),
            window: 1,
            window_name: "grok".to_string(),
            pane: 1,
            command: "grok-1.0.5-maco".to_string(),
            title: "\u{27e8}task\u{27e9} - grok".to_string(),
            path: "/Users/timer/Documents/Code/tz".to_string(),
            pid: 6316,
            cmdline: "-fish".to_string(),
            // Depth order: grok first, then what grok spawned.
            child_commands: vec![
                "grok --always-approve".to_string(),
                "grok".to_string(),
                "/bin/bash -O extglob -c ls ~/.claude/ | head -3".to_string(),
                "bash".to_string(),
            ],
        };
        let parser = registry
            .find_parser_for_pane(&grok_running_a_shell_tool)
            .expect("should detect Grok");
        assert_eq!(parser.agent_type(), crate::agents::AgentType::Grok);

        // And the mirror: Claude Code shelling out to `grok -p` stays Claude.
        let claude_shelling_out_to_grok = PaneInfo {
            session: "main".to_string(),
            window: 3,
            window_name: "tz".to_string(),
            pane: 1,
            command: "2.1.238".to_string(),
            title: "\u{2733} compare the agents".to_string(),
            path: "/Users/timer/Documents/Code/tz".to_string(),
            pid: 7788,
            cmdline: "-fish".to_string(),
            child_commands: vec![
                "claude --dangerously-skip-permissions".to_string(),
                "claude".to_string(),
                "grok -p \"summarise this\"".to_string(),
                "grok".to_string(),
            ],
        };
        let parser = registry
            .find_parser_for_pane(&claude_shelling_out_to_grok)
            .expect("should detect Claude Code");
        assert_eq!(parser.agent_type(), crate::agents::AgentType::ClaudeCode);

        // A plain shell where the user simply listed the config directory is
        // still not an agent.
        let shell_listing_the_config_dir = PaneInfo {
            session: "lupa".to_string(),
            window: 3,
            window_name: "lupa".to_string(),
            pane: 1,
            command: "ls".to_string(),
            title: "~/D/C/t/lupa_ed_mac".to_string(),
            path: "/Users/timer/Documents/Code/tz/lupa_ed_mac".to_string(),
            pid: 9911,
            cmdline: "-fish".to_string(),
            child_commands: vec!["ls -la /Users/timer/.claude".to_string(), "ls".to_string()],
        };
        assert!(registry
            .find_parser_for_pane(&shell_listing_the_config_dir)
            .is_none());
    }

    /// The `✳` glyph is still enough on its own, for panes whose process tree
    /// is invisible to the local `ps` (ssh/mosh hops).
    #[test]
    fn test_claude_icon_title_is_a_fallback() {
        let registry = ParserRegistry::new();

        let remote_claude = PaneInfo {
            session: "main".to_string(),
            window: 6,
            window_name: "remote".to_string(),
            pane: 0,
            command: "ssh".to_string(),
            title: "✳ Refactor the ingest pipeline".to_string(),
            path: "/Users/timer".to_string(),
            pid: 4242,
            cmdline: "ssh build-box".to_string(),
            child_commands: Vec::new(),
        };
        let parser = registry
            .find_parser_for_pane(&remote_claude)
            .expect("should fall back to Claude Code");
        assert_eq!(parser.agent_type(), crate::agents::AgentType::ClaudeCode);
    }

    #[test]
    fn test_kiro_detection_and_shell_marker() {
        let registry = ParserRegistry::new();

        // Real Kiro CLI pane: the shell is `fish (kiro-cli-term)` and the agent
        // sits a few levels below it.
        let kiro_pane = PaneInfo {
            session: "main".to_string(),
            window: 2,
            window_name: "tmuxcc".to_string(),
            pane: 1,
            command: "fish".to_string(),
            title: "kr ~/D/C/t/tmuxcc".to_string(),
            path: "/Users/timer/Documents/Code/tz/tmuxcc".to_string(),
            pid: 21915,
            cmdline: "fish (kiro-cli-term)".to_string(),
            child_commands: vec![
                "/opt/homebrew/Cellar/fish/4.8.1/bin/fish --login".to_string(),
                "fish".to_string(),
                "kiro-cli chat --trust-all-tools --v3".to_string(),
                "kiro-cli".to_string(),
                "/Users/timer/.local/bin/kiro-cli-chat chat --trust-all-tools --v3".to_string(),
                "kiro-cli-chat".to_string(),
            ],
        };
        let kiro_parser = registry
            .find_parser_for_pane(&kiro_pane)
            .expect("should detect Kiro CLI");
        assert_eq!(kiro_parser.agent_type(), crate::agents::AgentType::KiroCli);

        // A plain shell opened from a Kiro terminal carries the same marker but
        // runs no agent.
        let bare_shell = PaneInfo {
            session: "lupa".to_string(),
            window: 1,
            window_name: "lupa".to_string(),
            pane: 2,
            command: "fish".to_string(),
            title: "~/D/C/t/l/lupa".to_string(),
            path: "/Users/timer/Documents/Code/tz/lupa".to_string(),
            pid: 90915,
            cmdline: "fish (kiro-cli-term)".to_string(),
            child_commands: vec![
                "/opt/homebrew/Cellar/fish/4.8.1/bin/fish --login".to_string(),
                "fish".to_string(),
            ],
        };
        assert!(
            registry.find_parser_for_pane(&bare_shell).is_none(),
            "the kiro-cli-term shell marker must not register as an agent"
        );

        // Claude Code running under a Kiro-integrated shell still resolves to
        // Claude Code.
        let claude_under_kiro = PaneInfo {
            session: "resp".to_string(),
            window: 1,
            window_name: "libalgo".to_string(),
            pane: 1,
            command: "fish".to_string(),
            title: "\u{2733} Radar".to_string(),
            path: "/Users/timer/Documents/Code/tz/libalgo".to_string(),
            pid: 16576,
            cmdline: "fish -c reattach-to-user-namespace".to_string(),
            child_commands: vec![
                "fish (kiro-cli-term)".to_string(),
                "claude --dangerously-skip-permissions".to_string(),
                "claude".to_string(),
            ],
        };
        let claude_parser = registry
            .find_parser_for_pane(&claude_under_kiro)
            .expect("should detect Claude Code");
        assert_eq!(
            claude_parser.agent_type(),
            crate::agents::AgentType::ClaudeCode
        );
    }
}
