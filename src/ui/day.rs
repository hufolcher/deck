//! The day: one person's agenda beside the todo, or everyone side by side.

use super::{ACCENT, AMBER, DONE, GREEN, MUTED, RED, panel};
use crate::agenda::{Event, minutes};
use crate::app::App;
use chrono::Local;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
};

fn short_name(name: &str) -> String {
    // Ann-Marie and Ann share their first letters: initials of each part.
    if name.contains('-') {
        name.split('-')
            .filter_map(|part| part.chars().next())
            .collect()
    } else {
        name.chars().take(2).collect()
    }
}

fn now_hhmm() -> String {
    Local::now().format("%H:%M").to_string()
}

/// One person's day beside the todo. ←→ walks through the team, h comes back.
pub fn draw_day(frame: &mut Frame, area: Rect, app: &mut App) {
    let days = app.days();
    let person = app.person.min(days.len().saturating_sub(1));
    let Some(day) = days.get(person) else { return };
    let (freshness, updating) = app.agenda_freshness();
    let block = panel(
        Line::from(vec![
            " Day · ".bold(),
            day.name.clone().fg(ACCENT).bold(),
            " ".into(),
        ]),
        false,
    )
    .title(
        Line::from(vec![
            if updating {
                freshness.clone().fg(Color::Black).bg(AMBER).bold()
            } else {
                freshness.clone().fg(MUTED)
            },
            format!(" {}/{} ", person + 1, days.len()).fg(MUTED),
        ])
        .alignment(Alignment::Right),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [tabs, status, _, agenda] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(2),
    ])
    .areas(inner);

    let mut spans = vec![Span::raw(" ")];
    let mut x = tabs.x + 1;
    for (index, other) in days.iter().enumerate() {
        let text = format!(" {} ", short_name(&other.name));
        let width = text.chars().count() as u16;
        if x + width > tabs.x + tabs.width {
            break;
        }
        let style = if index == person {
            Style::new().fg(Color::Black).bg(ACCENT).bold()
        } else if other.kept_from.is_some() {
            Style::new().fg(AMBER)
        } else if other.error.is_some() {
            Style::new().fg(DONE)
        } else {
            Style::new().fg(MUTED)
        };
        app.hits.tabs.push((
            Rect {
                x,
                y: tabs.y,
                width,
                height: 1,
            },
            index,
        ));
        spans.push(Span::styled(text, style));
        x += width;
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), tabs);

    let now = now_hhmm();
    // What is shown may be wrong: say so before anything else.
    let status_line = if updating {
        Line::from(" ⟳ calendars updating: may change".fg(AMBER).bold())
    } else if let Some(from) = &day.kept_from {
        let why = day.error.as_deref().unwrap_or("not read");
        Line::from(vec![
            format!(" ⚠ as of {from}: ").fg(AMBER).bold(),
            why.to_string().fg(MUTED),
        ])
    } else if let Some(error) = &day.error {
        Line::from(format!(" {error}").fg(DONE))
    } else if let Some(event) = day
        .events
        .iter()
        .find(|event| event.start <= now && now < event.end)
    {
        Line::from(vec![
            " ● busy ".fg(RED).bold(),
            format!("{} till {}", event.title, event.end).fg(MUTED),
        ])
    } else if let Some(event) = day.events.iter().find(|event| event.start > now) {
        Line::from(vec![
            " ● free ".fg(GREEN).bold(),
            format!("next {} {}", event.start, event.title).fg(MUTED),
        ])
    } else {
        Line::from(" ● free for the rest of the day".fg(GREEN))
    };
    frame.render_widget(Paragraph::new(status_line), status);

    let (lines, now_row) = agenda_lines(&day.events, &now);
    // Keep the current time in the upper third when the day is longer than the box.
    let height = agenda.height as usize;
    let scroll = now_row
        .saturating_sub(height / 3)
        .min(lines.len().saturating_sub(height));
    frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), agenda);
}

/// Hour marks, events under the hour they start in, a red line at now.
fn agenda_lines(events: &[Event], now: &str) -> (Vec<Line<'static>>, usize) {
    let first = events
        .iter()
        .map(|event| minutes(&event.start) / 60)
        .min()
        .unwrap_or(8)
        .min(8);
    let last = events
        .iter()
        .map(|event| minutes(&event.end).saturating_sub(1) / 60)
        .max()
        .unwrap_or(19)
        .max(19);
    let now_minutes = minutes(now);
    let mut lines = Vec::new();
    let mut now_row = 0;
    for hour in first..=last {
        lines.push(Line::from(vec![
            format!(" {hour:02} ").fg(MUTED),
            "┊".fg(DONE),
        ]));
        for event in events
            .iter()
            .filter(|event| minutes(&event.start) / 60 == hour)
        {
            let past = minutes(&event.end) <= now_minutes;
            let color = if past { DONE } else { ACCENT };
            lines.push(Line::from(vec![
                "    ".into(),
                "▍".fg(color),
                format!("{}–{} ", event.start, event.end).fg(color),
                event
                    .title
                    .clone()
                    .fg(if past { DONE } else { Color::White }),
            ]));
            if !event.location.is_empty() {
                lines.push(Line::from(vec![
                    "    ".into(),
                    "▍".fg(color),
                    format!("            {}", event.location).fg(MUTED),
                ]));
            }
        }
        if hour == now_minutes / 60 {
            now_row = lines.len();
            lines.push(Line::from(vec![
                " ── ".fg(RED),
                format!("{now} now ").fg(RED).bold(),
                "─".repeat(24).fg(RED),
            ]));
        }
    }
    (lines, now_row)
}

/// Everyone side by side in half-hour rows, to find a common slot.
pub fn draw_team(frame: &mut Frame, area: Rect, app: &mut App) {
    let days = app.days();
    let (freshness, updating) = app.agenda_freshness();
    let block = panel(Line::from(" Team day ".bold()), true).title(
        Line::from(vec![
            if updating {
                freshness.fg(Color::Black).bg(AMBER).bold()
            } else {
                freshness.fg(MUTED)
            },
            " t back to the todo ".fg(MUTED),
        ])
        .alignment(Alignment::Right),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if days.is_empty() {
        return;
    }

    let gutter = 8u16;
    let column = (inner.width.saturating_sub(gutter) / days.len() as u16).max(4) as usize;
    let mut header = vec![Span::raw(" ".repeat(gutter as usize))];
    for day in &days {
        // An earlier read: the name says so (⚠), in amber.
        let marked = match day.kept_from {
            Some(_) => format!("{}⚠", day.name),
            None => day.name.clone(),
        };
        let name: String = marked.chars().take(column - 1).collect();
        let cell = format!("{name:<column$}");
        header.push(if day.kept_from.is_some() {
            cell.fg(AMBER).bold()
        } else {
            cell.bold()
        });
    }
    let mut lines = vec![Line::from(header)];

    let now = minutes(&now_hhmm());
    let first = days
        .iter()
        .flat_map(|day| &day.events)
        .map(|event| minutes(&event.start) / 60)
        .min()
        .unwrap_or(8)
        .min(8);
    let last = days
        .iter()
        .flat_map(|day| &day.events)
        .map(|event| minutes(&event.end).div_ceil(60))
        .max()
        .unwrap_or(20)
        .max(20);
    for slot in (first * 60..last * 60).step_by(30) {
        let is_now = (slot..slot + 30).contains(&now);
        let label = format!("{:02}:{:02}", slot / 60, slot % 60);
        let mut spans = vec![if is_now {
            format!("▶{label}  ").fg(RED).bold()
        } else {
            format!(" {label}  ").fg(MUTED)
        }];
        for day in &days {
            let cell = column - 1;
            let event = day
                .events
                .iter()
                .find(|event| minutes(&event.start) < slot + 30 && minutes(&event.end) > slot);
            let span = match event {
                Some(event) => {
                    let starts_here = minutes(&event.start) >= slot;
                    let text: String = if starts_here {
                        event.title.chars().take(cell.saturating_sub(1)).collect()
                    } else {
                        String::new()
                    };
                    let past = minutes(&event.end) <= now;
                    Span::styled(
                        format!(" {text:<w$}", w = cell.saturating_sub(1)),
                        Style::new()
                            .fg(Color::Black)
                            .bg(if past { DONE } else { ACCENT }),
                    )
                }
                None if day.error.is_some() => {
                    Span::styled(format!("{:<cell$}", "?"), Style::new().fg(DONE))
                }
                None => Span::styled(
                    format!("{:<cell$}", if is_now { "─" } else { "·" }),
                    Style::new().fg(if is_now { RED } else { DONE }),
                ),
            };
            spans.push(span);
            spans.push(Span::raw(" "));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}
