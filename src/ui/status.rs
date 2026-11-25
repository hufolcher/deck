//! What the connectors say about one item, as short tinted phrases: the
//! detail box shows them all, the list row only what asks for attention.

use super::{ACCENT, AMBER, GREEN, MUTED, RED};
use crate::connectors::Statuses;
use crate::connectors::claude::{Session, State};
use crate::connectors::github::{Checks, Pull, PullRef, PullState, Review};
use crate::connectors::linear::{Ticket, TicketId};
use crate::connectors::slack::Thread;
use crate::snapshot::{Item, parse_time};
use chrono::{DateTime, Local, TimeZone};
use ratatui::style::Color;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tone {
    Muted,
    Good,
    Warn,
    Bad,
    Accent,
}

impl Tone {
    pub fn color(self) -> Color {
        match self {
            Tone::Muted => MUTED,
            Tone::Good => GREEN,
            Tone::Warn => AMBER,
            Tone::Bad => RED,
            Tone::Accent => ACCENT,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Phrase {
    pub text: String,
    pub tone: Tone,
}

fn phrase(text: impl Into<String>, tone: Tone) -> Phrase {
    Phrase {
        text: text.into(),
        tone,
    }
}

/// `10:12` today, `Mon 24` before.
pub fn when(at: DateTime<Local>, now: DateTime<Local>) -> String {
    if at.date_naive() == now.date_naive() {
        at.format("%H:%M").to_string()
    } else {
        at.format("%a %-d").to_string()
    }
}

fn iso(text: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Local))
}

fn since(item: &Item) -> Option<DateTime<Local>> {
    item.added_at
        .as_deref()
        .and_then(parse_time)
        .map(|at| at.with_timezone(&Local))
}

pub fn ticket(TicketId(id): &TicketId, ticket: &Ticket, now: DateTime<Local>) -> Vec<Phrase> {
    let tone = if ticket.closed() {
        Tone::Good
    } else {
        Tone::Accent
    };
    let mut phrases = vec![phrase(format!("{id} {}", ticket.state.name), tone)];
    if let Some(assignee) = &ticket.assignee {
        phrases.push(phrase(format!("on {assignee}"), Tone::Muted));
    }
    if let Some((user, at)) = &ticket.last_comment {
        let at = iso(at)
            .map(|at| format!(" {}", when(at, now)))
            .unwrap_or_default();
        let (who, tone) = if user.is_me {
            ("you".to_string(), Tone::Muted)
        } else {
            (user.name.clone(), Tone::Warn)
        };
        phrases.push(phrase(format!("{who} commented{at}"), tone));
    }
    phrases
}

pub fn thread(thread: &Thread, now: DateTime<Local>) -> Phrase {
    match &thread.last {
        None => phrase("Slack: no reply yet", Tone::Muted),
        Some(reply) => {
            let at = Local
                .timestamp_opt(reply.ts.seconds as i64, 0)
                .single()
                .map(|at| format!(" {}", when(at, now)))
                .unwrap_or_default();
            let tone = if reply.mine { Tone::Muted } else { Tone::Warn };
            phrase(format!("Slack: {} replied{at}", reply.by), tone)
        }
    }
}

pub fn pull(reference: &PullRef, pull: &Pull) -> Phrase {
    let (state, mut tone) = match (pull.state, pull.draft) {
        (PullState::Merged, _) => ("merged", Tone::Good),
        (PullState::Closed, _) => ("closed", Tone::Muted),
        (PullState::Open, true) => ("draft", Tone::Muted),
        (PullState::Open, false) => ("open", Tone::Accent),
    };
    let mut text = format!("{reference} {state}");
    if pull.state == PullState::Open {
        text += match pull.checks {
            Checks::None => "",
            Checks::Pending => " · checks running",
            Checks::Passing => " · checks ✓",
            Checks::Failing => " · checks ✗",
        };
        text += match pull.review {
            Review::Approved => " · approved",
            Review::ChangesRequested => " · changes asked",
            Review::Required => " · review pending",
            Review::None => "",
        };
        if pull.checks == Checks::Failing || pull.review == Review::ChangesRequested {
            tone = Tone::Bad;
        }
    }
    phrase(text, tone)
}

pub fn session(session: &Session, now: DateTime<Local>) -> Phrase {
    let started = Local.timestamp_millis_opt(session.started_at).single();
    let minutes = started.map_or(0, |at| (now - at).num_minutes().max(0));
    let age = if minutes >= 60 {
        format!("{}h{:02}", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    };
    match session.state {
        State::Working => phrase(format!("⚙ Claude working {age}"), Tone::Accent),
        State::Blocked => phrase("⏸ Claude needs you", Tone::Bad),
        State::Busy => phrase("● Claude busy", Tone::Accent),
        State::Idle => phrase("○ Claude idle", Tone::Muted),
        State::Done => phrase(
            format!(
                "✓ Claude done{}",
                started
                    .map(|at| format!(", started {}", when(at, now)))
                    .unwrap_or_default()
            ),
            Tone::Good,
        ),
        State::Stopped => phrase("■ Claude stopped", Tone::Muted),
    }
}

/// Everything known, for the detail box; connector errors last.
pub fn detail(statuses: &Statuses, item: &Item, now: DateTime<Local>) -> Vec<Phrase> {
    let mut phrases: Vec<Phrase> = Vec::new();
    for (id, found) in statuses.tickets.for_item(item) {
        phrases.extend(ticket(&id, found, now));
    }
    for (_, found) in statuses.threads.for_item(item) {
        phrases.push(thread(found, now));
    }
    for (reference, found) in statuses.pulls.for_item(item) {
        phrases.push(pull(&reference, found));
    }
    let errors = [
        statuses.tickets.error_for(item),
        statuses.threads.error_for(item),
        statuses.pulls.error_for(item),
    ];
    phrases.extend(
        errors
            .into_iter()
            .flatten()
            .map(|error| phrase(error.to_string(), Tone::Muted)),
    );
    phrases
}

/// The row's badges: a session at work or waiting, a closed ticket, someone
/// who wrote since the item came in.
pub fn row(statuses: &Statuses, sessions: &[&Session], item: &Item) -> Vec<Phrase> {
    let mut phrases = Vec::new();
    match sessions.first().map(|session| session.state) {
        Some(State::Working | State::Busy) => phrases.push(phrase("⚙", Tone::Accent)),
        Some(State::Blocked) => phrases.push(phrase("⏸ needs you", Tone::Bad)),
        _ => {}
    }
    let tickets = statuses.tickets.for_item(item);
    if let Some((_, closed)) = tickets.iter().find(|(_, ticket)| ticket.closed()) {
        phrases.push(phrase(format!("✓ {}", closed.state.name), Tone::Good));
    }
    let added = since(item);
    let commenter = tickets.iter().find_map(|(_, ticket)| {
        let (user, at) = ticket.last_comment.as_ref()?;
        let newer = match (iso(at), added) {
            (Some(at), Some(added)) => at > added,
            _ => false,
        };
        (!user.is_me && newer).then(|| user.name.clone())
    });
    let replier = statuses
        .threads
        .for_item(item)
        .into_iter()
        .find_map(|(_, thread)| {
            let reply = thread.last.as_ref()?;
            (!reply.mine).then(|| reply.by.clone())
        });
    if let Some(name) = commenter.or(replier) {
        phrases.push(phrase(format!("↩ {name}"), Tone::Warn));
    }
    phrases
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::claude::Kind;
    use crate::connectors::linear::{State as TicketState, StateKind, User};
    use crate::connectors::slack::{Ask, Reply, Ts};
    use crate::connectors::{Error, Report};
    use serde_json::json;
    use std::collections::HashMap;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 25, 15, 0, 0).unwrap()
    }

    fn item() -> Item {
        serde_json::from_value(json!({
            "key": "ACME-1",
            "added_at": "2026-11-25T09:00+00:00",
            "asks": { "slack": "C0123456789 1764061200.000001", "pr": "acme/api#3" }
        }))
        .unwrap()
    }

    fn open_ticket(comment_by_me: bool, at: &str) -> Ticket {
        Ticket {
            state: TicketState {
                name: "In Progress".into(),
                kind: StateKind::Started,
            },
            assignee: Some("Bob".into()),
            last_comment: Some((
                User {
                    name: if comment_by_me { "Ann" } else { "Bob" }.into(),
                    is_me: comment_by_me,
                },
                at.into(),
            )),
        }
    }

    fn statuses(
        ticket: Ticket,
        reply: Option<Reply>,
        pull: Option<Pull>,
        slack_error: bool,
    ) -> Statuses {
        let ask: Ask = "C0123456789 1764061200.000001".parse().unwrap();
        Statuses {
            tickets: Report {
                found: HashMap::from([(TicketId("ACME-1".into()), ticket)]),
                error: None,
            },
            threads: Report {
                found: if slack_error {
                    HashMap::new()
                } else {
                    HashMap::from([(
                        ask,
                        Thread {
                            replies: 1,
                            last: reply,
                        },
                    )])
                },
                error: slack_error.then_some(Error::NotConnected("slack")),
            },
            pulls: Report {
                found: pull
                    .map(|pull| HashMap::from([("acme/api#3".parse().unwrap(), pull)]))
                    .unwrap_or_default(),
                error: None,
            },
        }
    }

    fn texts(phrases: &[Phrase]) -> Vec<String> {
        phrases.iter().map(|phrase| phrase.text.clone()).collect()
    }

    #[test]
    fn detail_says_everything_in_order() {
        let failing = Pull {
            state: PullState::Open,
            draft: false,
            review: Review::Approved,
            checks: Checks::Failing,
        };
        let reply = Reply {
            by: "Bob".into(),
            mine: false,
            ts: Ts {
                seconds: 1764061200,
                micros: 2,
            },
        };
        let found = statuses(
            open_ticket(false, "2026-11-25T10:12:00.000Z"),
            Some(reply),
            Some(failing),
            false,
        );
        let phrases = detail(&found, &item(), now());
        let commented = when(iso("2026-11-25T10:12:00.000Z").unwrap(), now());
        let replied = when(Local.timestamp_opt(1764061200, 0).unwrap(), now());
        assert_eq!(
            texts(&phrases),
            vec![
                "ACME-1 In Progress".to_string(),
                "on Bob".into(),
                format!("Bob commented {commented}"),
                format!("Slack: Bob replied {replied}"),
                "api#3 open · checks ✗ · approved".into(),
            ]
        );
        assert_eq!(phrases[4].tone, Tone::Bad);
    }

    #[test]
    fn missing_connector_says_so_only_for_its_items() {
        let found = statuses(
            open_ticket(true, "2026-11-25T10:12:00.000Z"),
            None,
            None,
            true,
        );
        assert_eq!(
            texts(&detail(&found, &item(), now()))
                .last()
                .map(String::as_str),
            Some("slack not connected")
        );
        let plain: Item = serde_json::from_value(json!({ "key": "ACME-1" })).unwrap();
        assert!(
            !texts(&detail(&found, &plain, now())).contains(&"slack not connected".to_string())
        );
    }

    #[test]
    fn row_badges_only_what_needs_attention() {
        let session = |state| Session {
            session_id: "s".into(),
            name: "ACME-1".into(),
            cwd: "/w".into(),
            state,
            started_at: now().timestamp_millis() - 3 * 60_000,
            kind: Kind::Background {
                short_id: "s".into(),
            },
        };
        let blocked = session(State::Blocked);
        let quiet = statuses(
            open_ticket(true, "2026-11-25T10:12:00.000Z"),
            None,
            None,
            false,
        );
        assert_eq!(
            texts(&row(&quiet, &[&blocked], &item())),
            vec!["⏸ needs you"]
        );
        let done = session(State::Done);
        assert!(row(&quiet, &[&done], &item()).is_empty());
        let old_comment = statuses(
            open_ticket(false, "2026-11-25T08:00:00.000Z"),
            None,
            None,
            false,
        );
        assert!(row(&old_comment, &[], &item()).is_empty());
        let new_comment = statuses(
            open_ticket(false, "2026-11-25T10:00:00.000Z"),
            None,
            None,
            false,
        );
        assert_eq!(texts(&row(&new_comment, &[], &item())), vec!["↩ Bob"]);
        assert_eq!(session(State::Working).state, State::Working);
        assert_eq!(
            super::session(&session(State::Working), now()).text,
            "⚙ Claude working 3m"
        );
    }

    #[test]
    fn closed_ticket_and_merged_pull() {
        let mut closed = open_ticket(true, "2026-11-25T10:12:00.000Z");
        closed.state = TicketState {
            name: "Done".into(),
            kind: StateKind::Completed,
        };
        let merged = Pull {
            state: PullState::Merged,
            draft: false,
            review: Review::None,
            checks: Checks::Passing,
        };
        let found = statuses(closed, None, Some(merged), false);
        assert_eq!(texts(&row(&found, &[], &item())), vec!["✓ Done"]);
        assert!(texts(&detail(&found, &item(), now())).contains(&"api#3 merged".to_string()));
    }

    #[test]
    fn times_today_then_day() {
        assert_eq!(
            when(
                Local.with_ymd_and_hms(2026, 11, 25, 10, 12, 0).unwrap(),
                now()
            ),
            "10:12"
        );
        assert_eq!(
            when(
                Local.with_ymd_and_hms(2026, 11, 24, 10, 12, 0).unwrap(),
                now()
            ),
            "Tue 24"
        );
    }
}
