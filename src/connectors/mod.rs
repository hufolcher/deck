//! The app's own view of each item, beside the skill's: what the tracker, the
//! chat and the code host say now, and which Claude sessions work on it.
//!
//! A `Connector` answers for one kind of reference an item carries (`ItemRef`);
//! `poll` asks it about every item at once. Everything runs through a `Shell`,
//! so the whole path is tested with `Fake` and no network.

pub mod claude;
pub mod github;
pub mod http;
pub mod linear;
pub mod shell;
pub mod slack;

use crate::snapshot::Item;
use shell::Shell;
use std::collections::HashMap;
use std::hash::Hash;

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// No token for this connector: README, "Connectors", says how to add one.
    NotConnected(&'static str),
    /// A program could not run or failed: its name and why.
    Program(&'static str, String),
    /// The service answered with an error.
    Api(String),
    /// The answer did not have the expected shape.
    Decode(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::NotConnected(name) => write!(formatter, "{name} not connected"),
            Error::Program(program, reason) => write!(formatter, "{program}: {reason}"),
            Error::Api(reason) => formatter.write_str(reason),
            Error::Decode(reason) => write!(formatter, "unexpected answer: {reason}"),
        }
    }
}

/// A reference to something outside, as an item carries it.
pub trait ItemRef: Clone + Eq + Hash {
    fn of(item: &Item) -> Vec<Self>;
}

pub trait Connector {
    type Ref: ItemRef;
    type Status;
    const NAME: &'static str;

    /// What is known of each reference; a reference that cannot be found is
    /// left out, a failure of the whole service is an error.
    fn fetch(&self, refs: &[Self::Ref]) -> Result<HashMap<Self::Ref, Self::Status>, Error>;
}

/// One connector's answer for the whole list.
#[derive(Clone, Debug)]
pub struct Report<R, S> {
    pub found: HashMap<R, S>,
    pub error: Option<Error>,
}

impl<R: Eq + Hash, S: PartialEq> PartialEq for Report<R, S> {
    fn eq(&self, other: &Self) -> bool {
        self.found == other.found && self.error == other.error
    }
}

impl<R, S> Default for Report<R, S> {
    fn default() -> Self {
        Report {
            found: HashMap::new(),
            error: None,
        }
    }
}

impl<R: ItemRef, S> Report<R, S> {
    /// This item's references with what is known of them, in the item's order.
    pub fn for_item(&self, item: &Item) -> Vec<(R, &S)> {
        R::of(item)
            .into_iter()
            .filter_map(|reference| {
                let status = self.found.get(&reference)?;
                Some((reference, status))
            })
            .collect()
    }

    /// The error, but only for an item that has references it would cover.
    pub fn error_for(&self, item: &Item) -> Option<&Error> {
        self.error.as_ref().filter(|_| !R::of(item).is_empty())
    }
}

pub fn poll<C: Connector>(connector: &C, items: &[Item]) -> Report<C::Ref, C::Status> {
    let mut refs: Vec<C::Ref> = Vec::new();
    for reference in items.iter().flat_map(C::Ref::of) {
        if !refs.contains(&reference) {
            refs.push(reference);
        }
    }
    if refs.is_empty() {
        return Report::default();
    }
    match connector.fetch(&refs) {
        Ok(found) => Report { found, error: None },
        Err(error) => Report {
            found: HashMap::new(),
            error: Some(error),
        },
    }
}

pub type Tickets = Report<linear::TicketId, linear::Ticket>;
pub type Threads = Report<slack::Ask, slack::Thread>;
pub type Pulls = Report<github::PullRef, github::Pull>;

/// Everything the slow poll learnt, sent to the app in one message.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Statuses {
    pub tickets: Tickets,
    pub threads: Threads,
    pub pulls: Pulls,
}

/// The slow poll: tracker, chat and code host, one after the other.
/// Where each API lives: the public services by default, a GitHub Enterprise
/// server, a proxy, or a local emulator.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Endpoints {
    pub linear: String,
    pub slack: String,
    pub github: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            linear: "https://api.linear.app/graphql".into(),
            slack: "https://slack.com/api".into(),
            github: "https://api.github.com".into(),
        }
    }
}

pub fn poll_all<S: Shell>(shell: S, endpoints: &Endpoints, items: &[Item]) -> Statuses {
    Statuses {
        tickets: poll(&linear::Linear::new(&shell, &endpoints.linear), items),
        threads: poll(&slack::Slack::new(&shell, &endpoints.slack), items),
        pulls: poll(&github::GitHub::new(&shell, &endpoints.github), items),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Clone, PartialEq, Eq, Hash, Debug)]
    struct Word(String);

    impl ItemRef for Word {
        fn of(item: &Item) -> Vec<Word> {
            item.line
                .split_whitespace()
                .map(|word| Word(word.into()))
                .collect()
        }
    }

    struct Lengths {
        fail: bool,
        asked: std::cell::RefCell<Vec<usize>>,
    }

    impl Connector for Lengths {
        type Ref = Word;
        type Status = usize;
        const NAME: &'static str = "lengths";

        fn fetch(&self, refs: &[Word]) -> Result<HashMap<Word, usize>, Error> {
            self.asked.borrow_mut().push(refs.len());
            if self.fail {
                return Err(Error::Api("down".into()));
            }
            Ok(refs
                .iter()
                .map(|word| (word.clone(), word.0.len()))
                .collect())
        }
    }

    fn item(line: &str) -> Item {
        serde_json::from_value(json!({ "key": "k", "line": line })).unwrap()
    }

    #[test]
    fn one_call_for_all_items_without_duplicates() {
        let connector = Lengths {
            fail: false,
            asked: Default::default(),
        };
        let items = [item("ab cde"), item("cde f")];
        let report = poll(&connector, &items);
        assert_eq!(*connector.asked.borrow(), vec![3]);
        assert_eq!(
            report.for_item(&items[1]),
            vec![(Word("cde".into()), &3), (Word("f".into()), &1)]
        );
        assert_eq!(report.error_for(&items[0]), None);
    }

    #[test]
    fn no_reference_no_call() {
        let connector = Lengths {
            fail: true,
            asked: Default::default(),
        };
        let report = poll(&connector, &[item("")]);
        assert!(connector.asked.borrow().is_empty());
        assert_eq!(report, Report::default());
    }

    #[test]
    fn an_error_shows_only_where_it_matters() {
        let connector = Lengths {
            fail: true,
            asked: Default::default(),
        };
        let report = poll(&connector, &[item("ab")]);
        assert_eq!(
            report.error_for(&item("ab")),
            Some(&Error::Api("down".into()))
        );
        assert_eq!(report.error_for(&item("")), None);
        assert_eq!(Lengths::NAME, "lengths");
    }

    #[test]
    fn errors_read_well() {
        assert_eq!(
            Error::NotConnected("slack").to_string(),
            "slack not connected"
        );
        assert_eq!(
            Error::Program("curl", "not logged in".into()).to_string(),
            "curl: not logged in"
        );
    }
}
