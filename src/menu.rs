//! The item's actions in one dialog, what ⏎ opens: each link, Claude, a ticket.
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
}

impl Choice {
    pub fn label(&self) -> String {
        match self {
            Choice::Open { label, .. } => format!("Open {label}"),
            Choice::Work => "Let Claude work on it · copies the command to follow it".into(),
            Choice::TakeOver { state } => {
                format!("Copy the command to open Claude's session · {state}")
            }
            Choice::Ticket => "Draft a Linear ticket for it".into(),
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
                Choice::Ticket
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
        assert!(
            matches!(menu.choices.last(), Some(Choice::TakeOver { state }) if state.contains("needs you"))
        );
        assert!(!menu.choices.contains(&Choice::Ticket), "it has a ticket");
    }

    #[test]
    fn selection_wraps() {
        let mut menu = Menu::for_item(&item(json!([]), json!({})), None, Local::now());
        assert_eq!(menu.chosen(), Some(&Choice::Work));
        menu.step(-1);
        assert_eq!(menu.chosen(), Some(&Choice::Ticket));
        menu.step(1);
        assert_eq!(menu.selected, 0);
    }
}
