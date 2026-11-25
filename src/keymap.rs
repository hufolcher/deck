//! Keys to actions. Few keys: moving, ⏎ for the item's actions (a dialog),
//! and the global ones. Each has standard defaults (arrows, ⏎, esc, letters)
//! and can be rebound in the config's `[keys]` table, e.g.
//! `refresh = ["r", "ctrl+r"]`. Bindings that collide with each other, or with a
//! key tmux takes before the app sees it, are reported at start.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Down,
    Up,
    PageDown,
    PageUp,
    First,
    Last,
    Open,
    NextPerson,
    PreviousPerson,
    MyDay,
    Team,
    Refresh,
    Settings,
    Help,
    Back,
    Quit,
}

impl Action {
    pub const ALL: [Action; 16] = [
        Action::Down,
        Action::Up,
        Action::PageDown,
        Action::PageUp,
        Action::First,
        Action::Last,
        Action::Open,
        Action::NextPerson,
        Action::PreviousPerson,
        Action::MyDay,
        Action::Team,
        Action::Refresh,
        Action::Settings,
        Action::Help,
        Action::Back,
        Action::Quit,
    ];

    pub fn describe(self) -> &'static str {
        match self {
            Action::Down => "next item",
            Action::Up => "previous item",
            Action::PageDown => "ten items down",
            Action::PageUp => "ten items up",
            Action::First => "first item",
            Action::Last => "last item",
            Action::Open => "the item's actions: open a link, Claude, ticket",
            Action::NextPerson => "next person's day",
            Action::PreviousPerson => "previous person's day",
            Action::MyDay => "back to your day",
            Action::Team => "team view, and back",
            Action::Refresh => "refresh todo, agenda and statuses",
            Action::Settings => "settings",
            Action::Help => "this help",
            Action::Back => "back, close",
            Action::Quit => "quit",
        }
    }

    fn defaults(self) -> &'static [&'static str] {
        match self {
            Action::Down => &["down", "j"],
            Action::Up => &["up", "k"],
            Action::PageDown => &["pagedown"],
            Action::PageUp => &["pageup"],
            Action::First => &["home", "g"],
            Action::Last => &["end", "G"],
            Action::Open => &["enter", "o"],
            Action::NextPerson => &["right", "l"],
            Action::PreviousPerson => &["left"],
            Action::MyDay => &["h"],
            Action::Team => &["t"],
            Action::Refresh => &["r"],
            Action::Settings => &["s"],
            Action::Help => &["?"],
            Action::Back => &["esc"],
            Action::Quit => &["q"],
        }
    }
}

/// One key with its modifiers, as written in the config: `j`, `G`, `?`,
/// `enter`, `shift+tab`, `ctrl+r`, `alt+left`, `f5`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Chord {
    code: KeyCode,
    modifiers: KeyModifiers,
}

const NAMED: [(&str, KeyCode, &str); 14] = [
    ("up", KeyCode::Up, "↑"),
    ("down", KeyCode::Down, "↓"),
    ("left", KeyCode::Left, "←"),
    ("right", KeyCode::Right, "→"),
    ("enter", KeyCode::Enter, "⏎"),
    ("esc", KeyCode::Esc, "esc"),
    ("tab", KeyCode::Tab, "⇥"),
    ("backtab", KeyCode::BackTab, "⇧⇥"),
    ("space", KeyCode::Char(' '), "space"),
    ("home", KeyCode::Home, "home"),
    ("end", KeyCode::End, "end"),
    ("pageup", KeyCode::PageUp, "pgup"),
    ("pagedown", KeyCode::PageDown, "pgdn"),
    ("backspace", KeyCode::Backspace, "⌫"),
];

impl Chord {
    /// Shift is part of the character (`G`) or of the key (`backtab`), never kept apart.
    fn normal(code: KeyCode, modifiers: KeyModifiers) -> Chord {
        let modifiers = match code {
            KeyCode::Char(_) | KeyCode::BackTab => modifiers - KeyModifiers::SHIFT,
            _ => modifiers,
        };
        Chord { code, modifiers }
    }

    /// A key name of `tmux list-keys`: `M-Left`, `C-a`, `S-Up`, `BTab`, `NPage`.
    pub fn from_tmux(name: &str) -> Option<Chord> {
        let mut modifiers = KeyModifiers::NONE;
        let mut rest = name;
        loop {
            let (flag, tail) = match rest.get(..2) {
                Some("C-") => (KeyModifiers::CONTROL, &rest[2..]),
                Some("M-") => (KeyModifiers::ALT, &rest[2..]),
                Some("S-") => (KeyModifiers::SHIFT, &rest[2..]),
                _ => break,
            };
            if tail.is_empty() {
                break;
            }
            modifiers |= flag;
            rest = tail;
        }
        let code = match rest {
            "Up" => KeyCode::Up,
            "Down" => KeyCode::Down,
            "Left" => KeyCode::Left,
            "Right" => KeyCode::Right,
            "Enter" => KeyCode::Enter,
            "Escape" => KeyCode::Esc,
            "Tab" => KeyCode::Tab,
            "BTab" => KeyCode::BackTab,
            "Space" => KeyCode::Char(' '),
            "Home" => KeyCode::Home,
            "End" => KeyCode::End,
            "PPage" | "PageUp" => KeyCode::PageUp,
            "NPage" | "PageDown" => KeyCode::PageDown,
            "BSpace" => KeyCode::Backspace,
            _ if rest.chars().count() == 1 => KeyCode::Char(rest.chars().next()?),
            _ if rest.starts_with('F') => KeyCode::F(rest[1..].parse().ok()?),
            _ => return None,
        };
        Some(Chord::normal(code, modifiers))
    }

    /// For the help and the footer: `↓`, `ctrl+r`, `G`.
    pub fn label(self) -> String {
        let mut text = String::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            text += "ctrl+";
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            text += "alt+";
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            text += "shift+";
        }
        match NAMED.iter().find(|(_, code, _)| *code == self.code) {
            Some((_, _, shown)) => text += shown,
            None => match self.code {
                KeyCode::Char(ch) => text.push(ch),
                KeyCode::F(number) => text += &format!("f{number}"),
                other => text += &format!("{other:?}").to_lowercase(),
            },
        }
        text
    }
}

impl From<KeyEvent> for Chord {
    fn from(event: KeyEvent) -> Chord {
        Chord::normal(event.code, event.modifiers)
    }
}

#[derive(Debug, PartialEq)]
pub struct UnknownKey(pub String);

impl FromStr for Chord {
    type Err = UnknownKey;

    fn from_str(text: &str) -> Result<Chord, UnknownKey> {
        let unknown = || UnknownKey(text.to_string());
        if text.chars().count() == 1 {
            let ch = text.chars().next().ok_or_else(unknown)?;
            return Ok(Chord::normal(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        let (prefix, key) = match text.rsplit_once('+') {
            Some((prefix, "")) => (prefix.strip_suffix('+').unwrap_or(prefix), "+"),
            Some((prefix, key)) => (prefix, key),
            None => ("", text),
        };
        let mut modifiers = KeyModifiers::NONE;
        for word in prefix.split('+').filter(|word| !word.is_empty()) {
            modifiers |= match word.to_lowercase().as_str() {
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "alt" | "meta" | "option" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                _ => return Err(unknown()),
            };
        }
        let lower = key.to_lowercase();
        let code = match NAMED.iter().find(|(name, _, _)| *name == lower) {
            Some((_, KeyCode::Tab, _)) if modifiers.contains(KeyModifiers::SHIFT) => {
                KeyCode::BackTab
            }
            Some((_, code, _)) => *code,
            None if key.chars().count() == 1 => {
                let ch = key.chars().next().ok_or_else(unknown)?;
                if modifiers.contains(KeyModifiers::SHIFT) {
                    KeyCode::Char(ch.to_ascii_uppercase())
                } else {
                    KeyCode::Char(ch)
                }
            }
            None => match lower
                .strip_prefix('f')
                .and_then(|number| number.parse().ok())
            {
                Some(number) => KeyCode::F(number),
                None => return Err(unknown()),
            },
        };
        Ok(Chord::normal(code, modifiers))
    }
}

impl std::fmt::Display for UnknownKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "unknown key `{}`", self.0)
    }
}

impl Serialize for Chord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut text = String::new();
        for (flag, word) in [
            (KeyModifiers::CONTROL, "ctrl+"),
            (KeyModifiers::ALT, "alt+"),
            (KeyModifiers::SHIFT, "shift+"),
        ] {
            if self.modifiers.contains(flag) {
                text += word;
            }
        }
        match NAMED.iter().find(|(_, code, _)| *code == self.code) {
            Some((name, _, _)) => text += name,
            None => match self.code {
                KeyCode::Char(ch) => text.push(ch),
                KeyCode::F(number) => text += &format!("f{number}"),
                _ => text += "?",
            },
        }
        serializer.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for Chord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Chord, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Why a binding cannot work as written.
#[derive(Debug, PartialEq)]
pub enum Clash {
    /// Two actions on one key: the first one listed keeps it.
    Twice(Chord, Action, Action),
    /// tmux takes this key in its root table: the app never receives it.
    Tmux(Chord, Action),
}

impl std::fmt::Display for Clash {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        let name = |action: &Action| format!("{action:?}").to_lowercase();
        match self {
            Clash::Twice(chord, kept, lost) => write!(
                formatter,
                "{} is bound to {} and {}: {} keeps it",
                chord.label(),
                name(kept),
                name(lost),
                name(kept)
            ),
            Clash::Tmux(chord, action) => {
                write!(
                    formatter,
                    "{} ({}) is taken by tmux",
                    chord.label(),
                    name(action)
                )
            }
        }
    }
}

pub struct Keymap {
    by_chord: HashMap<Chord, Action>,
    by_action: BTreeMap<Action, Vec<Chord>>,
}

impl Keymap {
    /// The defaults, with the config's actions replaced; `taken` are the keys
    /// something in front of the app (tmux) keeps for itself.
    pub fn new(overrides: &BTreeMap<Action, Vec<Chord>>, taken: &[Chord]) -> (Keymap, Vec<Clash>) {
        let mut keymap = Keymap {
            by_chord: HashMap::new(),
            by_action: BTreeMap::new(),
        };
        let mut clashes = Vec::new();
        for action in Action::ALL {
            let chords: Vec<Chord> = match overrides.get(&action) {
                Some(chords) => chords.clone(),
                None => action
                    .defaults()
                    .iter()
                    .filter_map(|text| text.parse().ok())
                    .collect(),
            };
            for chord in &chords {
                if taken.contains(chord) {
                    clashes.push(Clash::Tmux(*chord, action));
                }
                match keymap.by_chord.get(chord) {
                    Some(kept) => clashes.push(Clash::Twice(*chord, *kept, action)),
                    None => {
                        keymap.by_chord.insert(*chord, action);
                    }
                }
            }
            keymap.by_action.insert(action, chords);
        }
        (keymap, clashes)
    }

    pub fn action(&self, event: KeyEvent) -> Option<Action> {
        self.by_chord.get(&Chord::from(event)).copied()
    }

    /// `↓ j`: every key of the action, for the help and the footer.
    pub fn label(&self, action: Action) -> String {
        self.by_action
            .get(&action)
            .into_iter()
            .flatten()
            .map(|chord| chord.label())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The root-table keys of the tmux server this app runs under; none outside tmux.
pub fn tmux_taken<S: crate::connectors::shell::Shell>(shell: &S) -> Vec<Chord> {
    if std::env::var_os("TMUX").is_none() {
        return Vec::new();
    }
    let listing =
        crate::connectors::shell::Invocation::new("tmux").args(["list-keys", "-T", "root"]);
    let Ok(output) = shell.run(&listing) else {
        return Vec::new();
    };
    parse_tmux_keys(&output.stdout)
}

/// `bind-key [-r] [-N note] -T root M-Left select-pane -L` → `M-Left`.
fn parse_tmux_keys(listing: &str) -> Vec<Chord> {
    listing
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            words.find(|word| *word == "root")?;
            Chord::from_tmux(words.next()?)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(text: &str) -> Chord {
        text.parse().unwrap()
    }

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn defaults_are_standard_keys() {
        let (keymap, clashes) = Keymap::new(&BTreeMap::new(), &[]);
        assert!(clashes.is_empty(), "{clashes:?}");
        assert_eq!(
            keymap.action(press(KeyCode::Down, KeyModifiers::NONE)),
            Some(Action::Down)
        );
        assert_eq!(
            keymap.action(press(KeyCode::Char('j'), KeyModifiers::NONE)),
            Some(Action::Down)
        );
        assert_eq!(
            keymap.action(press(KeyCode::Esc, KeyModifiers::NONE)),
            Some(Action::Back)
        );
        assert_eq!(
            keymap.action(press(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Action::Open)
        );
        // Terminals send G with SHIFT.
        assert_eq!(
            keymap.action(press(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            Some(Action::Last)
        );
        assert_eq!(
            keymap.action(press(KeyCode::Char('?'), KeyModifiers::SHIFT)),
            Some(Action::Help)
        );
        assert_eq!(
            keymap.action(press(KeyCode::Char('x'), KeyModifiers::NONE)),
            None
        );
        assert_eq!(keymap.label(Action::Down), "↓ j");
    }

    #[test]
    fn every_action_has_a_default_and_a_description() {
        let (keymap, _) = Keymap::new(&BTreeMap::new(), &[]);
        for action in Action::ALL {
            assert!(!keymap.label(action).is_empty(), "{action:?}");
            assert!(!action.describe().is_empty());
        }
    }

    #[test]
    fn overrides_replace_and_clashes_are_named() {
        let overrides = BTreeMap::from([
            (Action::Refresh, vec![chord("ctrl+r")]),
            (Action::Team, vec![chord("j")]),
        ]);
        let (keymap, clashes) = Keymap::new(&overrides, &[]);
        assert_eq!(
            keymap.action(press(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            Some(Action::Refresh)
        );
        assert_eq!(
            keymap.action(press(KeyCode::Char('r'), KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            clashes,
            vec![Clash::Twice(chord("j"), Action::Down, Action::Team)]
        );
        assert_eq!(
            clashes[0].to_string(),
            "j is bound to down and team: down keeps it"
        );
    }

    #[test]
    fn keys_tmux_takes_are_reported() {
        let listing = "bind-key    -T root M-Left    select-pane -L\n\
                       bind-key -N \"Scroll mode\" -T root M-s copy-mode\n\
                       bind-key    -T prefix c new-window\n";
        let taken = parse_tmux_keys(listing);
        assert_eq!(taken, vec![chord("alt+left"), chord("alt+s")]);
        let overrides = BTreeMap::from([(Action::Settings, vec![chord("alt+s")])]);
        let (_, clashes) = Keymap::new(&overrides, &taken);
        assert_eq!(clashes, vec![Clash::Tmux(chord("alt+s"), Action::Settings)]);
        assert_eq!(clashes[0].to_string(), "alt+s (settings) is taken by tmux");
        // Defaults never use alt, so a tmux of alt bindings takes none of them.
        assert!(Keymap::new(&BTreeMap::new(), &taken).1.is_empty());
    }

    #[test]
    fn names_parse_both_ways() {
        assert_eq!(chord("shift+tab"), chord("backtab"));
        assert_eq!(chord("shift+g"), chord("G"));
        assert_eq!(
            chord("ctrl++"),
            Chord {
                code: KeyCode::Char('+'),
                modifiers: KeyModifiers::CONTROL
            }
        );
        assert_eq!(chord("f5").label(), "f5");
        assert_eq!(chord("ctrl+r").label(), "ctrl+r");
        assert_eq!(
            "hyper+x".parse::<Chord>(),
            Err(UnknownKey("hyper+x".into()))
        );
        assert_eq!("nope".parse::<Chord>(), Err(UnknownKey("nope".into())));
        assert_eq!(
            Chord::from_tmux("C-M-Up"),
            Some(Chord {
                code: KeyCode::Up,
                modifiers: KeyModifiers::CONTROL | KeyModifiers::ALT
            })
        );
        assert_eq!(
            Chord::from_tmux("M-S-Left"),
            Some(Chord {
                code: KeyCode::Left,
                modifiers: KeyModifiers::ALT | KeyModifiers::SHIFT
            })
        );
        assert_eq!(Chord::from_tmux("Nope"), None);
        for text in ["ctrl+r", "alt+left", "G", "?", "shift+tab", "f12", "space"] {
            let toml = toml::to_string(&BTreeMap::from([("key", chord(text))])).unwrap();
            let back: BTreeMap<String, Chord> = toml::from_str(&toml).unwrap();
            assert_eq!(back["key"], chord(text), "{text}");
        }
    }
}
