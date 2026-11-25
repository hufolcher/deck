//! The MCP tools: one typed variant per tool, arguments checked by serde.

use crate::config::{Config, Depth, home};
use crate::connectors::claude::{Claude, Started};
use crate::connectors::shell::System;
use crate::connectors::{self};
use crate::ping::Ping;
use crate::schedule::{Job, Schedule};
use crate::snapshot::{self, Item, Link};
use crate::ui::status;
use chrono::{DateTime, Duration, FixedOffset, Local, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Deserialize, Default)]
pub struct Nothing {}

#[derive(Deserialize)]
pub struct KeyArg {
    key: String,
}

#[derive(Deserialize)]
pub struct IdArg {
    id: u32,
}

#[derive(Deserialize)]
pub struct NewLink {
    label: String,
    url: String,
}

#[derive(Deserialize)]
pub struct Add {
    key: String,
    line: String,
    #[serde(default = "today")]
    bucket: String,
    #[serde(default)]
    links: Vec<NewLink>,
}

fn today() -> String {
    "today".into()
}

#[derive(Deserialize)]
pub struct Done {
    key: String,
    /// What closed it: shown after the arrow.
    text: String,
}

#[derive(Deserialize)]
pub struct Move {
    key: String,
    bucket: String,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum JobName {
    Refresh,
    Work,
}

#[derive(Deserialize)]
pub struct Plan {
    /// `HH:MM` (today, or tomorrow once past) or an RFC 3339 time.
    at: String,
    job: JobName,
    /// The item, for `work`.
    key: Option<String>,
    every_minutes: Option<u32>,
    #[serde(default)]
    note: String,
}

/// Only what is given changes.
#[derive(Deserialize)]
pub struct Settings {
    refresh_minutes: Option<u32>,
    status_minutes: Option<u32>,
    depth: Option<Depth>,
    active_hours: Option<[u32; 2]>,
    weekdays_only: Option<bool>,
    model: Option<String>,
    small_model: Option<String>,
    ping: Option<Ping>,
}

#[derive(Deserialize)]
#[serde(tag = "name", content = "arguments", rename_all = "snake_case")]
pub enum Call {
    TodoList(Nothing),
    TodoStatus(KeyArg),
    TodoAdd(Add),
    TodoDone(Done),
    TodoReopen(KeyArg),
    TodoMove(Move),
    WorkStart(KeyArg),
    ScheduleAdd(Plan),
    ScheduleList(Nothing),
    ScheduleRemove(IdArg),
    SettingsGet(Nothing),
    SettingsSet(Settings),
}

#[derive(Serialize)]
struct Text {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
}

#[derive(Serialize)]
pub struct Result {
    content: Vec<Text>,
    #[serde(rename = "isError")]
    is_error: bool,
}

impl Result {
    fn ok(text: impl Into<String>) -> Result {
        Result {
            content: vec![Text {
                kind: "text",
                text: text.into(),
            }],
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Result {
        Result {
            content: vec![Text {
                kind: "text",
                text: text.into(),
            }],
            is_error: true,
        }
    }
}

impl<E: std::fmt::Display> From<std::result::Result<String, E>> for Result {
    fn from(outcome: std::result::Result<String, E>) -> Result {
        match outcome {
            Ok(text) => Result::ok(text),
            Err(error) => Result::error(error.to_string()),
        }
    }
}

#[derive(Serialize)]
pub struct Tool {
    name: &'static str,
    description: &'static str,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
}

#[derive(Serialize)]
pub struct Listing {
    tools: Vec<Tool>,
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

pub fn list() -> Listing {
    let key = json!({ "type": "string", "description": "the item's key, e.g. ACME-12 or a slug" });
    let bucket = json!({ "type": "string", "enum": ["now", "today", "later"] });
    let tool = |name, description, input_schema| Tool {
        name,
        description,
        input_schema,
    };
    Listing {
        tools: vec![
            tool(
                "todo_list",
                "Every item of the todo: bucket, key, line, done, links.",
                schema(json!({}), &[]),
            ),
            tool(
                "todo_status",
                "What Linear, Slack, GitHub and the Claude sessions say about one item now.",
                schema(json!({ "key": key }), &["key"]),
            ),
            tool(
                "todo_add",
                "Add an item at the end of its bucket.",
                schema(
                    json!({ "key": key, "line": { "type": "string", "description": "what to do · why" },
                            "bucket": bucket,
                            "links": { "type": "array", "items": schema(json!({ "label": { "type": "string" }, "url": { "type": "string" } }), &["label", "url"]) } }),
                    &["key", "line"],
                ),
            ),
            tool(
                "todo_done",
                "Mark an item done, with what closed it.",
                schema(
                    json!({ "key": key, "text": { "type": "string" } }),
                    &["key", "text"],
                ),
            ),
            tool(
                "todo_reopen",
                "Open a done item again.",
                schema(json!({ "key": key }), &["key"]),
            ),
            tool(
                "todo_move",
                "Move an item to another bucket.",
                schema(json!({ "key": key, "bucket": bucket }), &["key", "bucket"]),
            ),
            tool(
                "work_start",
                "Start a background Claude session on the item; it stops before anything outward.",
                schema(json!({ "key": key }), &["key"]),
            ),
            tool(
                "schedule_add",
                "Plan a run while `deck todo` is open: a todo refresh or a work session on an item, once or every N minutes.",
                schema(
                    json!({ "at": { "type": "string", "description": "HH:MM (next one) or RFC 3339" },
                            "job": { "type": "string", "enum": ["refresh", "work"] },
                            "key": key, "every_minutes": { "type": "integer", "minimum": 1 },
                            "note": { "type": "string" } }),
                    &["at", "job"],
                ),
            ),
            tool("schedule_list", "The planned runs.", schema(json!({}), &[])),
            tool(
                "schedule_remove",
                "Cancel a planned run.",
                schema(json!({ "id": { "type": "integer" } }), &["id"]),
            ),
            tool("settings_get", "deck's settings.", schema(json!({}), &[])),
            tool(
                "settings_set",
                "Change settings; only the given ones change.",
                schema(
                    json!({ "refresh_minutes": { "type": "integer", "minimum": 0 },
                            "status_minutes": { "type": "integer", "minimum": 0 },
                            "depth": { "type": "string", "enum": ["today", "day", "2d", "week"] },
                            "active_hours": { "type": "array", "items": { "type": "integer" }, "minItems": 2, "maxItems": 2 },
                            "weekdays_only": { "type": "boolean" },
                            "model": { "type": "string" }, "small_model": { "type": "string" },
                            "ping": { "type": "string", "enum": ["notify", "bell", "off"] } }),
                    &[],
                ),
            ),
        ],
    }
}

/// `HH:MM` is the next such time; anything else must be RFC 3339.
pub fn parse_at(text: &str, now: DateTime<Local>) -> Option<DateTime<FixedOffset>> {
    if let Ok(time) = NaiveTime::parse_from_str(text, "%H:%M") {
        let mut at = Local
            .from_local_datetime(&now.date_naive().and_time(time))
            .earliest()?;
        if at <= now {
            at += Duration::days(1);
        }
        return Some(at.fixed_offset());
    }
    DateTime::parse_from_rfc3339(text).ok()
}

fn item(key: &str) -> std::result::Result<Item, String> {
    snapshot::load()
        .unwrap_or_default()
        .items
        .into_iter()
        .find(|item| item.key == key)
        .ok_or_else(|| format!("no item {key} in the todo"))
}

fn describe(item: &Item) -> String {
    let mut text = format!("[{}] {} — {}", item.bucket, item.key, item.line);
    if let Some(done) = item.done_text() {
        text += &format!(" (done: {done})");
    }
    for link in item.all_links() {
        text += &format!("\n    {}: {}", link.label, link.url);
    }
    text
}

impl Call {
    pub fn run(self) -> Result {
        let now = Local::now();
        match self {
            Call::TodoList(_) => {
                let snapshot = snapshot::load().unwrap_or_default();
                let lines: Vec<String> = snapshot
                    .display_order()
                    .into_iter()
                    .map(|index| describe(&snapshot.items[index]))
                    .collect();
                Result::ok(if lines.is_empty() {
                    "The todo is empty.".into()
                } else {
                    lines.join("\n")
                })
            }
            Call::TodoStatus(KeyArg { key }) => item(&key)
                .map(|item| {
                    let config = Config::load();
                    let statuses = connectors::poll_all(
                        System,
                        &config.endpoints,
                        std::slice::from_ref(&item),
                    );
                    let mut lines: Vec<String> = status::detail(&statuses, &item, now)
                        .into_iter()
                        .map(|phrase| phrase.text)
                        .collect();
                    let started = Started::load(&Started::path());
                    match Claude::new(System, &config.claude_bin, home()).sessions() {
                        Ok(sessions) => lines.extend(
                            sessions
                                .iter()
                                .filter(|session| session.concerns(&item, &started))
                                .map(|session| {
                                    format!(
                                        "{} ({})",
                                        status::session(session, now).text,
                                        session.name
                                    )
                                }),
                        ),
                        Err(error) => lines.push(format!("Claude sessions: {error}")),
                    }
                    if lines.is_empty() {
                        lines.push("Nothing known beyond the todo line.".into());
                    }
                    format!("{}\n{}", describe(&item), lines.join("\n"))
                })
                .into(),
            Call::TodoAdd(Add {
                key,
                line,
                bucket,
                links,
            }) => {
                let links: Vec<Link> = links
                    .into_iter()
                    .map(|link| Link {
                        label: link.label,
                        url: link.url,
                    })
                    .collect();
                snapshot::add_item(&key, &bucket, &line, &links, &now.to_rfc3339())
                    .map(|()| format!("{key} added to {bucket}"))
                    .into()
            }
            Call::TodoDone(Done { key, text }) => snapshot::set_done(&key, Some(&text))
                .map(|()| format!("{key} done → {text}"))
                .into(),
            Call::TodoReopen(KeyArg { key }) => snapshot::set_done(&key, None)
                .map(|()| format!("{key} open again"))
                .into(),
            Call::TodoMove(Move { key, bucket }) => snapshot::move_item(&key, &bucket)
                .map(|()| format!("{key} moved to {bucket}"))
                .into(),
            Call::WorkStart(KeyArg { key }) => item(&key)
                .and_then(|item| {
                    let config = Config::load();
                    let id = Claude::new(System, &config.claude_bin, home())
                        .start(&item, config.workdir(), &config.model)
                        .map_err(|error| error.to_string())?;
                    let mut started = Started::load(&Started::path());
                    started.add(&key, &id);
                    started
                        .save(&Started::path())
                        .map_err(|error| error.to_string())?;
                    Ok(format!(
                        "Claude works on {key} in session {id}: `claude attach {id}` takes it over"
                    ))
                })
                .into(),
            Call::ScheduleAdd(plan) => schedule_add(plan, now).into(),
            Call::ScheduleList(_) => {
                let schedule = Schedule::load(&Schedule::path());
                let lines: Vec<String> = schedule
                    .entries
                    .iter()
                    .map(|entry| {
                        let job = match &entry.job {
                            Job::Refresh => "refresh".to_string(),
                            Job::Work { key } => format!("work on {key}"),
                        };
                        let every = entry
                            .every_minutes
                            .map(|minutes| format!(", every {minutes} min"))
                            .unwrap_or_default();
                        format!(
                            "#{} {} {job}{every} {}",
                            entry.id,
                            entry.at.format("%a %d %H:%M"),
                            entry.note
                        )
                        .trim_end()
                        .to_string()
                    })
                    .collect();
                Result::ok(if lines.is_empty() {
                    "Nothing planned.".into()
                } else {
                    lines.join("\n")
                })
            }
            Call::ScheduleRemove(IdArg { id }) => {
                let mut schedule = Schedule::load(&Schedule::path());
                match schedule.remove(id) {
                    Some(_) => schedule
                        .save(&Schedule::path())
                        .map(|()| format!("#{id} cancelled"))
                        .into(),
                    None => Result::error(format!("no planned run #{id}")),
                }
            }
            Call::SettingsGet(_) => toml::to_string_pretty(&Config::load()).into(),
            Call::SettingsSet(settings) => settings_set(settings).into(),
        }
    }
}

fn schedule_add(plan: Plan, now: DateTime<Local>) -> std::result::Result<String, String> {
    let at = parse_at(&plan.at, now)
        .ok_or_else(|| format!("`{}` is neither HH:MM nor an RFC 3339 time", plan.at))?;
    let job = match (plan.job, plan.key) {
        (JobName::Refresh, _) => Job::Refresh,
        (JobName::Work, Some(key)) => Job::Work {
            key: item(&key)?.key,
        },
        (JobName::Work, None) => return Err("work needs the item's key".into()),
    };
    let mut schedule = Schedule::load(&Schedule::path());
    let id = schedule.add(at, plan.every_minutes, job, plan.note).id;
    schedule
        .save(&Schedule::path())
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "#{id} planned for {}; it runs while `deck todo` is open",
        at.format("%a %d %H:%M")
    ))
}

fn settings_set(settings: Settings) -> std::result::Result<String, String> {
    if let Some([from, to]) = settings.active_hours
        && !(from < to && to <= 24)
    {
        return Err(format!(
            "active_hours [{from}, {to}]: from before to, to at most 24"
        ));
    }
    let mut config = Config::load();
    let mut changed = Vec::new();
    macro_rules! set {
        ($($field:ident),*) => {$(
            if let Some(value) = settings.$field {
                config.$field = value;
                changed.push(stringify!($field));
            }
        )*};
    }
    set!(
        refresh_minutes,
        status_minutes,
        depth,
        active_hours,
        weekdays_only,
        model,
        small_model,
        ping
    );
    if changed.is_empty() {
        return Ok("nothing to change".into());
    }
    config.save().map_err(|error| error.to_string())?;
    Ok(format!("changed {}", changed.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 25, 15, 0, 0).unwrap()
    }

    #[test]
    fn at_is_the_next_such_time() {
        assert_eq!(
            parse_at("16:30", now()).unwrap().naive_local().to_string(),
            "2026-11-25 16:30:00"
        );
        assert_eq!(
            parse_at("09:00", now()).unwrap().naive_local().to_string(),
            "2026-11-26 09:00:00"
        );
        assert_eq!(
            parse_at("2026-12-01T08:00:00+01:00", now())
                .unwrap()
                .to_rfc3339(),
            "2026-12-01T08:00:00+01:00"
        );
        assert_eq!(parse_at("tomorrow", now()), None);
    }

    #[test]
    fn arguments_are_typed() {
        let call: Call = serde_json::from_value(json!({ "name": "schedule_add",
            "arguments": { "at": "16:30", "job": "work", "key": "ACME-1", "every_minutes": 60 } }))
        .unwrap();
        assert!(matches!(
            call,
            Call::ScheduleAdd(Plan {
                job: JobName::Work,
                every_minutes: Some(60),
                ..
            })
        ));
        let wrong = serde_json::from_value::<Call>(
            json!({ "name": "schedule_add", "arguments": { "at": "16:30", "job": "deploy" } }),
        );
        assert!(wrong.is_err());
        let add: Call = serde_json::from_value(
            json!({ "name": "todo_add", "arguments": { "key": "x", "line": "y" } }),
        )
        .unwrap();
        assert!(matches!(add, Call::TodoAdd(Add { ref bucket, .. }) if bucket == "today"));
    }

    #[test]
    fn bad_hours_refused_before_any_write() {
        let settings: Settings =
            serde_json::from_value(json!({ "active_hours": [20, 8] })).unwrap();
        assert_eq!(
            settings_set(settings),
            Err("active_hours [20, 8]: from before to, to at most 24".into())
        );
    }
}
