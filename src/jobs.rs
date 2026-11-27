//! Every Claude call the app makes, each one a headless `claude -p` on your
//! Claude subscription: ANTHROPIC_API_KEY is removed from its environment so
//! a key in the shell can never turn a run into API billing.
//!
//! The refresh runs the daily-todo skill in auto mode, as you would. The short
//! jobs run in dontAsk mode with an explicit tool list: the draft can only
//! read, the creation can only call save_issue.

use crate::config::{Config, Person, cache_dir};
use crate::connectors::{Error, Statuses, claude::Session};
use crate::snapshot::Item;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Refresh,
    Agenda,
    Draft,
    Create,
    Check,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Refresh => "todo",
            Kind::Agenda => "agenda",
            Kind::Draft => "draft",
            Kind::Create => "ticket",
            Kind::Check => "check",
        }
    }
}

pub enum Message {
    Started {
        kind: Kind,
        pid: u32,
    },
    Finished(Outcome),
    /// The slow poll: tracker, chat, code host.
    Statuses(Statuses),
    /// The Claude sessions on this machine.
    Sessions(Result<Vec<Session>, Error>),
    /// A work session started for an item, or why not.
    Work {
        key: String,
        started: Result<String, Error>,
    },
}

pub struct Outcome {
    pub kind: Kind,
    pub ok: bool,
    pub seconds: u64,
    /// The schema-checked answer of a short job.
    pub output: Option<Value>,
    /// Short reason when not ok.
    pub error: String,
}

/// A Linear ticket as drafted by Claude and shown for confirmation. Nothing
/// is created from it until you press ⏎ on it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub title: String,
    pub description: String,
    pub team: String,
    pub state: String,
    /// 0 none, 1 urgent, 2 high, 3 medium, 4 low.
    pub priority: u8,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub links: Vec<DraftLink>,
    /// An existing ticket that looks like the same subject.
    #[serde(default)]
    pub possible_duplicate: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct DraftLink {
    pub title: String,
    pub url: String,
}

pub const PRIORITIES: [&str; 5] = ["None", "Urgent", "High", "Medium", "Low"];

struct Call {
    prompt: String,
    system: String,
    model: String,
    permission: &'static str,
    tools: Vec<String>,
    schema: Option<Value>,
}

const HEADLESS: &str = "You run headless inside deck todo, the user's terminal todo. Nobody reads your \
reply and nobody can answer a question: never ask one, decide or stop. Never write to a work log, \
to memory files, or to any repo; this run is not a task to log. Never mention Claude, AI or an \
assistant in anything you write to Linear.";

pub fn refresh(config: &Config, tx: Sender<Message>) {
    let window = match config.depth.window() {
        Some(window) => format!(
            " On the first call of the day, collect asks {window} instead of the skill's default window."
        ),
        None => String::new(),
    };
    let call = Call {
        prompt: format!("/{}{window}", config.skill),
        system: format!(
            "{HEADLESS} Run the {} skill end to end and save its snapshot, links \
             included. That snapshot is the only file you write. Reply with one line: the item count.",
            config.skill
        ),
        model: config.model.clone(),
        permission: "auto",
        tools: Vec::new(),
        schema: None,
    };
    spawn(Kind::Refresh, call, config, tx);
}

pub fn agenda(config: &Config, today: &str, tx: Sender<Message>) {
    let people: Vec<String> = config
        .people
        .iter()
        .enumerate()
        .map(
            |(index, Person { name, email })| match (index, email.is_empty()) {
                (0, _) => format!("- {name}: the user, their own calendar"),
                (_, false) => format!("- {name}: {email}"),
                (_, true) => format!(
                    "- {name}: address unknown, find their work address among the user's contacts"
                ),
            },
        )
        .collect();
    let offset = chrono::Local::now().format("%:z");
    let call = Call {
        prompt: format!(
            "List the calendar events of {today} (00:00 to 24:00, UTC{offset}) for each person below, \
             in this order, with the same names:\n{}\n\n\
             Read coworkers' calendars in as few calls as the calendar tools allow. \
             Keep only timed events (skip all-day ones), times as HH:MM at UTC{offset}, sorted by start. \
             Skip events the person declined. A calendar that cannot be read: no events and `error` says why. \
             Return each person's email as used or found.",
            people.join("\n")
        ),
        system: HEADLESS.into(),
        model: config.small_model.clone(),
        permission: "dontAsk",
        tools: config.calendar_tools.clone(),
        schema: Some(json!({
            "type": "object",
            "properties": {
                "people": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "email": { "type": "string" },
                            "error": { "type": ["string", "null"] },
                            "events": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "start": { "type": "string" },
                                        "end": { "type": "string" },
                                        "title": { "type": "string" },
                                        "location": { "type": "string" }
                                    },
                                    "required": ["start", "end", "title"]
                                }
                            }
                        },
                        "required": ["name", "events"]
                    }
                }
            },
            "required": ["people"]
        })),
    };
    spawn(Kind::Agenda, call, config, tx);
}

fn draft_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "description": { "type": "string" },
            "team": { "type": "string" },
            "state": { "type": "string" },
            "priority": { "type": "integer", "minimum": 0, "maximum": 4 },
            "project": { "type": ["string", "null"] },
            "links": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": { "title": { "type": "string" }, "url": { "type": "string" } },
                    "required": ["title", "url"]
                }
            },
            "possible_duplicate": { "type": ["string", "null"] }
        },
        "required": ["title", "description", "team", "state", "priority", "links"]
    })
}

pub fn draft(config: &Config, item: &Item, tx: Sender<Message>) {
    let links: Vec<Value> = item
        .all_links()
        .iter()
        .map(|link| json!({ "title": link.label, "url": link.url }))
        .collect();
    let source = json!({
        "line": item.line,
        "bucket": item.bucket,
        "asks": item.asks,
        "links": links,
    });
    let call = Call {
        prompt: format!(
            "This item of the user's todo has no Linear ticket. Draft one; do not create it.\n\n{}\n\n\
             Read the source thread first (Gmail get_thread, Slack read_thread, the team inbox subject) \
             to get the facts. Then:\n\
             - title: at most ~70 characters, `<Client>: <what to do>` when the work is for a client, English.\n\
             - description: Markdown, 2 to 4 short lines: what is asked, who waits and since when, the \
               source (thread subject or channel and date). No shopper data, no secrets, no quoted bodies.\n\
             - team: the team the user's own tickets are in (look at their recent issues with list_issues \
               assignee me); state: that team's state for work planned now (e.g. a current sprint or Todo \
               state, from list_issue_statuses); priority: 1 urgent, 2 high when a client waits, else 3.\n\
             - links: the source threads, with a short title each.\n\
             - possible_duplicate: an existing open issue on the same subject (search list_issues by client \
               and topic), else null.",
            serde_json::to_string_pretty(&source).unwrap_or_default()
        ),
        system: format!("{HEADLESS} You can only read. Create or change nothing."),
        model: config.small_model.clone(),
        permission: "dontAsk",
        tools: [
            "mcp__claude_ai_Gmail__get_thread",
            "mcp__claude_ai_Gmail__search_threads",
            "mcp__plugin_slack_slack__slack_read_thread",
            "mcp__plugin_slack_slack__slack_read_channel",
            "mcp__linear__get_issue",
            "mcp__linear__list_issues",
            "mcp__linear__list_teams",
            "mcp__linear__list_issue_statuses",
            "mcp__linear__list_projects",
        ]
        .map(String::from)
        .to_vec(),
        schema: Some(draft_schema()),
    };
    spawn(Kind::Draft, call, config, tx);
}

pub fn create(config: &Config, draft: &Draft, tx: Sender<Message>) {
    let mut issue = json!({
        "team": draft.team,
        "title": draft.title,
        "description": draft.description,
        "state": draft.state,
        "priority": draft.priority,
        "assignee": "me",
        "links": draft.links,
    });
    if let Some(project) = draft.project.as_ref().filter(|project| !project.is_empty()) {
        issue["project"] = json!(project);
    }
    let call = Call {
        prompt: format!(
            "The user confirmed this ticket. Create it with one save_issue call, passing exactly these \
             fields, text unchanged:\n\n{}\n\nThen return the new issue's identifier (e.g. ABC-123) and url.",
            serde_json::to_string_pretty(&issue).unwrap_or_default()
        ),
        system: format!("{HEADLESS} Make exactly one save_issue call, nothing else."),
        model: config.small_model.clone(),
        permission: "dontAsk",
        tools: vec!["mcp__linear__save_issue".into()],
        schema: Some(json!({
            "type": "object",
            "properties": { "identifier": { "type": "string" }, "url": { "type": "string" } },
            "required": ["identifier", "url"]
        })),
    };
    spawn(Kind::Create, call, config, tx);
}

/// Claude's verdict on one item, from its sources as they are now.
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Verdict {
    pub done: bool,
    /// What closed it, or what still waits.
    pub why: String,
}

pub fn check(config: &Config, item: &Item, tx: Sender<Message>) {
    let links: Vec<Value> = item
        .all_links()
        .iter()
        .map(|link| json!({ "title": link.label, "url": link.url }))
        .collect();
    let source = json!({
        "key": item.key,
        "line": item.line,
        "asks": item.asks,
        "links": links,
        "status": item.status,
        "done": item.done_text(),
    });
    let user = config
        .people
        .first()
        .map(|person| format!(" The user is {}.", person.name))
        .unwrap_or_default();
    let call = Call {
        prompt: format!(
            "Check whether this item of the user's todo is done.{user}\n\n{}\n\n\
             Read each of its sources as it is now (Gmail get_thread, Slack read_thread, Linear get_issue \
             and list_comments, `gh pr view`). It is done when every ask in it was answered by the user \
             after it was asked (a later message from the user in the thread), its ticket moved to done, \
             deployed, canceled or in review, its pull request merged or its review comments answered, \
             or its alert resolved. Partly answered or unchanged: not done. Never call it done because it \
             stopped surfacing or because `done` says so: check.\n\
             - done: true or false.\n\
             - why: at most ~6 words. Done: what closed it, as the todo prints it after an arrow, e.g. \
               \"answered Ann (mail)\", \"Deployed\", \"merged\". Not done: what still waits, e.g. \
               \"no reply to Ann yet\".",
            serde_json::to_string_pretty(&source).unwrap_or_default()
        ),
        system: format!("{HEADLESS} You can only read. Create or change nothing."),
        model: config.small_model.clone(),
        permission: "dontAsk",
        tools: [
            "mcp__claude_ai_Gmail__get_thread",
            "mcp__claude_ai_Gmail__search_threads",
            "mcp__plugin_slack_slack__slack_read_thread",
            "mcp__plugin_slack_slack__slack_read_channel",
            "mcp__linear__get_issue",
            "mcp__linear__list_comments",
            "Bash(gh pr view:*)",
        ]
        .map(String::from)
        .to_vec(),
        schema: Some(json!({
            "type": "object",
            "properties": { "done": { "type": "boolean" }, "why": { "type": "string" } },
            "required": ["done", "why"]
        })),
    };
    spawn(Kind::Check, call, config, tx);
}

fn spawn(kind: Kind, call: Call, config: &Config, tx: Sender<Message>) {
    let mut command = Command::new(&config.claude_bin);
    command
        .arg("-p")
        .arg(&call.prompt)
        .args(["--output-format", "json", "--no-session-persistence"])
        .args(["--permission-mode", call.permission])
        .args(["--append-system-prompt", &call.system])
        .current_dir(config.workdir())
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !call.model.is_empty() {
        command.args(["--model", &call.model]);
    }
    if !call.tools.is_empty() {
        command.arg("--allowedTools").arg(call.tools.join(","));
    }
    if let Some(schema) = &call.schema {
        command.args(["--json-schema", &schema.to_string()]);
    }

    std::thread::spawn(move || {
        let started = Instant::now();
        let finish = |ok: bool, output: Option<Value>, error: String| {
            let _ = tx.send(Message::Finished(Outcome {
                kind,
                ok,
                seconds: started.elapsed().as_secs(),
                output,
                error,
            }));
        };
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => return finish(false, None, format!("cannot start claude: {error}")),
        };
        let _ = tx.send(Message::Started {
            kind,
            pid: child.id(),
        });
        let result = match child.wait_with_output() {
            Ok(result) => result,
            Err(error) => return finish(false, None, error.to_string()),
        };
        let stdout = String::from_utf8_lossy(&result.stdout).to_string();
        let stderr = String::from_utf8_lossy(&result.stderr).to_string();
        let _ = std::fs::create_dir_all(cache_dir());
        let _ = std::fs::write(
            cache_dir().join(format!("{}.log", kind.label())),
            format!("{stdout}\n--- stderr\n{stderr}"),
        );

        let parsed: Option<Value> = serde_json::from_str(&stdout).ok();
        let Some(parsed) = parsed else {
            let reason = if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            };
            return finish(false, None, first_line(&reason, result.status.code()));
        };
        let is_error = parsed
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(!result.status.success());
        if is_error {
            let reason = parsed
                .get("result")
                .and_then(Value::as_str)
                .unwrap_or("claude reported an error");
            return finish(false, None, first_line(reason, result.status.code()));
        }
        let denied = parsed
            .get("permission_denials")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let output = parsed
            .get("structured_output")
            .cloned()
            .filter(|value| !value.is_null());
        if call.schema.is_some() && output.is_none() {
            return finish(
                false,
                None,
                format!("no structured answer ({denied} tool calls denied)"),
            );
        }
        finish(true, output, String::new());
    });
}

fn first_line(text: &str, code: Option<i32>) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no output");
    let short: String = line.chars().take(160).collect();
    match code {
        Some(code) if code != 0 => format!("{short} (exit {code})"),
        _ => short,
    }
}
