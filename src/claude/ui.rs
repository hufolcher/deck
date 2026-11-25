//! Claude's pane on the left, deck's panels on the right, one status line.

use super::Deck;
use super::panels::{Changes, Pr, Review};
use crate::ui::{ACCENT, AMBER, DONE, GREEN, MUTED, RED, draw_choices, panel};
use chrono::{Local, TimeZone};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::Stylize,
    text::{Line, Span},
    widgets::Paragraph,
};
use tui_term::widget::PseudoTerminal;

const SIDE_WIDTH: u16 = 46;

fn split(area: Rect, side: bool) -> (Rect, Option<Rect>, Rect) {
    let [main, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    if !side || main.width < SIDE_WIDTH + 20 {
        return (main, None, status);
    }
    let [claude, panels] =
        Layout::horizontal([Constraint::Min(20), Constraint::Length(SIDE_WIDTH)]).areas(main);
    (claude, Some(panels), status)
}

/// Where Claude's terminal goes, for its first size.
pub fn claude_area(area: Rect, side: bool) -> Rect {
    split(area, side).0
}

pub fn draw(frame: &mut Frame, deck: &mut Deck) {
    let (claude, side, status) = split(frame.area(), deck.side);
    deck.pty.resize(claude.height, claude.width);
    deck.hits.claude = claude;
    frame.render_widget(PseudoTerminal::new(deck.pty.parser.screen()), claude);
    deck.hits.prs.clear();
    if let Some(side) = side {
        draw_side(frame, side, deck);
    }
    draw_status(frame, status, deck);
    deck.hits.dialog.clear();
    if let Some(dialog) = deck.dialog.as_ref() {
        let menu_key = deck.config.claude.menu_key;
        let labels: Vec<String> = dialog.choices.iter().map(|c| c.label(&menu_key)).collect();
        let title = Line::from(" deck ".fg(ACCENT).bold());
        deck.hits.dialog = draw_choices(frame, title, &labels, dialog.selected);
    }
}

fn draw_side(frame: &mut Frame, area: Rect, deck: &mut Deck) {
    let watched = !deck.config.claude.watch.command.trim().is_empty();
    let mut constraints = vec![
        Constraint::Length(4),
        Constraint::Fill(2),
        Constraint::Fill(2),
    ];
    if watched {
        constraints.push(Constraint::Fill(1));
    }
    let areas = Layout::vertical(constraints).split(area);
    draw_session(frame, areas[0], deck);
    draw_changes(frame, areas[1], deck.changes.as_ref());
    draw_prs(frame, areas[2], deck);
    if watched {
        let title = format!(" {} ", deck.config.claude.watch.title);
        let lines: Vec<Line> = match &deck.watch {
            None => vec![Line::from("…".fg(MUTED))],
            Some(Err(error)) => vec![Line::from(error.clone().fg(RED))],
            Some(Ok(lines)) => lines.iter().map(|line| Line::raw(line.clone())).collect(),
        };
        frame.render_widget(
            Paragraph::new(lines).block(panel(Line::from(title.bold()), false)),
            areas[3],
        );
    }
}

fn draw_session(frame: &mut Frame, area: Rect, deck: &Deck) {
    let lines = match &deck.session {
        None => vec![Line::from("starting…".fg(MUTED))],
        Some(session) => {
            let colour = match session.status.as_str() {
                "busy" => AMBER,
                "idle" => GREEN,
                _ => MUTED,
            };
            let since = Local
                .timestamp_millis_opt(session.started_at)
                .single()
                .map(|at| format!(" · since {}", at.format("%H:%M")))
                .unwrap_or_default();
            let short: String = session.session_id.chars().take(8).collect();
            vec![
                Line::from(vec![
                    "● ".fg(colour),
                    session.status.clone().fg(colour),
                    "  ".into(),
                    Span::raw(session.name.clone()),
                ]),
                Line::from(format!("{short}{since}").fg(MUTED)),
            ]
        }
    };
    frame.render_widget(
        Paragraph::new(lines).block(panel(Line::from(" Session ".bold()), false)),
        area,
    );
}

/// The path's end when it does not fit: the file name matters most.
fn tail(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_string();
    }
    let kept: String = text.chars().skip(count + 1 - width).collect();
    format!("…{kept}")
}

fn draw_changes(frame: &mut Frame, area: Rect, changes: Option<&Result<Changes, String>>) {
    let width = area.width.saturating_sub(16) as usize;
    let (title, lines) = match changes {
        None => (" Changes ".to_string(), vec![Line::from("…".fg(MUTED))]),
        Some(Err(error)) => (" Changes ".into(), vec![Line::from(error.clone().fg(RED))]),
        Some(Ok(changes)) => {
            let mut title = format!(" ⎇ {} ", changes.branch);
            if changes.ahead > 0 {
                title += &format!("↑{} ", changes.ahead);
            }
            if changes.behind > 0 {
                title += &format!("↓{} ", changes.behind);
            }
            let mut lines: Vec<Line> = changes
                .files
                .iter()
                .map(|file| {
                    let colour = match file.code.trim() {
                        "??" => MUTED,
                        code if code.contains('D') => RED,
                        code if code.contains('A') => GREEN,
                        _ => AMBER,
                    };
                    let mut spans = vec![
                        format!("{:>2} ", file.code.trim()).fg(colour),
                        Span::raw(format!("{:<width$}", tail(&file.path, width))),
                    ];
                    if let (Some(added), Some(removed)) = (file.added, file.removed) {
                        spans.push(format!(" +{added}").fg(GREEN));
                        spans.push(format!(" −{removed}").fg(RED));
                    }
                    Line::from(spans)
                })
                .collect();
            if lines.is_empty() {
                lines.push(Line::from("clean".fg(DONE)));
            }
            (title, lines)
        }
    };
    frame.render_widget(
        Paragraph::new(lines).block(panel(Line::from(title.bold()), false)),
        area,
    );
}

/// One line on what the pull request waits for.
fn pr_state(pr: &Pr) -> Line<'static> {
    let mut spans: Vec<Span> = vec!["    ".into()];
    let checks = pr.checks;
    if checks.failed > 0 {
        spans.push(format!("✗ {} failing  ", checks.failed).fg(RED));
    }
    if checks.pending > 0 {
        spans.push(format!("◌ {} running  ", checks.pending).fg(AMBER));
    }
    if checks.failed == 0 && checks.pending == 0 && checks.passed > 0 {
        spans.push("✓ checks  ".fg(GREEN));
    }
    spans.push(match pr.review {
        Review::Approved => "approved".fg(GREEN),
        Review::ChangesRequested => "changes requested".fg(RED),
        Review::Required => "review required".fg(AMBER),
        Review::None => "no review".fg(MUTED),
    });
    if pr.conflicts {
        spans.push("  conflicts".fg(RED));
    }
    if pr.draft {
        spans.push("  draft".fg(MUTED));
    }
    Line::from(spans)
}

fn draw_prs(frame: &mut Frame, area: Rect, deck: &mut Deck) {
    let block = panel(Line::from(" Pull requests ".bold()), false)
        .title_bottom(Line::from(" click opens ".fg(MUTED)).alignment(Alignment::Right));
    let inner = block.inner(area);
    let branch = match &deck.changes {
        Some(Ok(changes)) => changes.branch.clone(),
        _ => String::new(),
    };
    let mut lines = Vec::new();
    match &deck.prs {
        None => lines.push(Line::from("…".fg(MUTED))),
        Some(Err(error)) => lines.push(Line::from(error.clone().fg(RED))),
        Some(Ok(prs)) if prs.is_empty() => {
            lines.push(Line::from("no open pull request of yours".fg(DONE)));
        }
        Some(Ok(prs)) => {
            for pr in prs {
                let row = inner.y + lines.len() as u16;
                if row + 1 >= inner.y + inner.height {
                    break;
                }
                let here = pr.branch == branch;
                lines.push(Line::from(vec![
                    if here { "▶ ".fg(ACCENT) } else { "  ".into() },
                    format!("#{} ", pr.number).fg(ACCENT).bold(),
                    Span::raw(pr.title.clone()),
                ]));
                lines.push(pr_state(pr));
                deck.hits
                    .prs
                    .push((Rect::new(inner.x, row, inner.width, 2), pr.url.clone()));
            }
        }
    }
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_status(frame: &mut Frame, area: Rect, deck: &Deck) {
    let key = deck.config.claude.menu_key.label();
    let scrolled = deck.pty.parser.screen().scrollback();
    let mut spans = vec![
        format!(" {key} ").black().on_green(),
        " deck: fork, pull requests, panels".fg(MUTED),
    ];
    if scrolled > 0 {
        spans.push(format!("   ↑ {scrolled} lines back · a key returns").fg(AMBER));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    if let Some((text, error, _)) = &deck.toast {
        let toast = Line::from(format!("{text} ").fg(if *error { RED } else { GREEN }))
            .alignment(Alignment::Right);
        frame.render_widget(Paragraph::new(toast), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panels_only_when_they_fit() {
        let (claude, side, status) = split(Rect::new(0, 0, 160, 40), true);
        assert_eq!(
            (claude.width, side.map(|s| s.width), status.y),
            (114, Some(46), 39)
        );
        assert_eq!(split(Rect::new(0, 0, 160, 40), false).1, None);
        assert_eq!(split(Rect::new(0, 0, 60, 40), true).1, None);
    }

    #[test]
    fn long_paths_keep_their_end() {
        assert_eq!(tail("src/claude/panels.rs", 12), "…e/panels.rs");
        assert_eq!(tail("a.rs", 12), "a.rs");
    }
}
