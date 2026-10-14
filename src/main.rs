use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use tapin::event::AgentKind;

/// An inbox for your AI coding agents. Approve, jump, done.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
    /// Daemon socket path (defaults to a per-user runtime dir).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Don't show desktop notifications (applies to a daemon started now).
    #[arg(long, global = true)]
    no_notify: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the session store that agents and TUIs connect to.
    Daemon,
    /// Called by an agent's hook config; reads the hook payload on stdin.
    Hook {
        #[arg(long)]
        agent: Agent,
    },
    /// Add tapin's hooks to the installed agents (Claude Code, Codex, Gemini); shows the diff first.
    Init {
        /// Apply without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Remove exactly the hooks `tapin init` added.
    Uninstall {
        /// Apply without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Check the setup and explain why approvals might not show up.
    Doctor,
}

#[derive(Clone, Copy, ValueEnum)]
enum Agent {
    Claude,
    Codex,
    Gemini,
}

impl From<Agent> for AgentKind {
    fn from(a: Agent) -> Self {
        match a {
            Agent::Claude => AgentKind::Claude,
            Agent::Codex => AgentKind::Codex,
            Agent::Gemini => AgentKind::Gemini,
        }
    }
}

fn main() -> anyhow::Result<()> {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // A hook must never fail the agent: Claude Code treats exit 2 as "block".
        Err(_) if std::env::args().nth(1).as_deref() == Some("hook") => return Ok(()),
        Err(e) => e.exit(),
    };
    let socket = cli.socket.unwrap_or_else(tapin::socket::default_path);
    match cli.command {
        Some(Cmd::Daemon) => tapin::daemon::run(&socket, !cli.no_notify),
        Some(Cmd::Hook { agent }) => {
            tapin::hook::run(agent.into(), &socket);
            Ok(())
        }
        Some(Cmd::Init { yes }) => tapin::setup::run_init(yes),
        Some(Cmd::Uninstall { yes }) => tapin::setup::run_uninstall(yes),
        Some(Cmd::Doctor) => tapin::setup::run_doctor(&socket),
        None => tapin::tui::run(&socket, cli.no_notify),
    }
}
