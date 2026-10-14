//! Claude Code: the hook adapter, labelled as Claude.

use crate::event::{AgentEvent, AgentKind};
use serde_json::Value;

pub fn parse(raw: &Value) -> Option<AgentEvent> {
    super::hooks::parse(raw, AgentKind::Claude)
}
