//! The normalized event vocabulary every agent adapter produces.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    #[default]
    Claude,
    Codex,
    Gemini,
}

impl AgentKind {
    pub fn name(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Gemini => "gemini",
        }
    }

    /// True if the agent holds back its own approval prompt until the
    /// PermissionRequest hook returns (Codex: openai/codex#39835), so a hook
    /// that waits on tapin also delays the user's terminal.
    pub fn prompt_waits_for_hook(self) -> bool {
        matches!(self, AgentKind::Codex)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentEvent {
    pub session_id: String,
    pub agent: AgentKind,
    pub cwd: String,
    /// The model, when the payload names it (Codex does on every event).
    pub model: Option<String>,
    /// The agent's transcript file, when the payload names it.
    pub transcript: Option<String>,
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventKind {
    Started,
    PromptSubmitted,
    ToolStarted {
        summary: String,
    },
    /// The agent is blocked until someone approves or denies this tool use.
    PermissionRequested {
        summary: String,
        input: String,
        /// Option labels when it's a question tapin can answer with a number.
        choices: Option<Vec<String>>,
    },
    ToolFinished,
    TurnDone {
        message: String,
    },
    Ended,
}
