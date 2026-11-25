//! User settings, in ~/.config/deck/config.toml. The settings popup writes
//! the same file, so hand edits and popup edits mix freely.

use crate::keymap::{Action, Chord};
use crate::ping::Ping;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

/// Where this app keeps its own files: job logs, the draft being edited.
pub fn cache_dir() -> PathBuf {
    home().join(".cache/deck")
}

/// Where the daily-todo skill keeps its snapshot; the agenda sits next to it.
pub fn todo_dir() -> PathBuf {
    home().join(".cache/daily-todo")
}

/// How far back the first refresh of the day looks for asks. Later refreshes
/// of the same day only look at what came in since the previous one.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub enum Depth {
    #[serde(rename = "today")]
    Today,
    #[serde(rename = "day")]
    Day,
    #[serde(rename = "2d")]
    TwoDays,
    #[serde(rename = "week")]
    Week,
}

impl Depth {
    pub const ALL: [Depth; 4] = [Depth::Today, Depth::Day, Depth::TwoDays, Depth::Week];

    pub fn label(self) -> &'static str {
        match self {
            Depth::Today => "today",
            Depth::Day => "day",
            Depth::TwoDays => "2 days",
            Depth::Week => "week",
        }
    }

    /// The window sentence handed to the skill; None keeps the skill's own.
    pub fn window(self) -> Option<&'static str> {
        match self {
            Depth::Today => Some("since 00:00 today, local time"),
            Depth::Day => None,
            Depth::TwoDays => {
                Some("since the start of the working day two working days ago, local time")
            }
            Depth::Week => Some("over the last 7 days"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Person {
    pub name: String,
    /// Calendar address. Empty: the agenda job looks it up by name and the
    /// app saves what it found here.
    #[serde(default)]
    pub email: String,
}

/// URL prefixes used to rebuild links from the ids the todo skill stores.
/// An empty one turns that kind of link off.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct LinkBases {
    /// e.g. https://linear.app/<workspace>/issue/
    pub linear: String,
    /// e.g. https://<workspace>.slack.com/archives/
    pub slack: String,
    /// e.g. https://mail.google.com/mail/u/<you@domain>/#all/
    pub gmail: String,
    /// Where a `team_inbox` ask opens.
    pub team_inbox: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// The Claude skill that builds the todo and writes the snapshot.
    pub skill: String,
    /// 0 turns the automatic refresh off; r still refreshes by hand.
    pub refresh_minutes: u32,
    pub depth: Depth,
    /// Automatic refreshes only run from the first hour to before the second.
    pub active_hours: [u32; 2],
    pub weekdays_only: bool,
    /// Model of the todo refresh. Empty: Claude Code's default.
    pub model: String,
    /// The MCP tools the agenda job may call to read calendars.
    pub calendar_tools: Vec<String>,
    /// Model of the short jobs: agenda, ticket draft, ticket creation.
    pub small_model: String,
    pub claude_bin: String,
    /// How often the app asks Linear, Slack and GitHub itself; 0 turns it off.
    pub status_minutes: u32,
    /// How a Claude session on an item that starts waiting on you is announced.
    pub ping: Ping,
    /// Ticket ids are `<prefix>-<number>`; empty takes any uppercase prefix.
    pub ticket_prefixes: Vec<String>,
    /// Claude runs from here, so that folder's CLAUDE.md and memory apply.
    pub workdir: String,
    /// Opens the ticket draft. Empty: $VISUAL, $EDITOR, then vi.
    pub editor: String,
    /// Opens links. Empty: `open` on macOS, `xdg-open` elsewhere.
    pub opener: String,
    /// Slack and Linear links open in their desktop apps, not the browser.
    pub desktop_apps: bool,
    /// First person is you: your own calendar, not a coworker's.
    pub people: Vec<Person>,
    pub links: LinkBases,
    /// The APIs the connectors call.
    pub endpoints: crate::connectors::Endpoints,
    /// Rebound actions, e.g. `work = ["w", "ctrl+w"]`; the others keep their defaults.
    pub keys: BTreeMap<Action, Vec<Chord>>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            skill: "daily-todo".into(),
            refresh_minutes: 30,
            depth: Depth::Day,
            active_hours: [8, 20],
            weekdays_only: true,
            model: String::new(),
            small_model: "sonnet".into(),
            calendar_tools: Vec::new(),
            claude_bin: "claude".into(),
            status_minutes: 2,
            ping: Ping::Notify,
            ticket_prefixes: Vec::new(),
            workdir: "~/Projects".into(),
            editor: String::new(),
            opener: String::new(),
            desktop_apps: true,
            people: vec![Person {
                name: "Me".into(),
                email: String::new(),
            }],
            links: LinkBases::default(),
            endpoints: crate::connectors::Endpoints::default(),
            keys: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        home().join(".config/deck/config.toml")
    }

    /// Reads the file, or writes the defaults there on the first start so
    /// they can be edited by hand.
    pub fn load() -> Config {
        match std::fs::read_to_string(Self::path()) {
            Ok(text) => toml::from_str(&text).unwrap_or_default(),
            Err(_) => {
                let config = Config::default();
                let _ = config.save();
                config
            }
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }

    pub fn workdir(&self) -> PathBuf {
        match self.workdir.strip_prefix("~/") {
            Some(rest) => home().join(rest),
            None => PathBuf::from(&self.workdir),
        }
    }

    /// The command that opens `url`: the configured opener as is, else the
    /// system's, through the Slack or Linear desktop app when asked.
    /// Opens the link with `open_command`, detached from this terminal.
    pub fn open(&self, url: &str) -> std::io::Result<()> {
        let command = self.open_command(url);
        std::process::Command::new(&command[0])
            .args(&command[1..])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(drop)
    }

    pub fn open_command(&self, url: &str) -> Vec<String> {
        let opener = self.opener();
        let mut command: Vec<String> = opener.split_whitespace().map(String::from).collect();
        if self.opener.is_empty() && self.desktop_apps {
            if let Some(rest) = url.strip_prefix("https://linear.app/") {
                command.push(format!("linear://{rest}"));
                return command;
            }
            if cfg!(target_os = "macos") && url.contains(".slack.com/") {
                command.extend(["-a".into(), "Slack".into()]);
            }
        }
        command.push(url.into());
        command
    }

    pub fn opener(&self) -> String {
        match (self.opener.is_empty(), cfg!(target_os = "macos")) {
            (false, _) => self.opener.clone(),
            (true, true) => "open".into(),
            (true, false) => "xdg-open".into(),
        }
    }

    pub fn editor(&self) -> String {
        [
            self.editor.clone(),
            std::env::var("VISUAL").unwrap_or_default(),
            std::env::var("EDITOR").unwrap_or_default(),
        ]
        .into_iter()
        .find(|editor| !editor.is_empty())
        .unwrap_or_else(|| "vi".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slack_and_linear_open_in_their_apps() {
        let config = Config::default();
        assert_eq!(
            config
                .open_command("https://linear.app/acme/issue/ACME-1")
                .last()
                .map(String::as_str),
            Some("linear://acme/issue/ACME-1")
        );
        let slack = config.open_command("https://acme.slack.com/archives/C1/p1");
        assert_eq!(
            slack.last().map(String::as_str),
            Some("https://acme.slack.com/archives/C1/p1")
        );
        assert_eq!(
            slack.contains(&"Slack".to_string()),
            cfg!(target_os = "macos")
        );
        let browser = Config {
            desktop_apps: false,
            ..Config::default()
        };
        assert_eq!(
            browser
                .open_command("https://linear.app/x")
                .last()
                .map(String::as_str),
            Some("https://linear.app/x")
        );
        let own = Config {
            opener: "my-open --new".into(),
            ..Config::default()
        };
        assert_eq!(
            own.open_command("https://linear.app/x"),
            vec!["my-open", "--new", "https://linear.app/x"]
        );
    }
}
