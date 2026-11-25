//! Slack: who wrote last after each ask, with a user token (keychain item
//! `deck-slack`, from the app described in slack-app.yml).

use super::http::{self, Http, Scheme, Token};
use super::shell::Shell;
use super::{Connector, Error, ItemRef};
use crate::snapshot::Item;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::cell::RefCell;
use std::collections::HashMap;
use std::str::FromStr;

/// A message timestamp, `1795563621.575069`: seconds and micros, ordered as numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ts {
    pub seconds: u64,
    pub micros: u32,
}

impl FromStr for Ts {
    type Err = ();

    fn from_str(text: &str) -> Result<Ts, ()> {
        let (seconds, micros) = text.split_once('.').ok_or(())?;
        let digits = |part: &str| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit());
        if seconds.len() != 10 || !digits(seconds) || !digits(micros) {
            return Err(());
        }
        Ok(Ts {
            seconds: seconds.parse().map_err(|_| ())?,
            micros: micros.parse().map_err(|_| ())?,
        })
    }
}

impl std::fmt::Display for Ts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "{}.{:06}", self.seconds, self.micros)
    }
}

impl<'de> Deserialize<'de> for Ts {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Ts, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse()
            .map_err(|()| serde::de::Error::custom(format!("not a Slack ts: {text}")))
    }
}

/// An ask as the skill stores it: `<channel> [thread <thread ts>] <ask ts>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Ask {
    pub channel: String,
    /// The thread's first message; the ask itself outside a thread.
    pub thread: Ts,
    pub ask: Ts,
}

impl Ask {
    /// A direct message: an answer may come outside any thread.
    fn direct(&self) -> bool {
        self.channel.starts_with('D')
    }
}

impl FromStr for Ask {
    type Err = ();

    fn from_str(text: &str) -> Result<Ask, ()> {
        let channel = text
            .split_whitespace()
            .find(|token| {
                token.len() >= 9
                    && token.starts_with(['C', 'D', 'G'])
                    && token
                        .chars()
                        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
            })
            .ok_or(())?;
        let stamps: Vec<Ts> = text
            .split_whitespace()
            .filter_map(|token| token.parse().ok())
            .collect();
        Ok(Ask {
            channel: channel.to_string(),
            thread: *stamps.first().ok_or(())?,
            ask: *stamps.last().ok_or(())?,
        })
    }
}

impl ItemRef for Ask {
    fn of(item: &Item) -> Vec<Ask> {
        item.asks_of("slack")
            .iter()
            .filter_map(|text| text.parse().ok())
            .collect()
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Message {
    pub ts: Ts,
    pub user: Option<String>,
    pub bot_id: Option<String>,
    pub subtype: Option<String>,
}

impl Message {
    /// A person's message: not a bot's, not a join or a topic change.
    fn human(&self) -> bool {
        self.bot_id.is_none()
            && self.user.is_some()
            && self
                .subtype
                .as_deref()
                .is_none_or(|subtype| subtype == "thread_broadcast")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    /// A display name, or "you".
    pub by: String,
    pub mine: bool,
    pub ts: Ts,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    /// People's messages after the ask.
    pub replies: usize,
    pub last: Option<Reply>,
}

impl Thread {
    /// People's messages after the ask: how many, and the last one's author.
    fn after(ask: Ts, messages: &[Message]) -> (usize, Option<(String, Ts)>) {
        let mut humans: Vec<&Message> = messages
            .iter()
            .filter(|message| message.human() && message.ts > ask)
            .collect();
        humans.sort_by_key(|message| message.ts);
        humans.dedup_by_key(|message| message.ts);
        let last = humans
            .last()
            .and_then(|message| Some((message.user.clone()?, message.ts)));
        (humans.len(), last)
    }
}

/// Slack wraps every answer: `ok`, an `error` when not ok, the rest flattened.
#[derive(Deserialize)]
struct Answer<T> {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(flatten)]
    body: Option<T>,
}

#[derive(Deserialize)]
struct Identity {
    user_id: String,
}

#[derive(Deserialize)]
struct Messages {
    #[serde(default)]
    messages: Vec<Message>,
}

#[derive(Deserialize, Default)]
struct Profile {
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    real_name: String,
}

#[derive(Deserialize)]
struct Person {
    name: String,
    #[serde(default)]
    real_name: String,
    #[serde(default)]
    profile: Profile,
}

#[derive(Deserialize)]
struct PersonAnswer {
    user: Person,
}

impl Person {
    fn shown(self) -> String {
        [
            self.profile.display_name,
            self.profile.real_name,
            self.real_name,
            self.name,
        ]
        .into_iter()
        .find(|name| !name.is_empty())
        .unwrap_or_default()
    }
}

pub struct Slack<S: Shell> {
    shell: S,
    api: String,
    /// User id → shown name, kept for the life of the connector.
    names: RefCell<HashMap<String, String>>,
}

impl<S: Shell> Slack<S> {
    pub fn new(shell: S, api: &str) -> Slack<S> {
        Slack {
            shell,
            api: api.into(),
            names: RefCell::new(HashMap::new()),
        }
    }

    fn call<T: DeserializeOwned>(
        &self,
        token: &Token,
        method: &str,
        fields: &[(&str, &str)],
    ) -> Result<T, Error> {
        let answer: Answer<T> = Http::new(&self.shell).form(
            &format!("{}/{method}", self.api),
            token,
            Scheme::Bearer,
            fields,
        )?;
        match answer {
            Answer {
                ok: true,
                body: Some(body),
                ..
            } => Ok(body),
            Answer { error, .. } => Err(Error::Api(
                error.unwrap_or_else(|| format!("{method} failed")),
            )),
        }
    }

    fn name(&self, token: &Token, user: &str) -> String {
        if let Some(known) = self.names.borrow().get(user) {
            return known.clone();
        }
        let shown = self
            .call::<PersonAnswer>(token, "users.info", &[("user", user)])
            .map(|answer| answer.user.shown())
            .unwrap_or_else(|_| user.to_string());
        self.names
            .borrow_mut()
            .insert(user.to_string(), shown.clone());
        shown
    }

    fn thread(&self, token: &Token, me: &str, ask: &Ask) -> Result<Thread, Error> {
        let Messages { mut messages } = self.call(
            token,
            "conversations.replies",
            &[
                ("channel", &ask.channel),
                ("ts", &ask.thread.to_string()),
                ("limit", "100"),
            ],
        )?;
        if messages.len() <= 1 && ask.direct() {
            let Messages { messages: later } = self.call(
                token,
                "conversations.history",
                &[
                    ("channel", &ask.channel),
                    ("oldest", &ask.ask.to_string()),
                    ("limit", "20"),
                ],
            )?;
            messages.extend(later);
        }
        let (replies, last) = Thread::after(ask.ask, &messages);
        Ok(Thread {
            replies,
            last: last.map(|(user, ts)| Reply {
                mine: user == me,
                by: if user == me {
                    "you".into()
                } else {
                    self.name(token, &user)
                },
                ts,
            }),
        })
    }
}

impl<S: Shell> Connector for Slack<S> {
    type Ref = Ask;
    type Status = Thread;
    const NAME: &'static str = "slack";

    fn fetch(&self, asks: &[Ask]) -> Result<HashMap<Ask, Thread>, Error> {
        let token = http::token(&self.shell, Self::NAME)?;
        let Identity { user_id: me } = self.call(&token, "auth.test", &[])?;
        // One thread that cannot be read (left channel, deleted) is skipped.
        Ok(asks
            .iter()
            .filter_map(|ask| Some((ask.clone(), self.thread(&token, &me, ask).ok()?)))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::super::shell::fake::Fake;
    use super::*;
    use serde_json::json;

    fn ts(text: &str) -> Ts {
        text.parse().unwrap()
    }

    #[test]
    fn asks_in_a_thread_and_alone() {
        assert_eq!(
            "C08E7MKK6NS thread 1745334371.988719 1795563621.575069".parse(),
            Ok(Ask {
                channel: "C08E7MKK6NS".into(),
                thread: ts("1745334371.988719"),
                ask: ts("1795563621.575069"),
            })
        );
        let alone: Ask = "C08FCKQ76HL 1795519410.678459".parse().unwrap();
        assert_eq!(alone.thread, alone.ask);
        assert_eq!("#dev-chat 2026-11-03 12:25".parse::<Ask>(), Err(()));
        assert_eq!(ts("1795519410.000009").to_string(), "1795519410.000009");
        assert!(ts("1795519410.000010") > ts("1795519410.000009"));
    }

    fn slack_fake(replies: serde_json::Value) -> Fake {
        Fake::default()
            .answer("deck-slack", "xoxp-test")
            .answer("auth.test", r#"{"ok": true, "user_id": "UME"}"#)
            .answer("user=UBOB", r#"{"ok": true, "user": {"name": "bob", "real_name": "Bob B", "profile": {"display_name": ""}}}"#)
            .answer("conversations.replies", &replies.to_string())
    }

    #[test]
    fn last_person_after_the_ask_by_name() {
        let fake = slack_fake(json!({ "ok": true, "messages": [
            { "ts": "1700000000.000001", "user": "UANN" },
            { "ts": "1700000001.000001", "user": "UME" },
            { "ts": "1700000002.000001", "user": "UBOB" },
            { "ts": "1700000003.000001", "user": "UBOT", "bot_id": "B1" },
            { "ts": "1700000004.000001", "user": "UX", "subtype": "channel_join" }
        ]}));
        let ask: Ask = "C0123456789 thread 1700000000.000001 1700000000.000001"
            .parse()
            .unwrap();
        let found = Slack::new(&fake, "https://slack.com/api")
            .fetch(std::slice::from_ref(&ask))
            .unwrap();
        let thread = &found[&ask];
        assert_eq!(thread.replies, 2);
        assert_eq!(
            thread.last,
            Some(Reply {
                by: "Bob B".into(),
                mine: false,
                ts: ts("1700000002.000001")
            })
        );
        assert!(!fake.ran("conversations.history"));
    }

    #[test]
    fn my_own_answer_and_a_quiet_dm() {
        let fake = slack_fake(json!({ "ok": true, "messages": [
            { "ts": "1700000000.000001", "user": "UANN" }
        ]}))
        .answer(
            "conversations.history",
            r#"{"ok": true, "messages": [{"ts": "1700000009.000001", "user": "UME"}]}"#,
        );
        let ask: Ask = "D0123456789 1700000000.000001".parse().unwrap();
        let thread = &Slack::new(&fake, "https://slack.com/api")
            .fetch(std::slice::from_ref(&ask))
            .unwrap()[&ask];
        assert_eq!(
            thread
                .last
                .as_ref()
                .map(|reply| (reply.by.as_str(), reply.mine)),
            Some(("you", true))
        );
        assert!(
            fake.ran(
                "--data-urlencode channel=D0123456789 --data-urlencode oldest=1700000000.000001"
            )
        );
    }

    #[test]
    fn a_thread_that_cannot_be_read_is_skipped() {
        let fake = slack_fake(json!({ "ok": false, "error": "channel_not_found" }));
        let ask: Ask = "C0123456789 1700000000.000001".parse().unwrap();
        assert!(
            Slack::new(&fake, "https://slack.com/api")
                .fetch(&[ask])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn refused_token_is_the_whole_answer() {
        let fake = Fake::default()
            .answer("deck-slack", "xoxp-test")
            .answer("auth.test", r#"{"ok": false, "error": "invalid_auth"}"#);
        let ask: Ask = "C0123456789 1700000000.000001".parse().unwrap();
        assert_eq!(
            Slack::new(&fake, "https://slack.com/api").fetch(&[ask]),
            Err(Error::Api("invalid_auth".into()))
        );
    }

    #[test]
    fn asks_of_an_item() {
        let item: Item = serde_json::from_value(json!({
            "key": "x", "asks": { "slack": ["C0123456789 1700000000.000001", "#general yesterday"] }
        }))
        .unwrap();
        assert_eq!(Ask::of(&item).len(), 1);
    }
}
