//! The item's actions in one dialog, what ⏎ opens: each link, Claude, a ticket,
//! done or open again.
//! Arrows and ⏎, or the number, or a click; esc closes.

use crate::connectors::claude::Session;
use crate::snapshot::Item;
use crate::ui::status;
use chrono::{DateTime, Local};

#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    /// The item's link at this index.
    Open { index: usize, label: String },
    /// Start a background Claude on the item; its command goes to the clipboard.
    Work,
    /// Copy the command that opens the item's Claude session.
    TakeOver { state: String },
    /// Draft a Linear ticket for an item without one.
    Ticket,
    /// Close an item handled where the todo cannot see it (a mail, a call).
    Done,
    /// Open again an item closed by mistake.
    Reopen { done: String },
}

/// What a done item shows after its →, when the user closed it.
pub const DONE_BY_HAND: &str = "marked done";

impl Choice {
    pub fn label(&self) -> String {
        match self {
            Choice::Open { label, .. } => format!("Open {label}"),
            Choice::Work => "Let Claude work on it · copies the command to follow it".into(),
            Choice::TakeOver { state } => {
                format!("Copy the command to open Claude's session · {state}")
            }
            Choice::Ticket => "Draft a Linear ticket for it".into(),
            Choice::Done => "Mark it done · handled elsewhere".into(),
            Choice::Reopen { done } => format!("Reopen it · now {done}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub key: String,
    pub choices: Vec<Choice>,
    pub selected: usize,
}

impl Menu {
    pub fn for_item(item: &Item, session: Option<&Session>, now: DateTime<Local>) -> Menu {
        let mut choices: Vec<Choice> = item
            .all_links()
            .into_iter()
            .enumerate()
            .map(|(index, link)| Choice::Open {
                index,
                label: link.label,
            })
            .collect();
        choices.push(match session {
            Some(session) => Choice::TakeOver {
                state: status::session(session, now).text,
            },
            None => Choice::Work,
        });
        if !item.has_ticket() {
            choices.push(Choice::Ticket);
        }
        choices.push(match item.done_text() {
            Some(done) => Choice::Reopen { done },
            None => Choice::Done,
        });
        Menu {
            key: item.key.clone(),
            choices,
            selected: 0,
        }
    }

    pub fn step(&mut self, by: isize) {
        let count = self.choices.len() as isize;
        self.selected = (self.selected as isize + by).rem_euclid(count.max(1)) as usize;
    }

    pub fn chosen(&self) -> Option<&Choice> {
        self.choices.get(self.selected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::claude::{Kind, State};
    use serde_json::json;

    fn item(links: serde_json::Value, asks: serde_json::Value) -> Item {
        serde_json::from_value(json!({ "key": "k", "line": "l", "links": links, "asks": asks }))
            .unwrap()
    }

    #[test]
    fn links_then_claude_then_ticket() {
        let plain = item(
            json!([{ "label": "Mail", "url": "https://mail.test/1" }]),
            json!({}),
        );
        let menu = Menu::for_item(&plain, None, Local::now());
        assert_eq!(
            menu.choices,
            vec![
                Choice::Open {
                    index: 0,
                    label: "Mail".into()
                },
                Choice::Work,
                Choice::Ticket,
                Choice::Done
            ]
        );
        let session = Session {
            session_id: "s".into(),
            name: "k".into(),
            cwd: "/w".into(),
            state: State::Blocked,
            started_at: 0,
            kind: Kind::Background {
                short_id: "s".into(),
            },
        };
        let ticketed = item(
            json!([{ "label": "Linear ACME-1", "url": "https://linear.app/acme/issue/ACME-1" }]),
            json!({}),
        );
        let menu = Menu::for_item(&ticketed, Some(&session), Local::now());
        assert!(menu.choices.iter().any(
            |choice| matches!(choice, Choice::TakeOver { state } if state.contains("needs you"))
        ));
        assert!(!menu.choices.contains(&Choice::Ticket), "it has a ticket");
    }

    #[test]
    fn selection_wraps() {
        let mut menu = Menu::for_item(&item(json!([]), json!({})), None, Local::now());
        assert_eq!(menu.chosen(), Some(&Choice::Work));
        menu.step(-1);
        assert_eq!(menu.chosen(), Some(&Choice::Done));
        menu.step(1);
        assert_eq!(menu.selected, 0);
    }

    #[test]
    fn a_done_item_offers_to_reopen() {
        let done: Item = serde_json::from_value(
            json!({ "key": "k", "line": "l", "done": { "text": "answered Ann (mail)" } }),
        )
        .unwrap();
        let menu = Menu::for_item(&done, None, Local::now());
        assert_eq!(
            menu.choices.last(),
            Some(&Choice::Reopen {
                done: "answered Ann (mail)".into()
            })
        );
        assert!(!menu.choices.contains(&Choice::Done));
    }
}
