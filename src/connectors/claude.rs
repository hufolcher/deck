//! Claude Code sessions on this machine, matched to todo items: background ones
//! (`claude --bg`, listed by `claude agents --json --all`) and interactive ones,
//! with the tmux pane they run in. `w` starts one on an item, `c` takes it over.

use super::Error;
use super::shell::{Invocation, Shell};
use crate::snapshot::{Item, ticket_ids};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Progress {
    Working,
    Blocked,
    Done,
    Stopped,
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Activity {
    Busy,
    Idle,
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Entry {
    #[serde(rename_all = "camelCase")]
    Background {
        id: String,
        session_id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        cwd: String,
        #[serde(default)]
        started_at: i64,
        state: Option<Progress>,
        status: Option<Activity>,
    },
    #[serde(rename_all = "camelCase")]
    Interactive {
        pid: u32,
        session_id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        cwd: String,
        #[serde(default)]
        started_at: i64,
        status: Option<Activity>,
    },
    #[serde(other)]
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    /// Background, working.
    Working,
    /// Background, waiting on you: a question or a permission.
    Blocked,
    Done,
    Stopped,
    /// Interactive, a turn running.
    Busy,
    /// Interactive, waiting for a prompt.
    Idle,
}

impl State {
    pub fn active(self) -> bool {
        matches!(self, State::Working | State::Blocked | State::Busy)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// `claude attach <short id>` opens it.
    Background { short_id: String },
    /// Runs in a terminal of its own, in tmux at `pane` when it is.
    Interactive { pane: Option<String> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub session_id: String,
    pub name: String,
    pub cwd: PathBuf,
    pub state: State,
    /// Epoch milliseconds.
    pub started_at: i64,
    pub kind: Kind,
}

/// How to hand a session over to you: the command that opens it, for the
/// clipboard. Nothing opens by itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Handoff {
    /// `cd <cwd> && claude attach <id>`, or `… --resume <id>`.
    pub line: String,
    /// The tmux pane an interactive session already runs in.
    pub live_in: Option<String>,
}

impl Handoff {
    pub fn attach(claude: &str, short_id: &str, cwd: &Path) -> Handoff {
        Handoff {
            line: format!("cd {} && {claude} attach {short_id}", cwd.display()),
            live_in: None,
        }
    }

    /// A new conversation that starts from this one; the original goes on.
    pub fn fork(claude: &str, session_id: &str, cwd: &Path) -> Handoff {
        Handoff {
            line: format!(
                "cd {} && {claude} --resume {session_id} --fork-session",
                cwd.display()
            ),
            live_in: None,
        }
    }

    /// Copies the line; returns what happened, for the status line.
    pub fn copy<S: Shell>(&self, shell: &S) -> Result<String, Error> {
        let copy = if cfg!(target_os = "macos") {
            Invocation::new("pbcopy")
        } else {
            Invocation::new("sh").args(["-c", "wl-copy 2>/dev/null || xclip -selection clipboard"])
        };
        let output = shell
            .run(&copy.stdin(self.line.clone()))
            .map_err(|error| Error::Program("clipboard", error.to_string()))?;
        if !output.success {
            return Err(Error::Program("clipboard", output.reason()));
        }
        Ok(match &self.live_in {
            Some(pane) => format!("copied: {} (it also runs in tmux pane {pane})", self.line),
            None => format!("copied: {}", self.line),
        })
    }
}

impl Session {
    /// Whether the session is about this item: started for it by this app, or
    /// named after its key or one of its tickets.
    pub fn concerns(&self, item: &Item, started: &Started) -> bool {
        let short_id = self.session_id.split('-').next().unwrap_or("");
        if started.ids(&item.key).any(|id| id == short_id) {
            return true;
        }
        let mut keys = ticket_ids(&item.key);
        keys.push(item.key.clone());
        keys.extend(
            item.asks_of("linear")
                .iter()
                .flat_map(|text| ticket_ids(text)),
        );
        self.name
            .split(|ch: char| !(ch.is_alphanumeric() || ch == '-'))
            .any(|word| keys.iter().any(|key| key.eq_ignore_ascii_case(word)))
    }

    pub fn handoff(&self, claude: &str) -> Handoff {
        match &self.kind {
            Kind::Background { short_id } => Handoff::attach(claude, short_id, &self.cwd),
            Kind::Interactive { pane } => Handoff {
                line: format!(
                    "cd {} && {claude} --resume {}",
                    self.cwd.display(),
                    self.session_id
                ),
                live_in: pane.clone(),
            },
        }
    }
}

/// Sessions this app started, by item key; kept in the cache so a restart
/// still knows them.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Started {
    #[serde(flatten)]
    by_key: BTreeMap<String, Vec<String>>,
}

impl Started {
    pub fn path() -> PathBuf {
        crate::config::cache_dir().join("started.json")
    }

    pub fn load(path: &Path) -> Started {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }

    pub fn add(&mut self, key: &str, short_id: &str) {
        self.by_key
            .entry(key.to_string())
            .or_default()
            .push(short_id.to_string());
    }

    fn ids<'a>(&'a self, key: &str) -> impl Iterator<Item = &'a String> {
        self.by_key.get(key).into_iter().flatten()
    }
}

#[derive(Deserialize)]
struct SessionFile {
    tmux: Option<String>,
}

pub struct Claude<S: Shell> {
    shell: S,
    /// The `claude` program.
    pub bin: String,
    /// Where `~/.claude` is.
    home: PathBuf,
}

impl<S: Shell> Claude<S> {
    pub fn new(shell: S, bin: &str, home: PathBuf) -> Claude<S> {
        Claude {
            shell,
            bin: bin.to_string(),
            home,
        }
    }

    /// Every call: an API key in the shell must never turn a run into API billing.
    fn claude(&self) -> Invocation {
        Invocation::new(&self.bin)
            .unset("ANTHROPIC_API_KEY")
            .unset("ANTHROPIC_AUTH_TOKEN")
    }

    /// The tmux pane an interactive session reported in ~/.claude/sessions/<pid>.json.
    fn pane(&self, pid: u32) -> Option<String> {
        let path = self.home.join(format!(".claude/sessions/{pid}.json"));
        let file: SessionFile = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        file.tmux
    }

    fn session(&self, entry: Entry) -> Option<Session> {
        Some(match entry {
            Entry::Background {
                id,
                session_id,
                name,
                cwd,
                started_at,
                state,
                status,
            } => Session {
                state: match (state, status) {
                    (Some(Progress::Working), _)
                    | (None | Some(Progress::Other), Some(Activity::Busy)) => State::Working,
                    (Some(Progress::Blocked), _) => State::Blocked,
                    (Some(Progress::Stopped), _) => State::Stopped,
                    _ => State::Done,
                },
                session_id,
                name,
                cwd: PathBuf::from(cwd),
                started_at,
                kind: Kind::Background { short_id: id },
            },
            Entry::Interactive {
                pid,
                session_id,
                name,
                cwd,
                started_at,
                status,
            } => Session {
                state: if status == Some(Activity::Busy) {
                    State::Busy
                } else {
                    State::Idle
                },
                session_id,
                name,
                cwd: PathBuf::from(cwd),
                started_at,
                kind: Kind::Interactive {
                    pane: self.pane(pid),
                },
            },
            Entry::Other => return None,
        })
    }

    pub fn sessions(&self) -> Result<Vec<Session>, Error> {
        let listing = self.claude().args(["agents", "--json", "--all"]);
        let output = self
            .shell
            .run(&listing)
            .map_err(|error| Error::Program("claude", error.to_string()))?;
        let entries: Vec<Entry> = serde_json::from_str(&output.stdout)
            .map_err(|error| Error::Decode(error.to_string()))?;
        let mut sessions: Vec<Session> = entries
            .into_iter()
            .filter_map(|entry| self.session(entry))
            .collect();
        sessions.sort_by_key(|session| (!session.state.active(), -session.started_at));
        Ok(sessions)
    }

    /// Starts a background session on the item; returns its short id.
    pub fn start(&self, item: &Item, workdir: PathBuf, model: &str) -> Result<String, Error> {
        let mut run = self
            .claude()
            .arg("--bg")
            .args(["-n", &session_name(item)])
            .cwd(workdir);
        if !model.is_empty() {
            run = run.args(["--model", model]);
        }
        let output = self
            .shell
            .run(&run.arg(work_prompt(item)))
            .map_err(|error| Error::Program("claude", error.to_string()))?;
        started_id(&format!("{}\n{}", output.stdout, output.stderr))
            .ok_or_else(|| Error::Program("claude", output.reason()))
    }
}

/// The key first, so the session is found again by it.
pub fn session_name(item: &Item) -> String {
    let head = item.line.split(" · ").next().unwrap_or("");
    format!(
        "{} · {}",
        item.key,
        head.chars().take(50).collect::<String>()
    )
}

fn work_prompt(item: &Item) -> String {
    let links: Vec<String> = item
        .all_links()
        .iter()
        .map(|link| format!("- {}: {}", link.label, link.url))
        .collect();
    format!(
        "Work on this item of my todo.\n\n{}\nKey: {}\nSources: {}\n{}\n\n\
         Read its sources first, then do the work, with the matching skill if there is one. \
         Stop before anything outward (a post, comment, mail, status change, push, merge or \
         production write): prepare it and end with what is ready and what waits on my go.",
        item.line,
        item.key,
        serde_json::to_string(&item.asks).unwrap_or_default(),
        links.join("\n"),
    )
}

/// `claude --bg` prints `backgrounded · <id> · <name>`.
fn started_id(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("backgrounded · "))
        .and_then(|rest| rest.split(" · ").next())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::super::shell::fake::Fake;
    use super::*;
    use serde_json::json;

    fn item(key: &str, line: &str) -> Item {
        serde_json::from_value(json!({ "key": key, "line": line, "asks": { "linear": "ACME-9" } }))
            .unwrap()
    }

    /// A throwaway ~ with one interactive session's file.
    fn home_with_pane(pid: u32, pane: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!("deck-claude-{}-{pid}", std::process::id()));
        std::fs::create_dir_all(home.join(".claude/sessions")).unwrap();
        std::fs::write(
            home.join(format!(".claude/sessions/{pid}.json")),
            json!({ "tmux": pane }).to_string(),
        )
        .unwrap();
        home
    }

    fn listing() -> String {
        json!([
            { "kind": "background", "id": "0d802d74", "sessionId": "0d802d74-cd81", "name": "ACME-1 · answer",
              "cwd": "/w", "startedAt": 2, "state": "blocked" },
            { "kind": "background", "id": "11111111", "sessionId": "11111111-aa", "name": "old", "cwd": "/w",
              "startedAt": 9, "state": "done", "status": "idle" },
            { "kind": "interactive", "pid": 42, "sessionId": "a8b8-1", "name": "acme-9 review", "cwd": "/w",
              "startedAt": 1, "status": "busy" },
            { "kind": "remote", "sessionId": "x" }
        ])
        .to_string()
    }

    #[test]
    fn sessions_active_first_with_their_pane() {
        let fake = Fake::default().answer("claude agents --json --all", &listing());
        let claude = Claude::new(&fake, "claude", home_with_pane(42, "work:@3.%13"));
        let sessions = claude.sessions().unwrap();
        assert_eq!(
            fake.calls.borrow()[0].unset,
            vec!["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"]
        );
        let states: Vec<State> = sessions.iter().map(|session| session.state).collect();
        assert_eq!(states, vec![State::Blocked, State::Busy, State::Done]);
        assert_eq!(
            sessions[1].kind,
            Kind::Interactive {
                pane: Some("work:@3.%13".into())
            }
        );
        assert_eq!(
            sessions[1].handoff("claude").live_in.as_deref(),
            Some("work:@3.%13")
        );
        assert_eq!(
            sessions[0].handoff("claude").line,
            "cd /w && claude attach 0d802d74"
        );
    }

    #[test]
    fn matched_by_name_ticket_or_start() {
        let fake = Fake::default().answer("agents", &listing());
        let sessions = Claude::new(&fake, "claude", std::env::temp_dir())
            .sessions()
            .unwrap();
        let mut started = Started::default();
        let acme = item("ACME-1", "Answer");
        assert!(sessions[0].concerns(&acme, &started));
        // Named after the item's ticket in its asks, in another case.
        assert!(sessions.iter().any(|session| session.name == "acme-9 review" && session.concerns(&acme, &started)));
        let slug = item("feed-check", "Check the feed");
        let old = sessions
            .iter()
            .find(|session| session.name == "old")
            .unwrap();
        assert!(!old.concerns(&slug, &started));
        started.add("feed-check", "11111111");
        assert!(old.concerns(&slug, &started));
        assert_eq!(
            Session {
                kind: Kind::Interactive { pane: None },
                ..old.clone()
            }
            .handoff("claude"),
            Handoff {
                line: "cd /w && claude --resume 11111111-aa".into(),
                live_in: None
            }
        );
    }

    #[test]
    fn start_names_the_session_and_reads_its_id() {
        let fake = Fake::default().answer(
            "--bg",
            "backgrounded · 0d802d74 · ACME-1 · Answer\n  claude attach 0d802d74\n",
        );
        let claude = Claude::new(&fake, "claude", std::env::temp_dir());
        let id = claude
            .start(
                &item("ACME-1", "Answer · client waiting"),
                "/w".into(),
                "opus",
            )
            .unwrap();
        assert_eq!(id, "0d802d74");
        let call = fake.calls.borrow()[0].clone();
        assert_eq!(
            &call.args[..5],
            ["--bg", "-n", "ACME-1 · Answer", "--model", "opus"]
        );
        assert!(
            call.args[5].starts_with(
                "Work on this item of my todo.\n\nAnswer · client waiting\nKey: ACME-1"
            )
        );
        assert_eq!(call.cwd, Some(PathBuf::from("/w")));
        assert_eq!(
            call.unset,
            vec!["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"]
        );
        let refused = Fake::default().fail("--bg", "Not logged in · run claude login");
        let error = Claude::new(&refused, "claude", std::env::temp_dir()).start(
            &item("ACME-1", "x"),
            "/w".into(),
            "",
        );
        assert_eq!(
            error,
            Err(Error::Program(
                "claude",
                "Not logged in · run claude login".into()
            ))
        );
    }

    #[test]
    fn started_survives_a_restart() {
        let path = std::env::temp_dir().join(format!("deck-started-{}.json", std::process::id()));
        let mut started = Started::default();
        started.add("ACME-1", "0d802d74");
        started.save(&path).unwrap();
        assert_eq!(Started::load(&path), started);
        assert_eq!(
            Started::load(Path::new("/nonexistent/x.json")),
            Started::default()
        );
    }

    #[test]
    fn handoff_only_copies() {
        let fake = Fake::default().answer("pbcopy", "").answer("wl-copy", "");
        let attach = Handoff::attach("claude", "0d802d74", Path::new("/w"));
        assert_eq!(
            attach.copy(&fake),
            Ok("copied: cd /w && claude attach 0d802d74".into())
        );
        let calls = fake.calls.borrow().clone();
        assert_eq!(calls.len(), 1, "one clipboard call, nothing opened");
        assert_eq!(
            calls[0].stdin.as_deref(),
            Some("cd /w && claude attach 0d802d74")
        );
        assert!(!fake.ran("tmux"));
        let live = Handoff {
            line: "cd /w && claude --resume s1".into(),
            live_in: Some("work:@3.%13".into()),
        };
        assert!(
            live.copy(&fake)
                .unwrap()
                .ends_with("(it also runs in tmux pane work:@3.%13)")
        );
        assert_eq!(
            attach.copy(
                &Fake::default()
                    .fail("pbcopy", "no pasteboard")
                    .fail("sh", "no display")
            ),
            Err(Error::Program(
                "clipboard",
                if cfg!(target_os = "macos") {
                    "no pasteboard"
                } else {
                    "no display"
                }
                .into()
            ))
        );
    }
}
