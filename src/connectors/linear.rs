//! Linear: each ticket's state, assignee and last comment, in one GraphQL call
//! with a personal API key (keychain item `deck-linear`).

use super::http::{self, Http, Scheme};
use super::shell::Shell;
use super::{Connector, Error, ItemRef};
use crate::snapshot::{Item, ticket_ids};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// `ABC-123`, as the item's key or its `linear` asks carry it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TicketId(pub String);

impl ItemRef for TicketId {
    fn of(item: &Item) -> Vec<TicketId> {
        let mut ids: Vec<TicketId> = Vec::new();
        let texts = std::iter::once(item.key.clone()).chain(item.asks_of("linear"));
        for id in texts.flat_map(|text| ticket_ids(&text)).map(TicketId) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids
    }
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum StateKind {
    Triage,
    Backlog,
    Unstarted,
    Started,
    Completed,
    Canceled,
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct State {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: StateKind,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub name: String,
    pub is_me: bool,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    /// ISO time; it sorts as text.
    pub created_at: String,
    /// None for an integration's comment.
    pub user: Option<User>,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
struct Issue {
    state: State,
    assignee: Option<User>,
    comments: Nodes<Comment>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ticket {
    pub state: State,
    /// Someone else; None when it is yours or nobody's.
    pub assignee: Option<String>,
    /// The newest comment written by a person.
    pub last_comment: Option<(User, String)>,
}

impl Ticket {
    pub fn closed(&self) -> bool {
        matches!(self.state.kind, StateKind::Completed | StateKind::Canceled)
    }
}

impl From<Issue> for Ticket {
    fn from(issue: Issue) -> Ticket {
        Ticket {
            state: issue.state,
            assignee: issue
                .assignee
                .filter(|user| !user.is_me)
                .map(|user| user.name),
            last_comment: issue
                .comments
                .nodes
                .into_iter()
                .filter_map(|comment| Some((comment.user?, comment.created_at)))
                .max_by(|a, b| a.1.cmp(&b.1)),
        }
    }
}

#[derive(Serialize)]
struct Query {
    query: String,
}

impl Query {
    /// One aliased `issue` per ticket: `t0`, `t1`… in the order asked.
    fn for_tickets(ids: &[TicketId]) -> Query {
        let fields = "state { name type } assignee { name isMe } \
                      comments(first: 50) { nodes { createdAt user { name isMe } } }";
        let parts: Vec<String> = ids
            .iter()
            .enumerate()
            .map(|(index, TicketId(id))| format!("t{index}: issue(id: \"{id}\") {{ {fields} }}"))
            .collect();
        Query {
            query: format!("query {{ {} }}", parts.join(" ")),
        }
    }
}

#[derive(Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Deserialize)]
struct Answer {
    /// An unknown ticket comes back as a null alias, with an error beside it.
    #[serde(default)]
    data: Option<HashMap<String, Option<Issue>>>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

impl Answer {
    fn tickets(self, ids: &[TicketId]) -> Result<HashMap<TicketId, Ticket>, Error> {
        let Some(mut data) = self.data else {
            let message = self.errors.into_iter().next().map(|error| error.message);
            return Err(Error::Api(message.unwrap_or_else(|| "no data".into())));
        };
        Ok(ids
            .iter()
            .enumerate()
            .filter_map(|(index, id)| {
                let issue = data.remove(&format!("t{index}")).flatten()?;
                Some((id.clone(), Ticket::from(issue)))
            })
            .collect())
    }
}

pub struct Linear<S: Shell> {
    shell: S,
    url: String,
}

impl<S: Shell> Linear<S> {
    pub fn new(shell: S, url: &str) -> Linear<S> {
        Linear {
            shell,
            url: url.into(),
        }
    }
}

impl<S: Shell> Connector for Linear<S> {
    type Ref = TicketId;
    type Status = Ticket;
    const NAME: &'static str = "linear";

    fn fetch(&self, ids: &[TicketId]) -> Result<HashMap<TicketId, Ticket>, Error> {
        let token = http::token(&self.shell, Self::NAME)?;
        let answer: Answer = Http::new(&self.shell).post(
            &self.url,
            &token,
            Scheme::Raw,
            &Query::for_tickets(ids),
        )?;
        answer.tickets(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::super::shell::fake::Fake;
    use super::*;
    use serde_json::json;

    const KEYCHAIN: &str = "deck-linear";

    fn ids(names: &[&str]) -> Vec<TicketId> {
        names
            .iter()
            .map(|name| TicketId(name.to_string()))
            .collect()
    }

    #[test]
    fn state_assignee_and_newest_human_comment() {
        let answer = json!({ "data": {
            "t0": {
                "state": { "name": "In Progress", "type": "started" },
                "assignee": { "name": "Ann", "isMe": true },
                "comments": { "nodes": [
                    { "createdAt": "2026-11-25T09:00:00.000Z", "user": { "name": "Ann", "isMe": true } },
                    { "createdAt": "2026-11-25T10:12:00.000Z", "user": { "name": "Bob", "isMe": false } },
                    { "createdAt": "2026-11-25T11:00:00.000Z", "user": null }
                ] }
            },
            "t1": null
        }, "errors": [{ "message": "Entity not found" }] });
        let fake = Fake::default()
            .answer(KEYCHAIN, "lin_api_test")
            .answer("api.linear.app", &answer.to_string());
        let found = Linear::new(&fake, "https://api.linear.app/graphql")
            .fetch(&ids(&["ACME-1", "ACME-2"]))
            .unwrap();
        assert_eq!(found.len(), 1);
        let ticket = &found[&TicketId("ACME-1".into())];
        assert_eq!(ticket.state.name, "In Progress");
        assert_eq!(ticket.assignee, None);
        assert!(!ticket.closed());
        let (user, at) = ticket.last_comment.as_ref().unwrap();
        assert_eq!(
            (user.name.as_str(), user.is_me, at.as_str()),
            ("Bob", false, "2026-11-25T10:12:00.000Z")
        );
        let call = fake
            .calls
            .borrow()
            .iter()
            .find(|call| call.program == "curl")
            .cloned()
            .unwrap();
        assert!(call.line().contains(r#"t1: issue(id: \"ACME-2\")"#));
        assert_eq!(
            call.stdin.as_deref(),
            Some("Authorization: lin_api_test\nContent-Type: application/json")
        );
    }

    #[test]
    fn someone_elses_closed_ticket() {
        let answer = json!({ "data": { "t0": {
            "state": { "name": "Done", "type": "completed" },
            "assignee": { "name": "Bob", "isMe": false },
            "comments": { "nodes": [] }
        }}});
        let fake = Fake::default()
            .answer(KEYCHAIN, "k")
            .answer("curl", &answer.to_string());
        let ticket = Linear::new(&fake, "https://api.linear.app/graphql")
            .fetch(&ids(&["ACME-3"]))
            .unwrap()[&TicketId("ACME-3".into())]
            .clone();
        assert!(ticket.closed());
        assert_eq!(ticket.assignee.as_deref(), Some("Bob"));
        assert_eq!(ticket.last_comment, None);
    }

    #[test]
    fn unknown_state_type_is_kept() {
        let state: State =
            serde_json::from_value(json!({ "name": "Review", "type": "inReview" })).unwrap();
        assert_eq!(state.kind, StateKind::Other);
    }

    #[test]
    fn whole_call_refused() {
        let fake = Fake::default().answer(KEYCHAIN, "k").answer(
            "curl",
            r#"{"errors": [{"message": "Authentication required"}]}"#,
        );
        assert_eq!(
            Linear::new(&fake, "https://api.linear.app/graphql").fetch(&ids(&["ACME-1"])),
            Err(Error::Api("Authentication required".into()))
        );
    }

    #[test]
    fn no_token_no_call() {
        let fake = Fake::default();
        assert_eq!(
            Linear::new(&fake, "https://api.linear.app/graphql").fetch(&ids(&["ACME-1"])),
            Err(Error::NotConnected("linear"))
        );
        assert!(!fake.ran("curl"));
    }

    #[test]
    fn ids_from_key_and_asks_once() {
        let item: Item = serde_json::from_value(json!({
            "key": "ACME-1",
            "asks": { "linear": ["ACME-1 comment x", "ACME-7"], "slack": "C1 1.1" }
        }))
        .unwrap();
        assert_eq!(TicketId::of(&item), ids(&["ACME-1", "ACME-7"]));
    }
}
