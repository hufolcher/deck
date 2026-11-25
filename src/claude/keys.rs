//! Keys, pastes and the wheel, turned back into the bytes a terminal would
//! have sent Claude.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Without the kitty keyboard protocol, crossterm reads ctrl+\ ] ^ _ as
/// ctrl+4 5 6 7 (the same bytes): give them their names back.
pub fn named(event: KeyEvent) -> KeyEvent {
    let renamed = match event.code {
        KeyCode::Char('4') => '\\',
        KeyCode::Char('5') => ']',
        KeyCode::Char('6') => '^',
        KeyCode::Char('7') => '_',
        _ => return event,
    };
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        KeyEvent::new(KeyCode::Char(renamed), event.modifiers)
    } else {
        event
    }
}

/// xterm's modifier parameter: 1 + shift 1, alt 2, ctrl 4.
fn parameter(modifiers: KeyModifiers) -> u8 {
    1 + u8::from(modifiers.contains(KeyModifiers::SHIFT))
        + 2 * u8::from(modifiers.contains(KeyModifiers::ALT))
        + 4 * u8::from(modifiers.contains(KeyModifiers::CONTROL))
}

/// `application_cursor`: the program asked for SS3 arrows (`ESC O A`).
pub fn encode(event: KeyEvent, application_cursor: bool) -> Vec<u8> {
    let modifiers = event.modifiers;
    let alt = modifiers.contains(KeyModifiers::ALT);
    let ctrl = modifiers.contains(KeyModifiers::CONTROL);
    let modified = parameter(modifiers) > 1;
    let csi = |final_byte: char| -> Vec<u8> {
        if modified {
            format!("\x1b[1;{}{final_byte}", parameter(modifiers)).into_bytes()
        } else if application_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    };
    let tilde = |number: u8| -> Vec<u8> {
        if modified {
            format!("\x1b[{number};{}~", parameter(modifiers)).into_bytes()
        } else {
            format!("\x1b[{number}~").into_bytes()
        }
    };
    let escaped = |bytes: Vec<u8>| -> Vec<u8> {
        if alt {
            [vec![0x1b], bytes].concat()
        } else {
            bytes
        }
    };
    match event.code {
        KeyCode::Char(ch) if ctrl => escaped(match ch.to_ascii_lowercase() {
            letter @ 'a'..='z' => vec![letter as u8 - b'a' + 1],
            ' ' | '@' | '2' => vec![0],
            '[' | '3' => vec![0x1b],
            '\\' | '4' => vec![0x1c],
            ']' | '5' => vec![0x1d],
            '^' | '6' => vec![0x1e],
            '_' | '7' | '/' => vec![0x1f],
            other => other.to_string().into_bytes(),
        }),
        KeyCode::Char(ch) => escaped(ch.to_string().into_bytes()),
        // Shift+enter as meta+enter: Claude Code reads it as a new line.
        KeyCode::Enter if modifiers.contains(KeyModifiers::SHIFT) => b"\x1b\r".to_vec(),
        KeyCode::Enter => escaped(b"\r".to_vec()),
        KeyCode::Tab => escaped(b"\t".to_vec()),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => escaped(vec![0x7f]),
        KeyCode::Esc => escaped(vec![0x1b]),
        KeyCode::Up => csi('A'),
        KeyCode::Down => csi('B'),
        KeyCode::Right => csi('C'),
        KeyCode::Left => csi('D'),
        KeyCode::Home => csi('H'),
        KeyCode::End => csi('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(number @ 1..=4) => format!("\x1bO{}", (b'P' + number - 1) as char).into_bytes(),
        KeyCode::F(number) => match number {
            5 => tilde(15),
            6 => tilde(17),
            7 => tilde(18),
            8 => tilde(19),
            9 => tilde(20),
            10 => tilde(21),
            11 => tilde(23),
            12 => tilde(24),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// A paste, bracketed when the program asked for it.
pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.as_bytes().to_vec()
    }
}

/// The wheel in SGR encoding, at a cell counted from 0.
pub fn wheel(up: bool, column: u16, row: u16) -> Vec<u8> {
    format!(
        "\x1b[<{};{};{}M",
        if up { 64 } else { 65 },
        column + 1,
        row + 1
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn characters_and_control_letters() {
        assert_eq!(
            encode(key(KeyCode::Char('é'), KeyModifiers::NONE), false),
            "é".as_bytes()
        );
        assert_eq!(
            encode(key(KeyCode::Char('c'), KeyModifiers::CONTROL), false),
            [3]
        );
        assert_eq!(
            encode(key(KeyCode::Char(']'), KeyModifiers::CONTROL), false),
            [0x1d]
        );
        assert_eq!(
            encode(key(KeyCode::Char('b'), KeyModifiers::ALT), false),
            b"\x1bb"
        );
    }

    #[test]
    fn arrows_follow_the_cursor_mode_and_modifiers() {
        assert_eq!(
            encode(key(KeyCode::Up, KeyModifiers::NONE), false),
            b"\x1b[A"
        );
        assert_eq!(
            encode(key(KeyCode::Up, KeyModifiers::NONE), true),
            b"\x1bOA"
        );
        assert_eq!(
            encode(key(KeyCode::Left, KeyModifiers::CONTROL), true),
            b"\x1b[1;5D"
        );
        assert_eq!(
            encode(key(KeyCode::Delete, KeyModifiers::SHIFT), false),
            b"\x1b[3;2~"
        );
    }

    #[test]
    fn enter_tab_and_function_keys() {
        assert_eq!(
            encode(key(KeyCode::Enter, KeyModifiers::NONE), false),
            b"\r"
        );
        assert_eq!(
            encode(key(KeyCode::Enter, KeyModifiers::SHIFT), false),
            b"\x1b\r"
        );
        assert_eq!(
            encode(key(KeyCode::BackTab, KeyModifiers::SHIFT), false),
            b"\x1b[Z"
        );
        assert_eq!(
            encode(key(KeyCode::F(1), KeyModifiers::NONE), false),
            b"\x1bOP"
        );
        assert_eq!(
            encode(key(KeyCode::F(5), KeyModifiers::NONE), false),
            b"\x1b[15~"
        );
    }

    #[test]
    fn legacy_control_digits_get_their_names() {
        let read = key(KeyCode::Char('5'), KeyModifiers::CONTROL);
        assert_eq!(named(read).code, KeyCode::Char(']'));
        let plain = key(KeyCode::Char('5'), KeyModifiers::NONE);
        assert_eq!(named(plain).code, KeyCode::Char('5'));
    }

    #[test]
    fn pastes_and_the_wheel() {
        assert_eq!(paste("a\nb", true), b"\x1b[200~a\nb\x1b[201~");
        assert_eq!(paste("a", false), b"a");
        assert_eq!(wheel(true, 0, 4), b"\x1b[<64;1;5M");
    }
}
