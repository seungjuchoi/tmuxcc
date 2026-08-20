use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Process info stored in cache
#[derive(Clone, Debug)]
struct ProcessInfo {
    command: String,
    parent_pid: Option<u32>,
}

/// Cached process tree for efficient child process lookup
struct ProcessTreeCache {
    /// Map of PID -> ProcessInfo
    processes: HashMap<u32, ProcessInfo>,
    /// When the cache was last updated
    last_update: Instant,
}

impl ProcessTreeCache {
    fn new() -> Self {
        Self {
            processes: HashMap::new(),
            last_update: Instant::now() - Duration::from_secs(100), // Force initial refresh
        }
    }

    fn needs_refresh(&self) -> bool {
        self.last_update.elapsed() > Duration::from_millis(500)
    }

    fn refresh(&mut self) {
        // Get all processes in one call: PID, PPID, COMMAND
        let output = Command::new("ps")
            .args(["-A", "-o", "pid=,ppid=,command="])
            .output();

        let output = match output {
            Ok(o) if o.status.success() => o,
            _ => return,
        };

        self.processes.clear();
        let stdout = String::from_utf8_lossy(&output.stdout);

        for line in stdout.lines() {
            if let Some((pid, ppid, command)) = parse_ps_line(line) {
                let parent = if ppid == 0 { None } else { Some(ppid) };
                self.processes.insert(
                    pid,
                    ProcessInfo {
                        command,
                        parent_pid: parent,
                    },
                );
            }
        }

        self.last_update = Instant::now();
    }

    /// Descendant commands of `pid`, **breadth-first**.
    ///
    /// The ordering is load-bearing: an agent's own tool calls are always
    /// deeper in the tree than the agent process itself, and
    /// `ParserRegistry::find_parser_for_pane` takes the first command it
    /// recognises. Depth-first ordering let a shell command spawned by Grok
    /// (`ls ~/.claude`) be examined before `grok` itself and hand the pane to
    /// the Claude Code parser.
    fn get_child_commands(&self, pid: u32, max_depth: u32) -> Vec<String> {
        let mut commands = Vec::new();
        let mut frontier: HashSet<u32> = HashSet::from([pid]);

        for _ in 0..max_depth {
            // Sorted by pid so the order does not vary with HashMap iteration.
            let mut level: Vec<(u32, &ProcessInfo)> = self
                .processes
                .iter()
                .filter(|(_, info)| info.parent_pid.is_some_and(|p| frontier.contains(&p)))
                .map(|(&child_pid, info)| (child_pid, info))
                .collect();
            level.sort_unstable_by_key(|(child_pid, _)| *child_pid);

            if level.is_empty() {
                break;
            }

            let mut next = HashSet::with_capacity(level.len());
            for (child_pid, info) in level {
                commands.push(info.command.clone());
                // Add base name
                if let Some(first) = info.command.split_whitespace().next() {
                    if let Some(base) = first.rsplit('/').next() {
                        if base != info.command {
                            commands.push(base.to_string());
                        }
                    }
                }
                next.insert(child_pid);
            }
            frontier = next;
        }

        commands
    }

    fn get_cmdline(&self, pid: u32) -> Option<String> {
        self.processes.get(&pid).map(|info| info.command.clone())
    }
}

/// Parses one `ps -A -o pid=,ppid=,command=` line into (pid, ppid, command).
///
/// `ps` right-aligns the numeric columns, so the gap between them is one *or
/// more* spaces. Splitting on a whitespace predicate does not collapse those
/// runs — it yielded an empty ppid field and the whole process was dropped,
/// which silently emptied the child list for any pane whose ppid was shorter
/// than the widest pid on the system.
fn parse_ps_line(line: &str) -> Option<(u32, u32, String)> {
    let (pid, rest) = line.trim_start().split_once(char::is_whitespace)?;
    let (ppid, command) = rest.trim_start().split_once(char::is_whitespace)?;
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    Some((pid.parse().ok()?, ppid.parse().ok()?, command.to_string()))
}

static PROCESS_CACHE: OnceLock<Mutex<ProcessTreeCache>> = OnceLock::new();

fn get_process_cache() -> &'static Mutex<ProcessTreeCache> {
    PROCESS_CACHE.get_or_init(|| Mutex::new(ProcessTreeCache::new()))
}

/// Refresh the process cache if needed (call once per poll cycle)
pub fn refresh_process_cache() {
    let mut cache = get_process_cache().lock();
    if cache.needs_refresh() {
        cache.refresh();
    }
}

/// Represents a tmux pane with its identifying information
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneInfo {
    /// Session name
    pub session: String,
    /// Window index
    pub window: u32,
    /// Window name
    pub window_name: String,
    /// Pane index
    pub pane: u32,
    /// Current command running in the pane
    pub command: String,
    /// Pane title (often contains useful info like "Claude Code")
    pub title: String,
    /// Current working directory of the pane
    pub path: String,
    /// Process ID of the pane
    pub pid: u32,
    /// Full command line of the process
    pub cmdline: String,
    /// Child process commands (for detecting running agents)
    pub child_commands: Vec<String>,
}

impl PaneInfo {
    /// Returns the tmux target string (e.g., "session:0.1")
    pub fn target(&self) -> String {
        format!("{}:{}.{}", self.session, self.window, self.pane)
    }

    /// Parses a pane info from tmux list-panes output
    /// Expected format: "session:window.pane\twindow_name\tcommand\tpid\ttitle\tpath"
    pub fn parse(line: &str) -> Option<Self> {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 6 {
            return None;
        }

        let target = parts[0];
        let window_name = parts[1].to_string();
        let command = parts[2].to_string();
        let pid: u32 = parts[3].parse().ok()?;
        let title = parts[4].to_string();
        let path = parts[5].to_string();

        // Parse target "session:window.pane"
        let (session, rest) = target.split_once(':')?;
        let (window_str, pane_str) = rest.split_once('.')?;

        let window: u32 = window_str.parse().ok()?;
        let pane: u32 = pane_str.parse().ok()?;

        // Use cached process tree for fast lookups
        let cache = get_process_cache().lock();
        let cmdline = cache.get_cmdline(pid).unwrap_or_default();
        // Depth 6: agents can sit several levels below the pane shell
        // (e.g. fish -> shell-integration wrapper -> fish -> claude)
        let child_commands = cache.get_child_commands(pid, 6);

        Some(Self {
            session: session.to_string(),
            window,
            window_name,
            pane,
            command,
            title,
            path,
            pid,
            cmdline,
            child_commands,
        })
    }

    /// Returns the process-derived detection strings (pane command, cmdline and
    /// child commands) used for agent matching.
    ///
    /// The pane **title** is deliberately excluded: agents now write a
    /// human-readable task summary there, so a title may name any tool the
    /// session happens to be working on ("Claude Code proxy … - grok"). Only
    /// the process tree is trustworthy evidence of which agent is running;
    /// title-only signals go through [`AgentParser::matches_title`].
    pub fn process_strings(&self) -> Vec<&str> {
        let mut strings = vec![self.command.as_str(), self.cmdline.as_str()];

        // Add child command strings
        for cmd in &self.child_commands {
            strings.push(cmd.as_str());
        }

        strings
    }
}

impl fmt::Display for PaneInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.target())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ps_line_tolerates_column_padding() {
        // Real `ps` output: the pid column is right-aligned to the widest pid,
        // so a 5-digit pid with a 4-digit ppid leaves *two* spaces.
        assert_eq!(
            parse_ps_line("62879  6316 grok --always-approve"),
            Some((62879, 6316, "grok --always-approve".to_string()))
        );
        assert_eq!(
            parse_ps_line(" 6316 17890 -fish"),
            Some((6316, 17890, "-fish".to_string()))
        );
        // Arguments keep their own spacing.
        assert_eq!(
            parse_ps_line("  501     1 /usr/bin/foo  -a   -b"),
            Some((501, 1, "/usr/bin/foo  -a   -b".to_string()))
        );
        assert_eq!(parse_ps_line(""), None);
        assert_eq!(parse_ps_line("garbage line"), None);
        assert_eq!(parse_ps_line("123 456"), None);
    }

    #[test]
    fn test_target() {
        let pane = PaneInfo {
            session: "dev".to_string(),
            window: 2,
            window_name: "editor".to_string(),
            pane: 3,
            command: "bash".to_string(),
            title: "".to_string(),
            path: "/home/user".to_string(),
            pid: 99999,
            cmdline: "".to_string(),
            child_commands: Vec::new(),
        };
        assert_eq!(pane.target(), "dev:2.3");
    }

    #[test]
    fn test_parse_invalid() {
        assert!(PaneInfo::parse("invalid").is_none());
        assert!(PaneInfo::parse("").is_none());
    }

    #[test]
    fn test_process_strings_exclude_the_title() {
        let pane = PaneInfo {
            session: "main".to_string(),
            window: 0,
            window_name: "code".to_string(),
            pane: 0,
            command: "zsh".to_string(),
            title: "✳ investigate the grok parser".to_string(),
            path: "/home/user".to_string(),
            pid: 1234,
            cmdline: "-zsh".to_string(),
            child_commands: vec!["claude -c".to_string(), "claude".to_string()],
        };
        let strings = pane.process_strings();
        assert!(strings.contains(&"zsh"));
        assert!(strings.contains(&"-zsh"));
        assert!(strings.contains(&"claude -c"));
        assert!(strings.contains(&"claude"));
        // The title is a task summary, not evidence of which agent is running.
        assert!(!strings.contains(&"✳ investigate the grok parser"));
    }
}
