//! Hook payloads in the shape Claude Code defined and Codex CLI adopted: the
//! same event names, `tool_input.command`, `last_assistant_message`, and the
//! same PermissionRequest decision output (see each agent's hooks docs).

use crate::event::{AgentEvent, AgentKind, EventKind};
use crate::socket::Decision;
use serde_json::Value;

/// True for events where the agent waits on the hook's answer.
pub fn wants_decision(raw: &Value) -> bool {
    raw.get("hook_event_name").and_then(Value::as_str) == Some("PermissionRequest")
}

/// Hook stdout for a decision on `payload`'s PermissionRequest. `None` (no
/// output) leaves the agent's own prompt in charge.
pub fn decision_output(decision: Decision, payload: &Value) -> Option<String> {
    let decision = match decision {
        // A question is never allowed without an answer (e.g. `y` from an
        // older inbox): pass, so Claude's own question stays up.
        Decision::Allow if is_question(payload) => return None,
        Decision::Allow => serde_json::json!({ "behavior": "allow" }),
        // AskUserQuestion: the original questions plus the chosen answer.
        // Only a label among that question's options is ever sent.
        Decision::Answer(label) => {
            let (q, choices) = single_question(payload)?;
            if !choices.contains(&label) {
                return None;
            }
            let questions = payload.get("tool_input")?.get("questions")?.clone();
            serde_json::json!({
                "behavior": "allow",
                "updatedInput": { "questions": questions, "answers": { q: label } },
            })
        }
        Decision::Deny => serde_json::json!({
            "behavior": "deny",
            "message": "Denied by the user from tapin.",
        }),
        Decision::Pass => return None,
    };
    let out = serde_json::json!({
        "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": decision }
    });
    Some(out.to_string())
}

pub fn parse(raw: &Value, agent: AgentKind) -> Option<AgentEvent> {
    let name = raw.get("hook_event_name")?.as_str()?;
    if agent == AgentKind::Gemini {
        return parse_gemini(raw, name, agent);
    }
    let kind = match name {
        "SessionStart" => EventKind::Started,
        "UserPromptSubmit" => EventKind::PromptSubmitted,
        "SessionEnd" => EventKind::Ended,
        "Stop" => EventKind::TurnDone {
            message: str_field(raw, "last_assistant_message"),
        },
        "PermissionRequest" => match question_text(raw) {
            Some((summary, input)) => EventKind::PermissionRequested {
                summary,
                input,
                choices: single_question(raw).map(|(_, c)| c),
                question: true,
            },
            None => EventKind::PermissionRequested {
                summary: tool_summary(raw),
                input: tool_input_text(raw),
                choices: None,
                question: is_question(raw),
            },
        },
        "PostToolUse" | "PostToolUseFailure" => EventKind::ToolFinished,
        "PreToolUse" => EventKind::ToolStarted {
            summary: tool_summary(raw),
        },
        _ => return None,
    };
    Some(AgentEvent {
        session_id: raw.get("session_id")?.as_str()?.to_string(),
        agent,
        cwd: str_field(raw, "cwd"),
        model: raw
            .get("model")
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
            .map(String::from),
        transcript: transcript(raw),
        kind,
    })
}

/// AskUserQuestion's questions, readable: `AskUserQuestion: <header>`, and
/// each question followed by `1. label — description` lines.
fn question_text(raw: &Value) -> Option<(String, String)> {
    if raw.get("tool_name")?.as_str()? != "AskUserQuestion" {
        return None;
    }
    let questions = raw.get("tool_input")?.get("questions")?.as_array()?;
    let header = questions
        .first()
        .and_then(|q| q.get("header"))
        .and_then(Value::as_str)
        .unwrap_or("question");
    let mut text = Vec::new();
    for q in questions {
        text.push(
            q.get("question")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        );
        for (i, o) in q.get("options")?.as_array()?.iter().enumerate() {
            let label = o.get("label").and_then(Value::as_str).unwrap_or("");
            match o.get("description").and_then(Value::as_str) {
                Some(d) if !d.is_empty() => text.push(format!("{}. {label} — {d}", i + 1)),
                _ => text.push(format!("{}. {label}", i + 1)),
            }
        }
    }
    Some((
        format!("AskUserQuestion: {}", crate::inbox::one_line(header)),
        text.join("\n"),
    ))
}

/// The one question tapin can answer with a number: exactly one question,
/// single choice. Its text and option labels.
fn single_question(raw: &Value) -> Option<(String, Vec<String>)> {
    if raw.get("tool_name")?.as_str()? != "AskUserQuestion" {
        return None;
    }
    let questions = raw.get("tool_input")?.get("questions")?.as_array()?;
    let [q] = questions.as_slice() else {
        return None;
    };
    if q.get("multiSelect").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let labels: Vec<String> = q
        .get("options")?
        .as_array()?
        .iter()
        .map(|o| o.get("label").and_then(Value::as_str).map(String::from))
        .collect::<Option<_>>()?;
    (!labels.is_empty() && labels.len() <= 9)
        .then(|| Some((q.get("question")?.as_str()?.to_string(), labels)))
        .flatten()
}

/// True for Claude's AskUserQuestion tool.
pub fn is_question(raw: &Value) -> bool {
    raw.get("tool_name").and_then(Value::as_str) == Some("AskUserQuestion")
}

/// The whole tool input for review: the command verbatim, else pretty JSON.
fn tool_input_text(raw: &Value) -> String {
    let Some(input) = raw.get("tool_input") else {
        return String::new();
    };
    match input.get("command").and_then(Value::as_str) {
        Some(command) => command.to_string(),
        None => serde_json::to_string_pretty(input).unwrap_or_default(),
    }
}

/// `Bash(ls -la)`, `Edit(src/main.rs)`: tool name plus its most telling input.
fn tool_summary(raw: &Value) -> String {
    let tool = raw
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("tool");
    let input = raw.get("tool_input");
    // Codex's apply_patch: the "command" is the whole patch; name its files instead.
    if tool == "apply_patch" {
        if let Some(patch) = input.and_then(|i| i.get("command")).and_then(Value::as_str) {
            let files: Vec<&str> = patch
                .lines()
                .filter_map(|l| {
                    ["*** Update File: ", "*** Add File: ", "*** Delete File: "]
                        .iter()
                        .find_map(|p| l.strip_prefix(p))
                })
                .collect();
            if !files.is_empty() {
                return format!("apply_patch({})", crate::inbox::one_line(&files.join(", ")));
            }
        }
    }
    let arg = ["command", "file_path", "pattern", "url"]
        .iter()
        .find_map(|k| input?.get(*k)?.as_str());
    match arg {
        Some(arg) => format!("{tool}({})", crate::inbox::one_line(arg)),
        None => tool.to_string(),
    }
}

/// Gemini CLI uses its own event names (docs/hooks/reference.md) on the same
/// payload shape. It has no blocking permission event: `Notification` with
/// `ToolPermission` is observability only, so tapin shows NEEDS but the
/// answer stays in Gemini.
fn parse_gemini(raw: &Value, name: &str, agent: AgentKind) -> Option<AgentEvent> {
    let kind = match name {
        "SessionStart" => EventKind::Started,
        "BeforeAgent" => EventKind::PromptSubmitted,
        "BeforeTool" => EventKind::ToolStarted {
            summary: tool_summary(raw),
        },
        "AfterTool" => EventKind::ToolFinished,
        "AfterAgent" => EventKind::TurnDone {
            message: str_field(raw, "prompt_response"),
        },
        "SessionEnd" => EventKind::Ended,
        "Notification"
            if raw.get("notification_type").and_then(Value::as_str) == Some("ToolPermission") =>
        {
            // Name the request by its own message: BeforeTool may only fire
            // after the user confirms, so the row can't rely on it.
            EventKind::PermissionRequested {
                summary: crate::inbox::one_line(&str_field(raw, "message")),
                input: str_field(raw, "message"),
                choices: None,
                question: false,
            }
        }
        _ => return None,
    };
    Some(AgentEvent {
        session_id: raw.get("session_id")?.as_str()?.to_string(),
        agent,
        cwd: str_field(raw, "cwd"),
        model: None,
        transcript: transcript(raw),
        kind,
    })
}

fn transcript(raw: &Value) -> Option<String> {
    Some(str_field(raw, "transcript_path")).filter(|p| !p.is_empty())
}

fn str_field(raw: &Value, key: &str) -> String {
    raw.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
