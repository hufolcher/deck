//! App state and everything that changes it: file reloads, the refresh
//! schedule, Claude job results, keys and clicks.

use crate::agenda::{self, Agenda, Event, PersonDay};
use crate::config::{Config, Depth, cache_dir, home};
use crate::connectors::claude::{Claude, Handoff, Session, Started};
use crate::connectors::shell::System;
use crate::connectors::{self, Statuses};
use crate::jobs::{self, Draft, Kind, Message};
use crate::keymap::{self, Action, Keymap};
use crate::menu::{self, Choice, Menu};
use crate::schedule::{Job, Schedule};
use crate::snapshot::{self, Item, Snapshot};
use chrono::{DateTime, Datelike, Duration, Local, Timelike, Weekday};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration as Every, Instant, SystemTime};

pub const REFRESH_CHOICES: [u32; 5] = [15, 30, 60, 120, 0];
pub const HOURS_CHOICES: [[u32; 2]; 4] = [[8, 20], [7, 21], [9, 19], [0, 24]];
pub const MODEL_CHOICES: [&str; 4] = ["", "opus", "sonnet", "haiku"];
pub const SETTINGS_FIELDS: usize = 6;
/// Claude sessions are listed locally and cheaply: often.
const SESSIONS_EVERY: Every = Every::from_secs(10);

/// A background poll: whether one runs, and when the last one started.
#[derive(Default)]
struct Poll {
    running: bool,
    last: Option<Instant>,
}

impl Poll {
    fn due(&self, every: Every) -> bool {
        !self.running && self.last.is_none_or(|last| last.elapsed() >= every)
    }

    fn start(&mut self) {
        self.running = true;
        self.last = Some(Instant::now());
    }

    /// The next tick starts one, whatever the interval.
    fn now(&mut self) {
        self.last = None;
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum View {
    Todo,
    Team,
}

/// The ticket popup, from the draft request to the created ticket.
pub enum Ticket {
    Drafting {
        key: String,
    },
    Ready {
        key: String,
        draft: Draft,
    },
    Creating {
        key: String,
        draft: Draft,
    },
    Failed {
        key: String,
        draft: Option<Draft>,
        error: String,
    },
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

/// Screen areas drawn in the last frame, so a click finds what it hit.
#[derive(Default)]
pub struct Hits {
    pub rows: Vec<(Rect, usize)>,
    /// The action dialog's rows.
    pub menu: Vec<(Rect, usize)>,
    pub tabs: Vec<(Rect, usize)>,
    pub links: Vec<(Rect, usize)>,
    /// The Claude session badge of the selected item.
    pub claude: Option<Rect>,
}

/// One column of the day: a person and what is known of their calendar.
pub struct Day {
    pub name: String,
    pub events: Vec<Event>,
    pub error: Option<String>,
    /// The events are an earlier read's, from this HH:MM.
    pub kept_from: Option<String>,
}

pub struct LastRun {
    pub at: DateTime<Local>,
    pub ok: bool,
    pub seconds: u64,
    pub error: String,
}

pub struct App {
    pub config: Config,
    pub snapshot: Snapshot,
    snapshot_mtime: Option<SystemTime>,
    /// The config is edited by hand, by the settings popup and by the MCP server.
    config_mtime: Option<SystemTime>,
    schedule_mtime: Option<SystemTime>,
    pub schedule: Schedule,
    pub agenda: Option<Agenda>,
    agenda_mtime: Option<SystemTime>,
    /// Item indexes in display order; `selected` indexes this.
    pub order: Vec<usize>,
    pub selected: usize,
    pub list_scroll: usize,
    pub person: usize,
    pub view: View,
    pub keymap: Keymap,
    /// The help panel is open; any key closes it.
    pub help: bool,
    /// The item's actions, open on ⏎.
    pub menu: Option<Menu>,
    /// Focused field while the settings popup is open.
    pub settings: Option<usize>,
    pub ticket: Option<Ticket>,
    pub running: HashMap<Kind, (Instant, u32)>,
    pub last: HashMap<Kind, LastRun>,
    last_refresh_start: Option<DateTime<Local>>,
    last_agenda_start: Option<DateTime<Local>>,
    pub toast: Option<Toast>,
    /// What the app's own connectors last said about the items.
    pub statuses: Statuses,
    pub sessions: Vec<Session>,
    pub sessions_error: Option<connectors::Error>,
    /// A first listing came in: later ones can tell what changed.
    sessions_listed: bool,
    /// Sessions this app started, by item key.
    pub started: Started,
    status_poll: Poll,
    sessions_poll: Poll,
    pub hits: Hits,
    pub quit: bool,
    /// Set by `e` in the ticket popup; the main loop leaves the screen to
    /// run the editor, which the app cannot do from inside a frame.
    pub edit_requested: bool,
    /// Key to select on the next snapshot reload, when an item changes key.
    follow: Option<String>,
    tx: Sender<Message>,
    rx: Receiver<Message>,
}

fn mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

impl App {
    pub fn new(config: Config) -> App {
        let (tx, rx) = channel();
        let (keymap, clashes) = Keymap::new(&config.keys, &keymap::tmux_taken(&System));
        let mut app = App {
            keymap,
            help: false,
            menu: None,
            config,
            snapshot: Snapshot::default(),
            snapshot_mtime: None,
            config_mtime: mtime(&Config::path()),
            schedule_mtime: None,
            schedule: Schedule::default(),
            agenda: None,
            agenda_mtime: None,
            order: Vec::new(),
            selected: 0,
            list_scroll: 0,
            person: 0,
            view: View::Todo,
            settings: None,
            ticket: None,
            running: HashMap::new(),
            last: HashMap::new(),
            last_refresh_start: None,
            last_agenda_start: None,
            toast: None,
            statuses: Statuses::default(),
            sessions: Vec::new(),
            sessions_error: None,
            sessions_listed: false,
            started: Started::load(&Started::path()),
            status_poll: Poll::default(),
            sessions_poll: Poll::default(),
            hits: Hits::default(),
            quit: false,
            edit_requested: false,
            follow: None,
            tx,
            rx,
        };
        app.reload_files();
        if !clashes.is_empty() {
            let text: Vec<String> = clashes.iter().map(ToString::to_string).collect();
            app.notify(format!("keys: {}", text.join("; ")), true);
        }
        app
    }

    pub fn item(&self) -> Option<&Item> {
        self.snapshot.items.get(*self.order.get(self.selected)?)
    }

    pub fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.toast = Some(Toast {
            text: text.into(),
            error,
            at: Instant::now(),
        });
    }

    /// Picks up a snapshot or agenda written by anyone: a job of this app,
    /// or a /daily-todo run in another Claude session.
    fn reload_files(&mut self) {
        let snapshot_mtime = mtime(&snapshot::path());
        if snapshot_mtime != self.snapshot_mtime {
            self.snapshot_mtime = snapshot_mtime;
            let selected_key = self
                .follow
                .take()
                .or_else(|| self.item().map(|item| item.key.clone()));
            self.snapshot = snapshot::load().unwrap_or_default();
            self.order = self.snapshot.display_order();
            self.selected = selected_key
                .and_then(|key| {
                    self.order
                        .iter()
                        .position(|&index| self.snapshot.items[index].key == key)
                })
                .unwrap_or(0)
                .min(self.order.len().saturating_sub(1));
        }
        let config_mtime = mtime(&Config::path());
        if config_mtime != self.config_mtime {
            self.config_mtime = config_mtime;
            self.config = Config::load();
            self.keymap = Keymap::new(&self.config.keys, &keymap::tmux_taken(&System)).0;
        }
        let schedule_mtime = mtime(&Schedule::path());
        if schedule_mtime != self.schedule_mtime {
            self.schedule_mtime = schedule_mtime;
            self.schedule = Schedule::load(&Schedule::path());
        }
        let agenda_mtime = mtime(&agenda::path());
        if agenda_mtime != self.agenda_mtime {
            self.agenda_mtime = agenda_mtime;
            self.agenda = agenda::load();
        }
    }

    /// Each configured person with today's events. Your own day falls back on
    /// the snapshot's meeting lines while the agenda is missing or old.
    pub fn days(&self) -> Vec<Day> {
        let today = Local::now().format("%Y-%m-%d").to_string();
        let agenda = self.agenda.as_ref().filter(|agenda| agenda.date == today);
        self.config
            .people
            .iter()
            .enumerate()
            .map(|(index, person)| {
                let known: Option<&PersonDay> = agenda
                    .and_then(|agenda| agenda.people.iter().find(|day| day.name == person.name));
                let unreadable =
                    known.is_none_or(|day| day.events.is_empty() && day.error.is_some());
                match known {
                    Some(day) if !unreadable => Day {
                        name: person.name.clone(),
                        events: day.events.clone(),
                        error: day.error.clone(),
                        kept_from: day.kept_from.clone(),
                    },
                    _ if index == 0 && !self.snapshot.meetings.is_empty() => Day {
                        name: person.name.clone(),
                        events: agenda::from_snapshot_meetings(&self.snapshot.meetings),
                        error: None,
                        kept_from: None,
                    },
                    Some(day) => Day {
                        name: person.name.clone(),
                        events: Vec::new(),
                        error: day.error.clone(),
                        kept_from: None,
                    },
                    None => Day {
                        name: person.name.clone(),
                        events: Vec::new(),
                        error: Some(
                            if self.running.contains_key(&Kind::Agenda) {
                                "loading…"
                            } else {
                                "no agenda yet"
                            }
                            .into(),
                        ),
                        kept_from: None,
                    },
                }
            })
            .collect()
    }

    /// How fresh the agenda is, for the day panels: updating now, or as of when.
    pub fn agenda_freshness(&self) -> (String, bool) {
        let as_of = self
            .agenda
            .as_ref()
            .and_then(|agenda| agenda.at.get(11..16))
            .map(|at| format!("as of {at}"));
        if self.running.contains_key(&Kind::Agenda) {
            (" ⟳ updating ".into(), true)
        } else {
            (
                as_of.map(|as_of| format!(" {as_of} ")).unwrap_or_default(),
                false,
            )
        }
    }

    pub fn active_at(&self, now: DateTime<Local>) -> bool {
        let weekend = matches!(now.weekday(), Weekday::Sat | Weekday::Sun);
        let [from, to] = self.config.active_hours;
        !(self.config.weekdays_only && weekend) && now.hour() >= from && now.hour() < to
    }

    /// When the next automatic todo refresh is due; None when it is off.
    pub fn next_refresh(&self) -> Option<DateTime<Local>> {
        if self.config.refresh_minutes == 0 {
            return None;
        }
        let snapshot_at = self.snapshot.at().map(|at| at.with_timezone(&Local));
        let base = match (self.last_refresh_start, snapshot_at) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        // No snapshot and no run yet: due already, not "now" taken a moment
        // after the caller's own now.
        Some(base.map_or(DateTime::<Local>::MIN_UTC.into(), |base| {
            base + Duration::minutes(self.config.refresh_minutes as i64)
        }))
    }

    fn agenda_due(&self, now: DateTime<Local>) -> bool {
        let today = now.format("%Y-%m-%d").to_string();
        if self
            .agenda
            .as_ref()
            .is_none_or(|agenda| agenda.date != today)
        {
            // One try per start-up when there is nothing for today, even off hours.
            return self.last_agenda_start.is_none();
        }
        let minutes = self.config.refresh_minutes.max(30) as i64;
        let base = self.last_agenda_start.or_else(|| {
            snapshot::parse_time(&self.agenda.as_ref()?.at).map(|at| at.with_timezone(&Local))
        });
        self.active_at(now) && base.is_none_or(|base| base + Duration::minutes(minutes) <= now)
    }

    pub fn tick(&mut self) {
        self.reload_files();
        while let Ok(message) = self.rx.try_recv() {
            self.on_message(message);
        }
        let now = Local::now();
        if !self.running.contains_key(&Kind::Refresh)
            && self.active_at(now)
            && self.next_refresh().is_some_and(|due| due <= now)
        {
            self.start_refresh();
        }
        if !self.running.contains_key(&Kind::Agenda) && self.agenda_due(now) {
            self.start_agenda();
        }
        let status_every = Every::from_secs(u64::from(self.config.status_minutes) * 60);
        if self.config.status_minutes > 0
            && !self.order.is_empty()
            && self.status_poll.due(status_every)
        {
            self.start_statuses();
        }
        if self.sessions_poll.due(SESSIONS_EVERY) {
            self.start_sessions();
        }
        self.run_planned(now);
        if self
            .toast
            .as_ref()
            .is_some_and(|toast| toast.at.elapsed().as_secs() > 8)
        {
            self.toast = None;
        }
    }

    pub fn start_refresh(&mut self) {
        if self.running.contains_key(&Kind::Refresh) {
            return;
        }
        self.last_refresh_start = Some(Local::now());
        self.running.insert(Kind::Refresh, (Instant::now(), 0));
        jobs::refresh(&self.config, self.tx.clone());
    }

    /// No calendar tools, no job: the day panels show the todo's meeting lines.
    pub fn start_agenda(&mut self) {
        if self.running.contains_key(&Kind::Agenda) || self.config.calendar_tools.is_empty() {
            return;
        }
        self.last_agenda_start = Some(Local::now());
        self.running.insert(Kind::Agenda, (Instant::now(), 0));
        jobs::agenda(
            &self.config,
            &Local::now().format("%Y-%m-%d").to_string(),
            self.tx.clone(),
        );
    }

    fn start_statuses(&mut self) {
        self.status_poll.start();
        let items = self.snapshot.items.clone();
        let endpoints = self.config.endpoints.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Message::Statuses(connectors::poll_all(
                System, &endpoints, &items,
            )));
        });
    }

    fn start_sessions(&mut self) {
        self.sessions_poll.start();
        let bin = self.config.claude_bin.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Message::Sessions(
                Claude::new(System, &bin, home()).sessions(),
            ));
        });
    }

    /// Runs what the schedule says is due, and writes back what is left.
    fn run_planned(&mut self, now: DateTime<Local>) {
        let due = self.schedule.take_due(now.fixed_offset());
        if due.is_empty() {
            return;
        }
        if let Err(error) = self.schedule.save(&Schedule::path()) {
            self.notify(format!("schedule not saved: {error}"), true);
        }
        self.schedule_mtime = mtime(&Schedule::path());
        for job in due {
            match job {
                Job::Refresh => {
                    self.start_refresh();
                    self.notify("planned refresh started", false);
                }
                Job::Work { key } => match self
                    .order
                    .iter()
                    .position(|&index| self.snapshot.items[index].key == key)
                {
                    Some(position) => {
                        self.selected = position;
                        self.start_work();
                    }
                    None => self.notify(
                        format!("planned work on {key}: no such item any more"),
                        true,
                    ),
                },
            }
        }
    }

    /// The sessions about this item, the active and newest first.
    pub fn sessions_for(&self, item: &Item) -> Vec<&Session> {
        self.sessions
            .iter()
            .filter(|session| session.concerns(item, &self.started))
            .collect()
    }

    /// `w`: a background Claude session works on the selected item.
    pub fn start_work(&mut self) {
        let Some(item) = self.item().cloned() else {
            return;
        };
        if self
            .sessions_for(&item)
            .iter()
            .any(|session| session.state.active())
        {
            self.notify(
                format!("Claude is already on {}: c takes it over", item.key),
                true,
            );
            return;
        }
        self.notify(format!("starting Claude on {}…", item.key), false);
        let (bin, workdir, model) = (
            self.config.claude_bin.clone(),
            self.config.workdir(),
            self.config.model.clone(),
        );
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let started = Claude::new(System, &bin, home()).start(&item, workdir, &model);
            let _ = tx.send(Message::Work {
                key: item.key,
                started,
            });
        });
    }

    /// `c` or a click on the badge: the item's session, in your hands.
    pub fn take_over(&mut self) {
        let Some(item) = self.item().cloned() else {
            return;
        };
        let Some(handoff) = self
            .sessions_for(&item)
            .first()
            .map(|session| session.handoff(&self.config.claude_bin))
        else {
            self.notify(
                format!("no Claude session on {}: w starts one", item.key),
                true,
            );
            return;
        };
        match handoff.copy(&System) {
            Ok(done) => self.notify(done, false),
            Err(error) => self.notify(format!("not copied: {error}"), true),
        }
    }

    /// A session on a todo item now waits on you: say so, softly.
    fn ping_waiting(&mut self, sessions: &[Session]) {
        let waiting: Vec<String> = crate::ping::newly_blocked(&self.sessions, sessions)
            .into_iter()
            .filter_map(|session| {
                let item = self
                    .snapshot
                    .items
                    .iter()
                    .find(|item| session.concerns(item, &self.started))?;
                Some(item.key.clone())
            })
            .collect();
        if waiting.is_empty() {
            return;
        }
        let text = format!("Claude needs you on {}", waiting.join(", "));
        if let Err(error) = self.config.ping.send(&System, &text) {
            self.notify(format!("{text} (ping failed: {error})"), true);
        } else {
            self.notify(text, true);
        }
    }

    fn on_work(&mut self, key: String, started: Result<String, connectors::Error>) {
        match started {
            Ok(id) => {
                self.started.add(&key, &id);
                if let Err(error) = self.started.save(&Started::path()) {
                    self.notify(
                        format!("Claude on {key} ({id}), not remembered: {error}"),
                        true,
                    );
                } else {
                    let handoff =
                        Handoff::attach(&self.config.claude_bin, &id, &self.config.workdir());
                    match handoff.copy(&System) {
                        Ok(copied) => {
                            self.notify(format!("Claude works on {key} · {copied}"), false)
                        }
                        Err(error) => self.notify(
                            format!("Claude works on {key} ({id}), not copied: {error}"),
                            true,
                        ),
                    }
                }
                self.sessions_poll.now();
            }
            Err(error) => self.notify(format!("Claude not started on {key}: {error}"), true),
        }
    }

    fn on_message(&mut self, message: Message) {
        let outcome = match message {
            Message::Started { kind, pid } => {
                self.running.entry(kind).or_insert((Instant::now(), pid)).1 = pid;
                return;
            }
            Message::Statuses(statuses) => {
                self.status_poll.running = false;
                self.statuses = statuses;
                return;
            }
            Message::Sessions(listed) => {
                self.sessions_poll.running = false;
                match listed {
                    Ok(sessions) => {
                        // The first listing is the baseline: no ping for what already waited.
                        if self.sessions_listed {
                            self.ping_waiting(&sessions);
                        }
                        self.sessions_listed = true;
                        self.sessions = sessions;
                        self.sessions_error = None;
                    }
                    Err(error) => self.sessions_error = Some(error),
                }
                return;
            }
            Message::Work { key, started } => return self.on_work(key, started),
            Message::Finished(outcome) => outcome,
        };
        self.running.remove(&outcome.kind);
        self.last.insert(
            outcome.kind,
            LastRun {
                at: Local::now(),
                ok: outcome.ok,
                seconds: outcome.seconds,
                error: outcome.error.clone(),
            },
        );
        match outcome.kind {
            Kind::Refresh if !outcome.ok => {
                self.notify(format!("todo refresh failed: {}", outcome.error), true)
            }
            Kind::Refresh => {}
            Kind::Agenda => match outcome.output.filter(|_| outcome.ok) {
                Some(output) => self.store_agenda(output),
                None => self.notify(format!("agenda failed: {}", outcome.error), true),
            },
            Kind::Draft => {
                let Some(Ticket::Drafting { key }) = self.ticket.take() else {
                    return;
                };
                let draft = outcome
                    .output
                    .and_then(|output| serde_json::from_value::<Draft>(output).ok());
                self.ticket = Some(match draft {
                    Some(draft) if outcome.ok => Ticket::Ready { key, draft },
                    _ => Ticket::Failed {
                        key,
                        draft: None,
                        error: outcome.error,
                    },
                });
            }
            Kind::Create => {
                let Some(Ticket::Creating { key, draft }) = self.ticket.take() else {
                    return;
                };
                let created = outcome.output.as_ref().and_then(|output| {
                    Some((
                        output.get("identifier")?.as_str()?.to_string(),
                        output.get("url")?.as_str()?.to_string(),
                    ))
                });
                match created {
                    Some((identifier, url)) if outcome.ok => {
                        match snapshot::attach_ticket(&key, &identifier, &url) {
                            Ok(()) => {
                                self.follow = Some(identifier.clone());
                                self.notify(format!("{identifier} created"), false)
                            }
                            Err(error) => self.notify(
                                format!("{identifier} created, snapshot not updated: {error}"),
                                true,
                            ),
                        }
                    }
                    _ => {
                        self.ticket = Some(Ticket::Failed {
                            key,
                            draft: Some(draft),
                            error: outcome.error,
                        })
                    }
                }
            }
        }
    }

    fn store_agenda(&mut self, output: Value) {
        let people: Vec<PersonDay> = output
            .get("people")
            .cloned()
            .and_then(|people| serde_json::from_value(people).ok())
            .unwrap_or_default();
        let now = Local::now();
        let date = now.format("%Y-%m-%d").to_string();
        // Nobody readable (connector signed out, say): say why, once.
        if !people.is_empty()
            && people
                .iter()
                .all(|day| day.events.is_empty() && day.error.is_some())
        {
            let reason = people[0].error.clone().unwrap_or_default();
            self.notify(format!("agenda: no calendar read: {reason}"), true);
        }
        let people = agenda::merge(self.agenda.as_ref(), &date, people);
        // Addresses found by name are kept, so the next run skips the lookup.
        let mut learned = false;
        for person in self
            .config
            .people
            .iter_mut()
            .skip(1)
            .filter(|person| person.email.is_empty())
        {
            if let Some(found) = people
                .iter()
                .find(|day| day.name == person.name && day.email.contains('@'))
            {
                person.email = found.email.clone();
                learned = true;
            }
        }
        if learned {
            let _ = self.config.save();
        }
        let agenda = Agenda {
            at: now.to_rfc3339(),
            date,
            people,
        };
        match agenda::save(&agenda) {
            Ok(()) => self.agenda_mtime = None,
            Err(error) => self.notify(format!("agenda not saved: {error}"), true),
        }
        self.agenda = Some(agenda);
    }

    pub fn open_link(&mut self, index: usize) {
        let Some(link) = self
            .item()
            .and_then(|item| item.all_links().into_iter().nth(index))
        else {
            self.notify("no link on this item", true);
            return;
        };
        match self.config.open(&link.url) {
            Ok(_) => self.notify(format!("opened {}", link.label), false),
            Err(error) => self.notify(format!("cannot open {}: {error}", link.url), true),
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let last = self.order.len().saturating_sub(1) as isize;
        let next = (self.selected as isize + delta).clamp(0, last.max(0)) as usize;
        if next != self.selected {
            self.selected = next;
        }
    }

    pub fn start_ticket(&mut self) {
        let Some(item) = self.item().cloned() else {
            return;
        };
        if item.has_ticket() {
            self.notify(format!("{} already has a ticket", item.key), true);
            return;
        }
        if self.running.contains_key(&Kind::Draft) || self.running.contains_key(&Kind::Create) {
            self.notify("a ticket is already being drafted or created", true);
            return;
        }
        self.ticket = Some(Ticket::Drafting {
            key: item.key.clone(),
        });
        self.running.insert(Kind::Draft, (Instant::now(), 0));
        jobs::draft(&self.config, &item, self.tx.clone());
    }

    fn confirm_ticket(&mut self) {
        let Some(Ticket::Ready { key, draft }) = self.ticket.take() else {
            return;
        };
        self.running.insert(Kind::Create, (Instant::now(), 0));
        jobs::create(&self.config, &draft, self.tx.clone());
        self.ticket = Some(Ticket::Creating { key, draft });
    }

    fn redraft(&mut self) {
        let key = match &self.ticket {
            Some(Ticket::Ready { key, .. }) | Some(Ticket::Failed { key, .. }) => key.clone(),
            _ => return,
        };
        let Some(index) = self
            .order
            .iter()
            .position(|&index| self.snapshot.items[index].key == key)
        else {
            self.ticket = None;
            return;
        };
        self.selected = index;
        self.ticket = None;
        self.start_ticket();
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.ticket.is_some() {
            return self.on_ticket_key(key.code);
        }
        if let Some(field) = self.settings {
            return self.on_settings_key(field, key.code);
        }
        if self.help {
            self.help = false;
            return;
        }
        if self.menu.is_some() {
            return self.on_menu_key(key.code);
        }
        let Some(action) = self.keymap.action(key) else {
            return;
        };
        match action {
            Action::Quit => self.quit = true,
            Action::Back => self.view = View::Todo,
            Action::Down => self.move_selection(1),
            Action::Up => self.move_selection(-1),
            Action::PageDown => self.move_selection(10),
            Action::PageUp => self.move_selection(-10),
            Action::First => self.move_selection(-(self.order.len() as isize)),
            Action::Last => self.move_selection(self.order.len() as isize),
            Action::Open => self.open_menu(),
            Action::NextPerson => self.person = (self.person + 1) % self.config.people.len().max(1),
            Action::PreviousPerson => {
                let count = self.config.people.len().max(1);
                self.person = (self.person + count - 1) % count;
            }
            Action::MyDay => self.person = 0,
            Action::Team => {
                self.view = if self.view == View::Team {
                    View::Todo
                } else {
                    View::Team
                }
            }
            Action::Refresh => {
                self.start_refresh();
                self.start_agenda();
                self.status_poll.now();
                self.notify("refreshing todo, agenda and statuses", false);
            }
            Action::Settings => self.settings = Some(0),
            Action::Help => self.help = true,
        }
    }

    pub fn open_menu(&mut self) {
        let Some(item) = self.item().cloned() else {
            return;
        };
        let session = self
            .sessions_for(&item)
            .first()
            .map(|session| (*session).clone());
        self.menu = Some(Menu::for_item(&item, session.as_ref(), Local::now()));
    }

    fn on_menu_key(&mut self, code: KeyCode) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        match code {
            KeyCode::Esc | KeyCode::Char('q') => self.menu = None,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => menu.step(1),
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => menu.step(-1),
            KeyCode::Enter => self.run_choice(None),
            KeyCode::Char(digit @ '1'..='9') => {
                self.run_choice(Some(digit as usize - '1' as usize))
            }
            _ => {}
        }
    }

    /// Runs the selected choice, or the one at `index`, and closes the dialog.
    pub fn run_choice(&mut self, index: Option<usize>) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        let choice = match index {
            Some(index) => menu.choices.get(index),
            None => menu.chosen(),
        };
        let Some(choice) = choice.cloned() else {
            self.menu = Some(menu);
            return;
        };
        match choice {
            Choice::Open { index, .. } => self.open_link(index),
            Choice::Work => self.start_work(),
            Choice::TakeOver { .. } => self.take_over(),
            Choice::Ticket => self.start_ticket(),
            Choice::Done => self.set_done(&menu.key, Some(menu::DONE_BY_HAND)),
            Choice::Reopen { .. } => self.set_done(&menu.key, None),
        }
    }

    /// Done with the user's own word for it, or open again with None.
    fn set_done(&mut self, key: &str, done: Option<&str>) {
        match snapshot::set_done(key, done) {
            Ok(()) => {
                self.follow = Some(key.to_string());
                self.reload_files();
                let text = if done.is_some() { "done" } else { "open again" };
                self.notify(format!("{key} {text}"), false);
            }
            Err(error) => self.notify(format!("not saved: {error}"), true),
        }
    }

    fn on_ticket_key(&mut self, code: KeyCode) {
        match (&self.ticket, code) {
            (Some(Ticket::Ready { .. }), KeyCode::Enter) => self.confirm_ticket(),
            (Some(Ticket::Ready { .. }), KeyCode::Char('e')) => self.edit_requested = true,
            (Some(Ticket::Ready { .. }) | Some(Ticket::Failed { .. }), KeyCode::Char('r')) => {
                self.redraft()
            }
            (Some(Ticket::Failed { draft: Some(_), .. }), KeyCode::Enter) => {
                if let Some(Ticket::Failed {
                    key,
                    draft: Some(draft),
                    ..
                }) = self.ticket.take()
                {
                    self.ticket = Some(Ticket::Ready { key, draft });
                }
            }
            // Creating cannot be cancelled: the ticket may already exist.
            (Some(Ticket::Creating { .. }), _) => {}
            (Some(Ticket::Drafting { .. }), KeyCode::Esc) => {
                self.kill(Kind::Draft);
                self.ticket = None;
            }
            (_, KeyCode::Esc | KeyCode::Char('q')) => self.ticket = None,
            _ => {}
        }
    }

    fn on_settings_key(&mut self, field: usize, code: KeyCode) {
        let step: isize = match code {
            KeyCode::Esc | KeyCode::Char('s') | KeyCode::Char('q') | KeyCode::Enter => {
                self.settings = None;
                return;
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.settings = Some((field + 1) % SETTINGS_FIELDS);
                return;
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                self.settings = Some((field + SETTINGS_FIELDS - 1) % SETTINGS_FIELDS);
                return;
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => 1,
            KeyCode::Left | KeyCode::Char('h') => -1,
            _ => return,
        };
        fn cycle<T: PartialEq + Copy>(choices: &[T], current: T, step: isize) -> T {
            let at = choices
                .iter()
                .position(|choice| *choice == current)
                .unwrap_or(0) as isize;
            choices[(at + step).rem_euclid(choices.len() as isize) as usize]
        }
        let config = &mut self.config;
        match field {
            0 => config.refresh_minutes = cycle(&REFRESH_CHOICES, config.refresh_minutes, step),
            1 => config.depth = cycle(&Depth::ALL, config.depth, step),
            2 => config.active_hours = cycle(&HOURS_CHOICES, config.active_hours, step),
            3 => config.weekdays_only = !config.weekdays_only,
            4 => config.model = cycle(&MODEL_CHOICES, config.model.as_str(), step).to_string(),
            _ => {
                config.small_model =
                    cycle(&MODEL_CHOICES[1..], config.small_model.as_str(), step).to_string()
            }
        }
        if let Err(error) = self.config.save() {
            self.notify(format!("settings not saved: {error}"), true);
        }
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        if self.ticket.is_some() || self.settings.is_some() || self.help {
            return;
        }
        if self.menu.is_some() {
            if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                let at = Position {
                    x: mouse.column,
                    y: mouse.row,
                };
                match self.hits.menu.iter().find(|(area, _)| area.contains(at)) {
                    Some(&(_, index)) => self.run_choice(Some(index)),
                    None => self.menu = None,
                }
            }
            return;
        }
        let at = Position {
            x: mouse.column,
            y: mouse.row,
        };
        match mouse.kind {
            MouseEventKind::ScrollDown => self.move_selection(1),
            MouseEventKind::ScrollUp => self.move_selection(-1),
            MouseEventKind::Down(MouseButton::Left) => {
                if self.hits.claude.is_some_and(|area| area.contains(at)) {
                    self.take_over();
                } else if let Some(&(_, link)) =
                    self.hits.links.iter().find(|(area, _)| area.contains(at))
                {
                    self.open_link(link);
                } else if let Some(&(_, person)) =
                    self.hits.tabs.iter().find(|(area, _)| area.contains(at))
                {
                    self.person = person;
                } else if let Some(&(_, row)) =
                    self.hits.rows.iter().find(|(area, _)| area.contains(at))
                {
                    // A click selects; a click on the selected row opens its actions.
                    if self.selected == row {
                        self.open_menu();
                    }
                    self.selected = row;
                }
            }
            _ => {}
        }
    }

    fn kill(&mut self, kind: Kind) {
        if let Some((_, pid)) = self.running.remove(&kind).filter(|(_, pid)| *pid != 0) {
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .status();
        }
    }

    /// Stops the jobs still running, except a ticket creation: cutting it
    /// could leave a ticket created but not linked, so it is left to finish.
    pub fn shutdown(&mut self) {
        for kind in [Kind::Refresh, Kind::Agenda, Kind::Draft] {
            self.kill(kind);
        }
    }

    /// Runs the editor on the draft; called by the main loop with the screen
    /// handed back to the terminal.
    pub fn edit_draft(&mut self) {
        self.edit_requested = false;
        let Some(Ticket::Ready { draft, .. }) = &mut self.ticket else {
            return;
        };
        let file = cache_dir().join("ticket-draft.md");
        let _ = std::fs::create_dir_all(cache_dir());
        if let Err(error) = std::fs::write(&file, draft_to_text(draft)) {
            self.notify(format!("cannot write the draft: {error}"), true);
            return;
        }
        let editor = self.config.editor();
        let mut parts = editor.split_whitespace();
        let program = parts.next().unwrap_or("vi");
        let status = std::process::Command::new(program)
            .args(parts)
            .arg(&file)
            .status();
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let Some(Ticket::Ready { draft, .. }) = &mut self.ticket else {
            return;
        };
        match status {
            Ok(_) => match text_to_draft(&text, draft) {
                Some(edited) => *draft = edited,
                None => {
                    self.ticket = None;
                    self.notify("empty title: ticket cancelled", false);
                }
            },
            Err(error) => self.notify(format!("cannot run {editor}: {error}"), true),
        }
    }
}

const DRAFT_HEADER: &str = "# Edit the ticket, save and quit. An empty title cancels it.\n\
# priority: 0 none, 1 urgent, 2 high, 3 medium, 4 low. Below --- is the description.\n";

pub fn draft_to_text(draft: &Draft) -> String {
    format!(
        "{DRAFT_HEADER}title: {}\nteam: {}\nstate: {}\npriority: {}\nproject: {}\n---\n{}\n",
        draft.title,
        draft.team,
        draft.state,
        draft.priority,
        draft.project.clone().unwrap_or_default(),
        draft.description
    )
}

/// None when the title was emptied, which cancels the ticket.
pub fn text_to_draft(text: &str, base: &Draft) -> Option<Draft> {
    let mut draft = base.clone();
    let (head, description) = text.split_once("\n---\n").unwrap_or((text, ""));
    draft.description = description.trim().to_string();
    for line in head.lines().filter(|line| !line.starts_with('#')) {
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().to_string();
        match field.trim() {
            "title" => draft.title = value,
            "team" => draft.team = value,
            "state" => draft.state = value,
            "priority" => draft.priority = value.parse().unwrap_or(draft.priority).min(4),
            "project" => draft.project = Some(value).filter(|project| !project.is_empty()),
            _ => {}
        }
    }
    (!draft.title.is_empty()).then_some(draft)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> Draft {
        Draft {
            title: "Supplier SFTP: confirm the import".into(),
            description: "Dana waits.\nSource: Gmail.".into(),
            team: "Core".into(),
            state: "Current sprint".into(),
            priority: 2,
            ..Draft::default()
        }
    }

    #[test]
    fn draft_survives_the_editor_unchanged() {
        assert_eq!(
            text_to_draft(&draft_to_text(&draft()), &draft()),
            Some(draft())
        );
    }

    #[test]
    fn edited_fields_are_read_back() {
        let text = draft_to_text(&draft())
            .replace("priority: 2", "priority: 3")
            .replace("Dana waits.", "Dana waits since Monday.");
        let edited = text_to_draft(&text, &draft()).unwrap();
        assert_eq!(edited.priority, 3);
        assert!(edited.description.starts_with("Dana waits since Monday."));
    }

    #[test]
    fn empty_title_cancels() {
        let text =
            draft_to_text(&draft()).replace("title: Supplier SFTP: confirm the import", "title:");
        assert_eq!(text_to_draft(&text, &draft()), None);
    }
}
