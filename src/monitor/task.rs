use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use super::kiro_session;
use crate::agents::{AgentStatus, AgentType, MonitoredAgent};
use crate::app::AgentTree;
use crate::parsers::ParserRegistry;
use crate::tmux::{descendant_pids, refresh_process_cache, TmuxClient};

/// Hysteresis duration - keep "Processing" status for this long after last active detection
const STATUS_HYSTERESIS_MS: u64 = 2000;

/// Update message sent from monitor to UI
#[derive(Debug, Clone)]
pub struct MonitorUpdate {
    pub agents: AgentTree,
}

/// Background task that monitors tmux panes for AI agents
pub struct MonitorTask {
    tmux_client: Arc<TmuxClient>,
    parser_registry: Arc<ParserRegistry>,
    tx: mpsc::Sender<MonitorUpdate>,
    poll_interval: Duration,
    /// Track when each agent was last seen as "active" (Processing/AwaitingApproval)
    /// Key: agent target string
    last_active: HashMap<String, Instant>,
}

impl MonitorTask {
    pub fn new(
        tmux_client: Arc<TmuxClient>,
        parser_registry: Arc<ParserRegistry>,
        tx: mpsc::Sender<MonitorUpdate>,
        poll_interval: Duration,
    ) -> Self {
        Self {
            tmux_client,
            parser_registry,
            tx,
            poll_interval,
            last_active: HashMap::new(),
        }
    }

    /// Runs the monitoring loop
    pub async fn run(mut self) {
        loop {
            match self.poll_agents().await {
                Ok(tree) => {
                    let update = MonitorUpdate { agents: tree };
                    if self.tx.send(update).await.is_err() {
                        debug!("Monitor channel closed, stopping");
                        break;
                    }
                }
                Err(e) => {
                    warn!("Monitor poll error: {}", e);
                }
            }

            tokio::time::sleep(self.poll_interval).await;
        }
    }

    async fn poll_agents(&mut self) -> anyhow::Result<AgentTree> {
        // Refresh process cache once per poll cycle (much faster than per-pane)
        refresh_process_cache();

        let panes = self.tmux_client.list_panes()?;
        let mut tree = AgentTree::new();

        for pane in panes {
            // Try to find a matching parser for the pane (checks command, title, cmdline)
            if let Some(parser) = self.parser_registry.find_parser_for_pane(&pane) {
                let target = pane.target();

                // Capture pane content. The styled copy keeps the colours for
                // the preview; parsers work on the stripped text.
                let styled = match self.tmux_client.capture_pane_styled(&target) {
                    Ok(c) => c,
                    Err(e) => {
                        error!("Failed to capture pane {}: {}", target, e);
                        continue;
                    }
                };
                let content = crate::ansi::strip(&styled);

                // Parse status from content
                let mut status = parser.parse_status(&content);

                // Check pane title for spinner (Claude Code / Grok / others)
                // Spinners like ⠐⠇⠋⠙⠸ in title indicate processing.
                // Grok also prefixes busy titles with "Running:".
                let title_has_spinner = pane.title.chars().any(|c| {
                    matches!(
                        c,
                        '⠿' | '⠇'
                            | '⠋'
                            | '⠙'
                            | '⠸'
                            | '⠴'
                            | '⠦'
                            | '⠧'
                            | '⠖'
                            | '⠏'
                            | '⠹'
                            | '⠼'
                            | '⠷'
                            | '⠾'
                            | '⠽'
                            | '⠻'
                            | '⠐'
                            | '⠑'
                            | '⠒'
                            | '⠓'
                            // Claude Code moon-phase busy icons in pane title
                            | '◐'
                            | '◓'
                            | '◑'
                            | '◒'
                    )
                }) || pane.title.contains("Running:");

                // If title has spinner, override to Processing
                if title_has_spinner
                    && matches!(
                        status,
                        AgentStatus::Idle | AgentStatus::Done { .. } | AgentStatus::Unknown
                    )
                {
                    status = AgentStatus::Processing {
                        activity: "Working...".to_string(),
                    };
                }

                // Apply hysteresis: if status is now Idle but was recently active, keep as Processing
                let now = Instant::now();
                let is_active = matches!(
                    status,
                    AgentStatus::Processing { .. } | AgentStatus::AwaitingApproval { .. }
                );

                if is_active {
                    // Update last active time
                    self.last_active.insert(target.clone(), now);
                } else if matches!(status, AgentStatus::Idle | AgentStatus::Done { .. }) {
                    // Check if we were recently active
                    if let Some(last) = self.last_active.get(&target) {
                        if now.duration_since(*last) < Duration::from_millis(STATUS_HYSTERESIS_MS) {
                            // Keep as Processing to avoid flicker
                            status = AgentStatus::Processing {
                                activity: "Working...".to_string(),
                            };
                        }
                    }
                }

                // Parse subagents
                let subagents = parser.parse_subagents(&content);

                // Parse context used
                let context_used = parser.parse_context_used(&content);

                // Create monitored agent. The id is keyed on the stable `%12`
                // pane id: closing a pane renumbers the panes after it, which
                // would change a target-based id and lose the cursor.
                let id_key = if pane.pane_id.is_empty() {
                    &target
                } else {
                    &pane.pane_id
                };
                let mut agent = MonitoredAgent::new(
                    format!("{}-{}", id_key, pane.pid),
                    target,
                    pane.session.clone(),
                    pane.window,
                    pane.window_name.clone(),
                    pane.pane,
                    pane.path.clone(),
                    parser.agent_type(),
                    pane.pid,
                );
                agent.status = status;
                agent.pane_id = pane.pane_id.clone();
                // Kiro CLI never puts the session name in the pane title
                // (only `kiro: <cwd>`), so take it from Kiro's session store.
                // Falls back to the pane title — the prompt-text hook — while
                // the session is still untitled.
                agent.title = match parser.agent_type() {
                    AgentType::KiroCli => kiro_session::title_for(&descendant_pids(pane.pid))
                        .unwrap_or_else(|| pane.title.clone()),
                    _ => pane.title.clone(),
                };
                agent.subagents = subagents;
                agent.last_content = content;
                agent.styled_content = styled;
                agent.context_used = context_used;
                agent.touch(); // Update last_updated

                tree.root_agents.push(agent);
            }
        }

        // Sort agents by target for consistent ordering
        tree.root_agents.sort_by(|a, b| a.target.cmp(&b.target));

        Ok(tree)
    }
}
