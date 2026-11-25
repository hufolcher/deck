//! `deck claude [claude args…]`: Claude Code itself, in a pane deck draws,
//! with panels beside it: the session, the repo's changed files, your pull
//! requests and a watched command. Every key goes to Claude except the menu
//! key, which opens deck's dialog: fork the conversation, open a pull request,
//! hide the panels.

mod keys;
mod panels;
mod pty;
mod ui;

use crate::config::Config;
use crate::connectors::claude::Handoff;
use crate::connectors::shell::System;
use crate::keymap::Chord;
use panels::{Changes, Pr, Session, Update};
use pty::Pty;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Position, Rect};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};
use tui_term::vt100::MouseProtocolMode;

#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    Fork,
    OpenPr { number: u64, url: String },
    CopyPr { number: u64, url: String },
    Panels { show: bool },
    SendMenuKey,
    Quit,
}

impl Choice {
    pub fn label(&self, menu_key: &Chord) -> String {
        match self {
            Choice::Fork => "Fork this conversation · copies the command".into(),
            Choice::OpenPr { number, .. } => format!("Open #{number} in the browser"),
            Choice::CopyPr { number, .. } => format!("Copy the link to #{number}"),
            Choice::Panels { show: true } => "Show the side panels".into(),
            Choice::Panels { show: false } => "Hide the side panels".into(),
            Choice::SendMenuKey => format!("Send {} to Claude", menu_key.label()),
            Choice::Quit => "Quit: Claude stops too".into(),
        }
    }
}

pub struct Dialog {
    pub choices: Vec<Choice>,
    pub selected: usize,
}

impl Dialog {
    /// The pull requests of the current branch first, at most three of them.
    pub fn new(prs: &[Pr], side: bool) -> Dialog {
        let mut choices = vec![Choice::Fork];
        for pr in prs.iter().take(3) {
            choices.push(Choice::OpenPr {
                number: pr.number,
                url: pr.url.clone(),
            });
        }
        if let Some(pr) = prs.first() {
            choices.push(Choice::CopyPr {
                number: pr.number,
                url: pr.url.clone(),
            });
        }
        choices.extend([
            Choice::Panels { show: !side },
            Choice::SendMenuKey,
            Choice::Quit,
        ]);
        Dialog {
            choices,
            selected: 0,
        }
    }
}

#[derive(Default)]
pub struct Hits {
    pub claude: Rect,
    pub prs: Vec<(Rect, String)>,
    pub dialog: Vec<(Rect, usize)>,
}

pub struct Deck {
    pub config: Config,
    pub cwd: PathBuf,
    pub pty: Pty,
    output: Receiver<Vec<u8>>,
    updates: Receiver<Update>,
    pub changes: Option<Result<Changes, String>>,
    pub prs: Option<Result<Vec<Pr>, String>>,
    pub watch: Option<Result<Vec<String>, String>>,
    pub session: Option<Session>,
    session_read: Option<Instant>,
    pub side: bool,
    pub dialog: Option<Dialog>,
    pub toast: Option<(String, bool, Instant)>,
    pub hits: Hits,
    quit: bool,
}

impl Deck {
    fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.toast = Some((text.into(), error, Instant::now()));
    }

    pub fn prs_listed(&self) -> &[Pr] {
        self.prs
            .as_ref()
            .and_then(|prs| prs.as_ref().ok())
            .map_or(&[], Vec::as_slice)
    }

    fn tick(&mut self) {
        while let Ok(bytes) = self.output.try_recv() {
            if bytes.is_empty() {
                self.quit = true;
            }
            self.pty.parser.process(&bytes);
        }
        while let Ok(update) = self.updates.try_recv() {
            match update {
                Update::Changes(read) => self.changes = Some(read),
                Update::Prs(read) => self.prs = Some(read),
                Update::Watch(read) => self.watch = Some(read),
            }
        }
        if self
            .session_read
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(1))
        {
            self.session_read = Some(Instant::now());
            self.session = self
                .pty
                .pid()
                .and_then(|pid| Session::read(&Session::path(pid)));
        }
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, _, at)| at.elapsed() > Duration::from_secs(8))
        {
            self.toast = None;
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        let key = keys::named(key);
        if let Some(dialog) = self.dialog.as_mut() {
            let count = dialog.choices.len();
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.dialog = None,
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                    dialog.selected = (dialog.selected + 1) % count;
                }
                KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                    dialog.selected = (dialog.selected + count - 1) % count;
                }
                KeyCode::Enter => {
                    let selected = dialog.selected;
                    self.choose(selected);
                }
                KeyCode::Char(digit @ '1'..='9') => {
                    let index = digit as usize - '1' as usize;
                    if index < count {
                        self.choose(index);
                    }
                }
                _ => {}
            }
            return;
        }
        if Chord::from(key) == self.config.claude.menu_key {
            self.dialog = Some(Dialog::new(self.prs_listed(), self.side));
            return;
        }
        self.pty.parser.screen_mut().set_scrollback(0);
        let application_cursor = self.pty.parser.screen().application_cursor();
        let bytes = keys::encode(key, application_cursor);
        self.pty.write(&bytes);
    }

    fn choose(&mut self, index: usize) {
        let Some(choice) = self
            .dialog
            .take()
            .and_then(|dialog| dialog.choices.get(index).cloned())
        else {
            return;
        };
        match choice {
            Choice::Fork => match &self.session {
                Some(session) => {
                    let handoff =
                        Handoff::fork(&self.config.claude_bin, &session.session_id, &self.cwd);
                    match handoff.copy(&System) {
                        Ok(done) => self.notify(done, false),
                        Err(error) => self.notify(error.to_string(), true),
                    }
                }
                None => self.notify("no session yet: Claude has not registered it", true),
            },
            Choice::OpenPr { number, url } => match self.config.open(&url) {
                Ok(()) => self.notify(format!("opened #{number}"), false),
                Err(error) => self.notify(format!("cannot open {url}: {error}"), true),
            },
            Choice::CopyPr { url, .. } => {
                let handoff = Handoff {
                    line: url,
                    live_in: None,
                };
                match handoff.copy(&System) {
                    Ok(done) => self.notify(done, false),
                    Err(error) => self.notify(error.to_string(), true),
                }
            }
            Choice::Panels { show } => self.side = show,
            Choice::SendMenuKey => {
                self.pty
                    .write(&keys::encode(self.config.claude.menu_key.event(), false));
            }
            Choice::Quit => {
                self.pty.kill();
                self.quit = true;
            }
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent) {
        let at = Position::new(mouse.column, mouse.row);
        let claude = self.hits.claude;
        match mouse.kind {
            MouseEventKind::Down(_) if self.dialog.is_some() => {
                match self.hits.dialog.iter().find(|(area, _)| area.contains(at)) {
                    Some(&(_, index)) => self.choose(index),
                    None => self.dialog = None,
                }
            }
            MouseEventKind::Down(_) => {
                if let Some((_, url)) = self.hits.prs.iter().find(|(area, _)| area.contains(at)) {
                    let url = url.clone();
                    if let Err(error) = self.config.open(&url) {
                        self.notify(format!("cannot open {url}: {error}"), true);
                    }
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if claude.contains(at) => {
                let up = matches!(mouse.kind, MouseEventKind::ScrollUp);
                if self.pty.parser.screen().mouse_protocol_mode() == MouseProtocolMode::None {
                    self.pty.scroll(if up { 3 } else { -3 });
                } else {
                    let bytes = keys::wheel(up, mouse.column - claude.x, mouse.row - claude.y);
                    self.pty.write(&bytes);
                }
            }
            _ => {}
        }
    }

    fn on_paste(&mut self, text: &str) {
        if self.dialog.is_some() {
            return;
        }
        let bracketed = self.pty.parser.screen().bracketed_paste();
        self.pty.write(&keys::paste(text, bracketed));
    }
}

pub fn run(args: Vec<String>) -> std::io::Result<i32> {
    let config = Config::load();
    let cwd = std::env::current_dir()?;
    let (columns, rows) = ratatui::crossterm::terminal::size()?;
    let side = columns >= config.claude.side_min_width;
    let (output_tx, output) = channel();
    let first = ui::claude_area(Rect::new(0, 0, columns, rows), side);
    let pty = Pty::spawn(
        &config.claude_bin,
        &args,
        &cwd,
        (first.height, first.width),
        output_tx,
    )
    .map_err(std::io::Error::other)?;
    let (updates_tx, updates) = channel();
    panels::spawn(cwd.clone(), config.claude.watch.clone(), updates_tx);
    let mut deck = Deck {
        config,
        cwd,
        pty,
        output,
        updates,
        changes: None,
        prs: None,
        watch: None,
        session: None,
        session_read: None,
        side,
        dialog: None,
        toast: None,
        hits: Hits::default(),
        quit: false,
    };

    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let result = (|| -> std::io::Result<()> {
        while !deck.quit {
            deck.tick();
            if deck.pty.exited().is_some() {
                break;
            }
            terminal.draw(|frame| ui::draw(frame, &mut deck))?;
            if event::poll(Duration::from_millis(16))? {
                match event::read()? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => deck.on_key(key),
                    Event::Mouse(mouse) => deck.on_mouse(mouse),
                    Event::Paste(text) => deck.on_paste(&text),
                    _ => {}
                }
            }
        }
        Ok(())
    })();
    let _ = execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste
    );
    ratatui::restore();
    result?;
    let code = (0..50)
        .find_map(|_| {
            let code = deck.pty.exited();
            if code.is_none() {
                std::thread::sleep(Duration::from_millis(20));
            }
            code
        })
        .unwrap_or(0);
    Ok(code as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use panels::{Checks, Review};

    fn pr(number: u64) -> Pr {
        Pr {
            number,
            title: format!("PR {number}"),
            url: format!("https://example.test/pull/{number}"),
            branch: "b".into(),
            draft: false,
            conflicts: false,
            review: Review::None,
            checks: Checks::default(),
        }
    }

    #[test]
    fn dialog_without_prs() {
        let dialog = Dialog::new(&[], true);
        assert_eq!(
            dialog.choices,
            [
                Choice::Fork,
                Choice::Panels { show: false },
                Choice::SendMenuKey,
                Choice::Quit
            ]
        );
    }

    #[test]
    fn dialog_offers_three_prs_and_copies_the_first() {
        let prs: Vec<Pr> = (1..=4).map(pr).collect();
        let dialog = Dialog::new(&prs, false);
        let opened: Vec<u64> = dialog
            .choices
            .iter()
            .filter_map(|choice| match choice {
                Choice::OpenPr { number, .. } => Some(*number),
                _ => None,
            })
            .collect();
        assert_eq!(opened, [1, 2, 3]);
        assert!(dialog.choices.contains(&Choice::CopyPr {
            number: 1,
            url: "https://example.test/pull/1".into()
        }));
        assert!(dialog.choices.contains(&Choice::Panels { show: true }));
    }

    #[test]
    fn labels_name_the_menu_key() {
        assert_eq!(
            Choice::SendMenuKey.label(&Chord::ctrl(']')),
            "Send ctrl+] to Claude"
        );
    }
}
