//! Popups: help, settings, and the Linear ticket draft waiting for confirmation.

use super::{ACCENT, AMBER, DONE, GREEN, MUTED, RED, panel, spinner};
use crate::app::{App, SETTINGS_FIELDS, Ticket};
use crate::config::Config;
use crate::jobs::{Kind, PRIORITIES};
use crate::keymap::Action;
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

fn centered(frame: &Frame, width: u16, height: u16) -> Rect {
    let area = frame.area();
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: (area.width - width) / 2,
        y: (area.height - height) / 2,
        width,
        height,
    }
}

fn model_label(model: &str) -> String {
    if model.is_empty() {
        "default".into()
    } else {
        model.into()
    }
}

/// Every action with its keys as configured, then the popups' own keys.
pub fn draw_help(frame: &mut Frame, app: &App) {
    let keys: Vec<(String, &str)> = Action::ALL
        .iter()
        .map(|action| (app.keymap.label(*action), action.describe()))
        .chain([
            (
                "click".to_string(),
                "a row selects it, again opens its actions; a link opens it",
            ),
            (
                "⏎ ↑↓ 1-9 esc".to_string(),
                "actions: run, move, pick by number, close",
            ),
            ("wheel".to_string(), "scroll the list"),
            ("ctrl+c".to_string(), "quit, always"),
            (
                "⏎ e r esc".to_string(),
                "ticket draft: create, edit, redraft, cancel",
            ),
            ("↑↓ ←→ esc".to_string(), "settings: field, value, close"),
        ])
        .collect();
    let width = keys
        .iter()
        .map(|(key, _)| key.chars().count())
        .max()
        .unwrap_or(4)
        + 2;
    let mut lines = vec![Line::raw("")];
    lines.extend(keys.iter().map(|(key, what)| {
        Line::from(vec![
            format!("  {key:<width$}").fg(ACCENT).bold(),
            Span::raw((*what).to_string()),
        ])
    }));
    lines.push(Line::raw(""));
    lines.push(Line::from(
        "  Rebind in [keys] of ~/.config/deck/config.toml · any key closes".fg(MUTED),
    ));
    let area = centered(frame, 76, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(panel(Line::from(" Keys ".bold()), true)),
        area,
    );
}

pub fn draw_settings(frame: &mut Frame, app: &App) {
    let focused = app.settings.unwrap_or(0);
    let config: &Config = &app.config;
    let [from, to] = config.active_hours;
    let rows: [(&str, String, &str); SETTINGS_FIELDS] = [
        (
            "Refresh every",
            if config.refresh_minutes == 0 {
                "off".into()
            } else {
                format!("{} min", config.refresh_minutes)
            },
            "15 · 30 · 60 · 120 · off",
        ),
        (
            "Depth",
            config.depth.label().into(),
            "first run of the day looks back",
        ),
        (
            "Active hours",
            if from == 0 && to == 24 {
                "always".into()
            } else {
                format!("{from:02}–{to:02}")
            },
            "no automatic run outside",
        ),
        (
            "Days",
            if config.weekdays_only {
                "Mon–Fri".into()
            } else {
                "every day".into()
            },
            "",
        ),
        (
            "Todo model",
            model_label(&config.model),
            "the daily-todo run",
        ),
        (
            "Small model",
            model_label(&config.small_model),
            "agenda and tickets",
        ),
    ];
    let mut lines = vec![Line::raw("")];
    for (index, (label, value, hint)) in rows.into_iter().enumerate() {
        let style = if index == focused {
            Style::new().fg(Color::Black).bg(ACCENT).bold()
        } else {
            Style::new().fg(Color::White)
        };
        lines.push(Line::from(vec![
            format!("  {label:<15}").fg(MUTED),
            Span::styled(format!(" ◀ {value} ▶ "), style),
            format!("  {hint}").fg(DONE),
        ]));
    }
    lines.push(Line::raw(""));
    for kind in [Kind::Refresh, Kind::Agenda] {
        let text = match app.last.get(&kind) {
            Some(run) if run.ok => format!(
                "  Last {} run {} ✓ {}m{:02}s",
                kind.label(),
                run.at.format("%H:%M"),
                run.seconds / 60,
                run.seconds % 60
            ),
            Some(run) => format!(
                "  Last {} run {} ✗ {}",
                kind.label(),
                run.at.format("%H:%M"),
                run.error
            ),
            None => format!("  No {} run since start", kind.label()),
        };
        lines.push(Line::from(text.fg(MUTED)));
    }
    lines.push(Line::from(
        format!("  Saved to {}", Config::path().display()).fg(DONE),
    ));
    lines.push(Line::raw(""));
    lines.push(Line::from("  ↑↓ field   ←→ change   esc close".fg(MUTED)));

    let area = centered(frame, 74, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(panel(Line::from(" Settings ".bold()), true)),
        area,
    );
}

/// Word wrap by hand, so the popup knows its height and the key line under
/// the text always shows.
fn wrap(text: &str, width: usize, indent: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                lines.push(format!("{indent}{line}"));
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(format!("{indent}{line}"));
    }
    lines
}

const POPUP_WIDTH: u16 = 90;

pub fn draw_ticket(frame: &mut Frame, app: &App) {
    let text_width = (POPUP_WIDTH.min(frame.area().width) as usize).saturating_sub(10);
    let Some(ticket) = &app.ticket else { return };
    let field = |label: &str, value: Span<'static>| {
        Line::from(vec![format!("  {label:<11}").fg(MUTED), value])
    };
    let keys = |pairs: &[(&'static str, &'static str)]| {
        let mut spans = vec![Span::raw("  ")];
        for &(key, label) in pairs {
            spans.push(key.fg(ACCENT).bold());
            spans.push(format!(" {label}   ").fg(MUTED));
        }
        Line::from(spans)
    };
    let seconds = |kind: Kind| {
        app.running
            .get(&kind)
            .map_or(0, |(since, _)| since.elapsed().as_secs())
    };

    let (title, color, lines) = match ticket {
        Ticket::Drafting { key } => (
            " New Linear ticket ",
            AMBER,
            vec![
                Line::raw(""),
                Line::from(
                    format!(
                        "  {} drafting a ticket for {key}… {}s",
                        spinner(),
                        seconds(Kind::Draft)
                    )
                    .fg(ACCENT),
                ),
                Line::from(
                    "  Reading the source thread and your recent tickets. Nothing is created."
                        .fg(MUTED),
                ),
                Line::raw(""),
                keys(&[("esc", "cancel")]),
            ],
        ),
        Ticket::Ready { draft, .. } | Ticket::Creating { draft, .. } => {
            let creating = matches!(ticket, Ticket::Creating { .. });
            let mut lines = vec![Line::raw("")];
            for (index, part) in wrap(&draft.title, text_width.saturating_sub(9), "")
                .into_iter()
                .enumerate()
            {
                let label = if index == 0 { "Title" } else { "" };
                lines.push(field(label, Span::raw(part).bold()));
            }
            lines.push(Line::raw(""));
            lines.push(Line::from("  Description".fg(MUTED)));
            for line in wrap(&draft.description, text_width, "    ") {
                lines.push(Line::from(line));
            }
            lines.push(Line::raw(""));
            lines.push(field("Team", Span::raw(draft.team.clone())));
            lines.push(field("Status", Span::raw(draft.state.clone())));
            let priority = PRIORITIES
                .get(draft.priority as usize)
                .copied()
                .unwrap_or("None");
            lines.push(field(
                "Priority",
                Span::raw(priority).fg(if draft.priority <= 2 && draft.priority > 0 {
                    AMBER
                } else {
                    Color::White
                }),
            ));
            lines.push(field("Assignee", Span::raw("you")));
            if let Some(project) = draft.project.as_ref().filter(|project| !project.is_empty()) {
                lines.push(field("Project", Span::raw(project.clone())));
            }
            for link in &draft.links {
                lines.push(field("Link", Span::raw(link.title.clone()).fg(ACCENT)));
            }
            if let Some(duplicate) = &draft.possible_duplicate {
                lines.push(Line::raw(""));
                lines.push(Line::from(
                    format!("  ⚠ looks like {duplicate} already covers it").fg(RED),
                ));
            }
            lines.push(Line::raw(""));
            if creating {
                lines.push(Line::from(
                    format!(
                        "  {} creating in Linear… {}s",
                        spinner(),
                        seconds(Kind::Create)
                    )
                    .fg(ACCENT),
                ));
            } else {
                lines.push(keys(&[
                    ("⏎", "create in Linear"),
                    ("e", "edit"),
                    ("r", "redraft"),
                    ("esc", "cancel"),
                ]));
            }
            (
                " New Linear ticket · draft, not created yet ",
                if creating { ACCENT } else { AMBER },
                lines,
            )
        }
        Ticket::Failed { draft, error, .. } => {
            let mut lines = vec![Line::raw("")];
            for line in wrap(&format!("✗ {error}"), text_width, "  ") {
                lines.push(Line::from(line.fg(RED)));
            }
            lines.push(Line::raw(""));
            lines.push(Line::from(
                format!(
                    "  Details: {}",
                    crate::config::cache_dir().join("*.log").display()
                )
                .fg(DONE),
            ));
            lines.push(Line::raw(""));
            if draft.is_some() {
                lines.push(keys(&[
                    ("⏎", "back to the draft"),
                    ("r", "redraft"),
                    ("esc", "close"),
                ]));
            } else {
                lines.push(keys(&[("r", "try again"), ("esc", "close")]));
            }
            (" New Linear ticket · failed ", RED, lines)
        }
    };
    let area = centered(frame, POPUP_WIDTH, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    let block = panel(Line::from(title.bold()), true)
        .border_style(Style::new().fg(color))
        .title(
            Line::from(" nothing reaches Linear before ⏎ ".fg(GREEN)).alignment(Alignment::Right),
        );
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The selected item's actions: numbered rows, the selected one highlighted.
pub fn draw_menu(frame: &mut Frame, app: &mut App) {
    let Some(menu) = app.menu.as_ref() else {
        return;
    };
    let labels: Vec<String> = menu.choices.iter().map(|choice| choice.label()).collect();
    let width = labels
        .iter()
        .map(|label| label.chars().count() as u16 + 8)
        .max()
        .unwrap_or(30)
        .max(44);
    let area = centered(frame, width, labels.len() as u16 + 4);
    let block = panel(
        Line::from(vec![
            " ".into(),
            menu.key.clone().fg(ACCENT).bold(),
            " ".into(),
        ]),
        true,
    )
    .title_bottom(Line::from(" ↑↓ ⏎ or 1-9 · esc closes ".fg(MUTED)).alignment(Alignment::Right));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    let mut lines = vec![Line::raw("")];
    let mut hits = Vec::new();
    for (index, label) in labels.iter().enumerate() {
        let row = inner.y + 1 + index as u16;
        hits.push((
            Rect {
                x: inner.x,
                y: row,
                width: inner.width,
                height: 1,
            },
            index,
        ));
        let text = format!(" {}  {label}", index + 1);
        let padded = format!("{text:<w$}", w = inner.width as usize);
        lines.push(if index == menu.selected {
            Line::from(Span::styled(
                padded,
                Style::new().fg(Color::Black).bg(ACCENT).bold(),
            ))
        } else {
            Line::from(vec![
                format!(" {}  ", index + 1).fg(MUTED),
                Span::raw(label.clone()),
            ])
        });
    }
    frame.render_widget(Paragraph::new(lines), inner);
    app.hits.menu = hits;
}

#[cfg(test)]
mod tests {
    use super::wrap;

    #[test]
    fn wrap_keeps_words_and_indent() {
        let lines = wrap("one two three four", 9, "  ");
        assert_eq!(lines, vec!["  one two", "  three", "  four"]);
    }
}
