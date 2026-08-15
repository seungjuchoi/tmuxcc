# TmuxCC

**AI Agent Dashboard for tmux** - Monitor and manage multiple AI coding agents from a single terminal interface.

TmuxCC is a TUI (Terminal User Interface) application that provides centralized monitoring and control of AI coding assistants running in tmux panes. It supports Claude Code, Kiro CLI, Grok, OpenCode, Codex CLI, and Gemini CLI.

---

## Screenshot

<!-- TODO: Add actual screenshot -->
```
+------------------------------------------------------------------+
| TmuxCC | 7 agents | * 1 working | CPU 17% | MEM 11.9G/16.0G (75%) |
+------------------------------------------------------------------+
| v main                            | main:2.0 (Kiro CLI)          |
|   project1                        |                              |
| |   1 ! PyCut video autoedit      | Kiro wants to run:           |
| |     Claude | ctx: 44% ####...   | rm -rf build/                |
| |     ! Shell                     |                              |
| |     -> rm -rf build/            | Allow this command? [y/n]    |
|   project2                        |                              |
|     2 * /U/t/D/tz                 |                              |
|       Kiro | ctx: 90% #########.  |                              |
|       Shell cargo build           |                              |
|      -> * subagent (12s)          |                              |
+------------------------------------------------------------------+
| 2 selected                                                       |
+------------------------------------------------------------------+
```

---

## Features

- **Multi-Agent Monitoring**: Track multiple AI agents across all tmux sessions and windows
- **Real-time Status**: See agent states at a glance (Idle, Processing, Awaiting Approval, Error)
- **Approval Management**: Approve or reject pending requests with single keystrokes
- **Batch Operations**: Select multiple agents and approve/reject all at once
- **Hierarchical View**: Tree display organized by Session/Window/Pane
- **Subagent Tracking**: Monitor spawned subagents (Task tool) with their status
- **Context Awareness**: View context window usage percentage when available
- **Pane Preview**: See live content from selected agent's tmux pane, in the
  colors the agent printed, and scroll back through it
- **Focus Integration**: Jump directly to any agent's pane in tmux
- **Customizable**: Configure polling interval, capture lines, and custom agent patterns

### Supported AI Agents

| Agent | Detection Method | Approval Keys |
|-------|------------------|---------------|
| **Claude Code** | `claude` command, version numbers, window title with icon | `y` / `n` |
| **Kiro CLI** | `kiro-cli` / `kiro-cli-chat` process in the pane's process tree | `y` / `n` |
| **Grok** | `grok` command (e.g. `grok-1.0.3-maco`), title ending with `- grok` | `y` / `n` |
| **OpenCode** | `opencode` command | `y` / `n` |
| **Codex CLI** | `codex` command | `y` / `n` |
| **Gemini CLI** | `gemini` command | `y` / `n` |

#### Context percentage

tmuxcc reports context **used** (`ctx: 44% ████░░░░░░`), matching what the agents
themselves print, so the bar fills up as headroom runs out. It turns yellow at 50%
and red at 80%.

The bar is shown whenever the agent puts a percentage on screen:

- **Kiro CLI** always does, in its status bar (`◔ 18%` = context *used*)
- **Claude Code** reports what is *left* only once it nears auto-compact
  (`Context left until auto-compact: 42%`), which tmuxcc inverts into a used
  reading. For a reading that is present the whole session, have your status line
  emit `ctx:NN%` from the status line hook's `context_window.used_percentage`

Other agents do not expose a percentage, so their bar stays hidden.

#### Kiro CLI notes

Kiro CLI does not write the tmux pane title, so its status is read entirely from
the pane content:

- **Approval**: the `<Tool> · <detail> requires approval` snackbar, or the
  approval dropdown options (`Yes, single permission` / `No (Tab to edit)`)
- **Question**: the question panel, recognised by its `Type a different answer…`
  option; the choices are listed in the sidebar
- **Working / Idle**: the input hint line (`Kiro is working …` vs.
  `ask a question or describe a task`)
- **Context**: the status bar reports context *used* (`◔ 18%`), which tmuxcc
  inverts into the remaining percentage it displays
- **Task summary**: Kiro has no per-turn equivalent of Claude Code's `✳ <task>`
  pane title, and writes nothing to the title itself. Set it from an agent hook
  instead — `tmux select-pane -T '<task>'` — and tmuxcc shows that text
  verbatim. Titles that still look like a shell title (`kiro-cli --resume-id
  ~/D/C/C/downloader`, i.e. ending in a path) are discarded, so the sidebar
  falls back to the abbreviated path until the session is named.
  A `kiro: ` prefix is stripped too, for the `chat.terminalTitle` setting.

Kiro's shell integration renames the pane shell to `fish (kiro-cli-term)` in
*every* pane opened from a Kiro terminal, including plain shells and panes
running other agents. That marker is explicitly ignored, so only panes with a
real `kiro-cli` process are reported as Kiro agents.

#### How a pane's agent is identified

Only the **process tree** decides which agent owns a pane: `pane_current_command`,
the pane process's command line, and its descendants up to 6 levels deep. The
pane title is excluded, because titles now carry a human-written task summary
that may name any tool the session is working on — a Grok pane titled
`Claude Code proxy … - grok` used to be reported as Claude Code.

Title branding is consulted only as a fallback, when no process matched at all:
Claude Code's `✳` glyph and Grok's ` - grok` suffix. That covers agents running
behind an ssh hop the local `ps` cannot see.

---

## Installation

### From crates.io

```bash
cargo install tmuxcc
```

### From Source

```bash
git clone https://github.com/nyanko3141592/tmuxcc.git
cd tmuxcc
cargo build --release
cargo install --path .
```

### Verifying a local change

After editing the source, this installs the binary and prints one rendered frame
from a throwaway tmux session, so the UI can be checked without attaching:

```bash
./scripts/install-and-verify.sh          # fmt + clippy + tests + install + frame
./scripts/install-and-verify.sh --no-ui  # install only
```

### Requirements

- **tmux** (must be running with at least one session)
- **Rust** 1.70+ (for building from source)

---

## Usage

### Quick Start

1. Start tmux and run AI agents in different panes
2. Launch TmuxCC from any terminal:

```bash
tmuxcc
```

### Command Line Options

```
tmuxcc [OPTIONS]

Options:
  -p, --poll-interval <MS>      Polling interval in milliseconds [default: 500]
  -l, --capture-lines <LINES>   Lines to capture from each pane [default: 100]
  -f, --config <FILE>           Path to config file
  -d, --debug                   Enable debug logging to tmuxcc.log
      --show-config-path        Show config file path and exit
      --init-config             Create default config file and exit
  -h, --help                    Print help
  -V, --version                 Print version
```

### Examples

```bash
# Run with default settings
tmuxcc

# Set polling interval to 1 second
tmuxcc -p 1000

# Capture more lines for better context
tmuxcc -l 200

# Use custom config file
tmuxcc -f ~/.config/tmuxcc/custom.toml

# Enable debug logging
tmuxcc --debug

# Initialize default config file
tmuxcc --init-config
```

---

## Key Bindings

### Navigation

| Key | Action |
|-----|--------|
| `j` / `Down` | Next agent |
| `k` / `Up` | Previous agent |
| `Tab` | Cycle through agents |
| `1`-`9` | Jump cursor to agent number |
| `Enter` | Go to selected pane (closes tmuxcc) |

### Preview Scrolling

The preview follows the live output until you scroll it; the title then shows how
far back you are (`↑12 of 143`) and the border turns yellow. `G` returns to live.
Scroll depth is limited by `--capture-lines` (100 by default).

| Key | Action |
|-----|--------|
| `J` / `K` (`Shift`+`j`/`k`) | Scroll preview down / up one row |
| `Shift`+`Down` / `Shift`+`Up` | Scroll preview down / up one row |
| `PageDown` / `PageUp` | Scroll preview half a screen |
| `Ctrl+d` / `Ctrl+u` | Scroll preview half a screen |
| `G` | Jump back to live output |
| `g` | Jump to the oldest captured output |

### Mouse

| Action | Effect |
|--------|--------|
| Wheel over the preview | Scrolls the preview |
| Wheel over the agent list | Scrolls the list viewport (the cursor stays put) |
| Click in the agent list | Selects the agent on that row |
| Click in the input box | Focuses the input |

The wheel acts on whatever sits under the pointer, so a list that already fits on
screen does not move.

### Selection

| Key | Action |
|-----|--------|
| `Space` | Toggle selection of current agent |
| `Ctrl+a` | Select all agents |
| `Esc` | Clear selection / Close popup |

### Actions

| Key | Action |
|-----|--------|
| `y` / `Y` | Approve pending request(s) |
| `n` / `N` | Reject pending request(s) |
| `a` / `A` | Approve ALL pending requests |
| `Left` / `Right` | Narrow / widen the sidebar (same as `<` / `>`) |

### View

| Key | Action |
|-----|--------|
| `s` / `S` | Toggle subagent log |
| `r` | Refresh agent list |
| `h` / `?` | Show help |
| `q` | Quit |

---

## Configuration

TmuxCC uses a TOML configuration file.

### Initialize Config

```bash
# Create default config file
tmuxcc --init-config

# Show config file location
tmuxcc --show-config-path
```

### Config File Location

| OS | Path |
|----|------|
| Linux | `~/.config/tmuxcc/config.toml` |
| macOS | `~/Library/Application Support/tmuxcc/config.toml` |
| Windows | `%APPDATA%\tmuxcc\config.toml` |

### Configuration Options

```toml
# Polling interval in milliseconds
poll_interval_ms = 500

# Number of lines to capture from each pane
capture_lines = 100

# Custom agent patterns (optional)
# Add patterns to detect additional AI agents
[[agent_patterns]]
pattern = "my-custom-agent"
agent_type = "CustomAgent"
```

---

## Status Indicators

The glyph and its color carry the state, so no status label is printed in the list.

| Icon | Color | Status |
|------|-------|--------|
| `⚠` | red | Approval / question pending (details are listed below the agent) |
| spinner | green | Working (the activity text is shown below when the agent reports one) |
| `●` | yellow | Idle — waiting for your input |
| `✗` | red | Error |
| `○` | gray | Unknown |

---

## How It Works

1. **Discovery**: TmuxCC scans all tmux sessions, windows, and panes
2. **Detection**: Identifies AI agents by process name, window title, and command line
3. **Parsing**: Agent-specific parsers analyze pane content for status and approvals
4. **Monitoring**: Continuously polls panes at configurable intervals
5. **Actions**: Sends keystrokes to panes for approvals/rejections

---

## Project Structure

```
tmuxcc/
├── src/
│   ├── main.rs           # Entry point
│   ├── lib.rs            # Library root
│   ├── agents/           # Agent type definitions
│   │   ├── types.rs      # AgentType, AgentStatus, MonitoredAgent
│   │   └── subagent.rs   # Subagent, SubagentType, SubagentStatus
│   ├── app/              # Application logic
│   │   ├── state.rs      # AppState, AgentTree
│   │   ├── actions.rs    # Action enum
│   │   └── config.rs     # Configuration
│   ├── monitor/          # Monitoring
│   │   └── task.rs       # Async monitoring task
│   ├── parsers/          # Agent output parsers
│   │   ├── mod.rs        # AgentParser trait
│   │   ├── claude_code.rs
│   │   ├── kiro_cli.rs
│   │   ├── grok.rs
│   │   ├── opencode.rs
│   │   ├── codex_cli.rs
│   │   └── gemini_cli.rs
│   ├── tmux/             # tmux integration
│   │   ├── client.rs     # TmuxClient
│   │   └── pane.rs       # PaneInfo, process detection
│   └── ui/               # UI implementation
│       ├── app.rs        # Main loop
│       ├── layout.rs     # Layout definitions
│       └── components/   # UI components
└── Cargo.toml
```

---

## Tech Stack

- **Language**: Rust (Edition 2021)
- **TUI Framework**: [Ratatui](https://ratatui.rs/) 0.29
- **Terminal**: [Crossterm](https://github.com/crossterm-rs/crossterm) 0.28
- **Async Runtime**: [Tokio](https://tokio.rs/)
- **CLI Parser**: [Clap](https://clap.rs/) 4

---

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

---

## Contributing

Contributions are welcome! Here's how you can help:

### Getting Started

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Make your changes
4. Run tests (`cargo test`)
5. Run clippy (`cargo clippy`)
6. Format code (`cargo fmt`)
7. Commit your changes (`git commit -m 'Add amazing feature'`)
8. Push to the branch (`git push origin feature/amazing-feature`)
9. Open a Pull Request

### Areas for Contribution

- **New Agent Support**: Add parsers for other AI coding assistants
- **UI Improvements**: Enhance the terminal interface
- **Performance**: Optimize polling and parsing
- **Documentation**: Improve docs and examples
- **Bug Fixes**: Report and fix issues
- **Tests**: Improve test coverage

### Code Style

- Follow Rust conventions and idioms
- Run `cargo fmt` before committing
- Ensure `cargo clippy` passes without warnings
- Add tests for new functionality

---

## Related Projects

- [Claude Code](https://claude.ai/code) - Anthropic's AI coding assistant
- [Kiro](https://kiro.dev/) - Kiro CLI agentic coding assistant
- [Grok](https://grok.x.ai/) - xAI Grok Build TUI
- [OpenCode](https://github.com/opencode-ai/opencode) - Open-source AI coding assistant
- [Codex CLI](https://github.com/openai/codex-cli) - OpenAI's Codex CLI
- [Gemini CLI](https://github.com/google/gemini-cli) - Google's Gemini CLI
