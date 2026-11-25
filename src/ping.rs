//! A soft ping when a Claude session on a todo item starts waiting on you: a
//! desktop notification, a terminal bell, or nothing.

use crate::connectors::Error;
use crate::connectors::claude::{Session, State};
use crate::connectors::shell::{Invocation, Shell};
use serde::{Deserialize, Serialize};
use std::io::Write;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Ping {
    /// macOS Notification Center, or notify-send elsewhere.
    #[default]
    Notify,
    /// The terminal's bell; tmux shows it on the window.
    Bell,
    Off,
}

impl Ping {
    pub fn send<S: Shell>(self, shell: &S, text: &str) -> Result<(), Error> {
        let notification = match self {
            Ping::Off => return Ok(()),
            Ping::Bell => {
                let mut stdout = std::io::stdout();
                return stdout
                    .write_all(b"\x07")
                    .and_then(|()| stdout.flush())
                    .map_err(|error| Error::Program("bell", error.to_string()));
            }
            // The text goes as an argument: no quoting of the session's name.
            Ping::Notify if cfg!(target_os = "macos") => Invocation::new("osascript")
                .args(["-e", "on run argv", "-e"])
                .arg("display notification (item 1 of argv) with title \"deck\"")
                .args(["-e", "end run", text]),
            Ping::Notify => Invocation::new("notify-send").args(["deck", text]),
        };
        let program = if cfg!(target_os = "macos") {
            "osascript"
        } else {
            "notify-send"
        };
        let output = shell
            .run(&notification)
            .map_err(|error| Error::Program(program, error.to_string()))?;
        if output.success {
            Ok(())
        } else {
            Err(Error::Program(program, output.reason()))
        }
    }
}

/// Sessions waiting on you now that were not on the previous listing.
pub fn newly_blocked<'a>(before: &[Session], after: &'a [Session]) -> Vec<&'a Session> {
    after
        .iter()
        .filter(|session| session.state == State::Blocked)
        .filter(|session| {
            !before
                .iter()
                .any(|old| old.session_id == session.session_id && old.state == State::Blocked)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::claude::Kind;
    use crate::connectors::shell::fake::Fake;

    fn session(id: &str, state: State) -> Session {
        Session {
            session_id: id.into(),
            name: "ACME-1 · answer".into(),
            cwd: "/w".into(),
            state,
            started_at: 0,
            kind: Kind::Background {
                short_id: id.into(),
            },
        }
    }

    #[test]
    fn only_the_newly_waiting() {
        let before = [session("a", State::Working), session("b", State::Blocked)];
        let after = [
            session("a", State::Blocked),
            session("b", State::Blocked),
            session("c", State::Blocked),
            session("d", State::Working),
        ];
        let ids: Vec<&str> = newly_blocked(&before, &after)
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();
        assert_eq!(ids, vec!["a", "c"]);
    }

    #[test]
    fn notification_carries_the_text_as_an_argument() {
        let fake = Fake::default()
            .answer("osascript", "")
            .answer("notify-send", "");
        Ping::Notify
            .send(&fake, "Claude needs you on \"ACME-1\"")
            .unwrap();
        let call = fake.calls.borrow()[0].clone();
        assert_eq!(
            call.args.last().map(String::as_str),
            Some("Claude needs you on \"ACME-1\"")
        );
        Ping::Off.send(&fake, "x").unwrap();
        assert_eq!(fake.calls.borrow().len(), 1);
        let broken = Fake::default();
        assert!(Ping::Notify.send(&broken, "x").is_err());
    }
}
