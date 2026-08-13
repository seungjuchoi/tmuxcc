//! Temporary verification helper: prints how tmuxcc sees every live pane.

use tmuxcc::agents::{AgentType, MonitoredAgent};
use tmuxcc::parsers::ParserRegistry;
use tmuxcc::tmux::{refresh_process_cache, TmuxClient};

fn main() -> anyhow::Result<()> {
    refresh_process_cache();
    let client = TmuxClient::new();
    let registry = ParserRegistry::new();

    for pane in client.list_panes()? {
        let Some(parser) = registry.find_parser_for_pane(&pane) else {
            continue;
        };
        let mut agent = MonitoredAgent::new(
            format!("{}-{}", pane.target(), pane.pid),
            pane.target(),
            pane.session.clone(),
            pane.window,
            pane.window_name.clone(),
            pane.pane,
            pane.path.clone(),
            parser.agent_type(),
            pane.pid,
        );
        agent.title = pane.title.clone();

        println!(
            "{:<12} {:<11} title={:?}\n{:<12} {:<11} summary={:?}",
            agent.target,
            format!("{:?}", agent.agent_type),
            agent.title,
            "",
            "",
            agent.task_summary(),
        );
    }
    let _ = AgentType::KiroCli;
    Ok(())
}
