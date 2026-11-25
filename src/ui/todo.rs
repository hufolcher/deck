//! The todo list and the detail box of the selected item.

use super::status;
use super::{ACCENT, AMBER, DONE, GREEN, MUTED, RED, SELECTED_BG, panel};
use crate::app::App;
use crate::snapshot::BUCKETS;
use chrono::Local;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
};

/// A reason worth a colour: a client waiting or a date passing.
fn reason_color(reason: &str) -> Color {
    if reason.contains("client") {
        AMBER
    } else if reason.contains("overdue") || reason.contains("due today") {
        RED
    } else {
        MUTED
    }
}

const KEY_WIDTH: usize = 10;

/// Keys are padded to one column; a long slug key is cut, its line says the rest.
fn key_cell(key: &str) -> String {
    if key.chars().count() > KEY_WIDTH {
        format!("{}…", key.chars().take(KEY_WIDTH - 1).collect::<String>())
    } else {
        format!("{key:<KEY_WIDTH$}")
    }
}

/// The skill writes `what · reason · reason`: the head stays white.
fn split_line(line: &str) -> (&str, Option<&str>) {
    match line.split_once(" · ") {
        Some((head, reasons)) => (head, Some(reasons)),
        None => (line, None),
    }
}

pub fn draw_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = panel(Line::from(" Todo ".bold()), true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.order.is_empty() {
        let text = if app.running.is_empty() {
            " No todo yet: r runs the first refresh."
        } else {
            " First refresh running…"
        };
        frame.render_widget(Paragraph::new(text.fg(MUTED)), inner);
        return;
    }

    // Rows first, each with the position in `order` it shows, then a scroll
    // that keeps the selection in view.
    let mut rows: Vec<(Line, Option<usize>)> = Vec::new();
    let mut number = 0;
    // Items of the day's first list share the earliest added_at; later ones are new.
    let first_added: String = app
        .snapshot
        .items
        .iter()
        .filter_map(|item| item.added_at.as_deref()?.get(..16))
        .min()
        .unwrap_or("")
        .to_string();
    for (bucket, title) in BUCKETS {
        let positions: Vec<usize> = (0..app.order.len())
            .filter(|&position| app.snapshot.items[app.order[position]].bucket == bucket)
            .collect();
        if positions.is_empty() {
            continue;
        }
        if !rows.is_empty() {
            rows.push((Line::raw(""), None));
        }
        let color = match bucket {
            "now" => RED,
            "today" => AMBER,
            _ => MUTED,
        };
        rows.push((Line::from(format!(" {title}").fg(color).bold()), None));
        for position in positions {
            let item = &app.snapshot.items[app.order[position]];
            let selected = position == app.selected;
            let mark = if bucket == "later" {
                "  -".to_string()
            } else {
                number += 1;
                format!("{number:>3}")
            };
            let pointer = Span::raw(if selected { "▌" } else { " " }).fg(ACCENT);
            let mut line = if let Some(done) = item.done_text() {
                let (head, _) = split_line(&item.line);
                Line::from(vec![
                    pointer,
                    format!("{mark}  {} {head} → {done}", key_cell(&item.key))
                        .fg(DONE)
                        .add_modifier(Modifier::CROSSED_OUT),
                ])
            } else {
                let has_ticket = item.has_ticket();
                let (head, reasons) = split_line(&item.line);
                let mut spans = vec![
                    pointer,
                    Span::raw(mark).fg(MUTED),
                    if has_ticket {
                        Span::raw("  ")
                    } else {
                        Span::raw(" ◌").fg(AMBER)
                    },
                    if has_ticket {
                        key_cell(&item.key).fg(ACCENT).bold()
                    } else {
                        key_cell(&item.key.to_lowercase()).fg(MUTED).italic()
                    },
                    Span::raw(" "),
                    Span::raw(head.to_string()),
                ];
                if let Some(reasons) = reasons {
                    spans.push(format!("  · {reasons}").fg(reason_color(reasons)));
                }
                if item
                    .added_at
                    .as_deref()
                    .and_then(|added| added.get(..16))
                    .is_some_and(|added| added > first_added.as_str())
                {
                    spans.push("  new".fg(GREEN).bold());
                }
                let sessions = app.sessions_for(item);
                for badge in status::row(&app.statuses, &sessions, item) {
                    spans.push(format!("  {}", badge.text).fg(badge.tone.color()));
                }
                Line::from(spans)
            };
            if selected {
                line = line.bg(SELECTED_BG);
            }
            rows.push((line, Some(position)));
        }
    }

    let height = inner.height as usize;
    let selected_row = rows
        .iter()
        .position(|(_, position)| *position == Some(app.selected))
        .unwrap_or(0);
    if selected_row < app.list_scroll + 1 {
        // Keep the bucket title above the first item visible.
        app.list_scroll = selected_row.saturating_sub(2);
    } else if selected_row >= app.list_scroll + height {
        app.list_scroll = selected_row + 1 - height;
    }
    app.list_scroll = app.list_scroll.min(rows.len().saturating_sub(height));

    let visible: Vec<Line> = rows
        .iter()
        .skip(app.list_scroll)
        .take(height)
        .map(|(line, _)| line.clone())
        .collect();
    for (offset, (_, position)) in rows.iter().skip(app.list_scroll).take(height).enumerate() {
        if let Some(position) = position {
            let row = Rect {
                x: inner.x,
                y: inner.y + offset as u16,
                width: inner.width,
                height: 1,
            };
            app.hits.rows.push((row, *position));
        }
    }
    frame.render_widget(Paragraph::new(visible), inner);
}

pub fn draw_detail(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(item) = app.item().cloned() else {
        frame.render_widget(panel(Line::from(" — ".fg(MUTED)), false), area);
        return;
    };
    let block = panel(Line::from(format!(" {} ", item.key).bold()), true);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (head, reasons) = split_line(&item.line);
    let mut first = vec![Span::raw(head.to_string())];
    if let Some(reasons) = reasons {
        first.push(format!("  · {reasons}").fg(reason_color(reasons)));
    }
    let mut facts: Vec<String> = Vec::new();
    if let Some(status) = item.status.as_deref().filter(|status| !status.is_empty()) {
        facts.push(status.to_string());
    }
    if let Some(added) = item.added_at.as_deref().and_then(|added| added.get(11..16)) {
        facts.push(format!("added {added}"));
    }
    if let Some(done) = item.done_text() {
        facts.push(format!("done → {done}"));
    }

    let links = item.all_links();
    let mut link_spans = vec![Span::raw("Open ").fg(MUTED)];
    let mut x = inner.x + 5;
    let link_y = inner.y + 2;
    for (index, link) in links.iter().enumerate() {
        let text = format!(" {} ", link.label);
        let width = text.chars().count() as u16;
        let style = Style::new().fg(ACCENT);
        app.hits.links.push((
            Rect {
                x,
                y: link_y,
                width,
                height: 1,
            },
            index,
        ));
        link_spans.push(Span::styled(text, style));
        link_spans.push(Span::raw(" "));
        x += width + 1;
    }
    if links.is_empty() {
        link_spans.push("no link".fg(DONE));
    } else {
        link_spans.push("  click one, or ⏎ for all actions".fg(MUTED));
    }
    if !item.has_ticket() {
        link_spans.push("    ◌ no Linear ticket".fg(AMBER));
    }

    // Now: the item's Claude session first (a click takes it over), so it
    // never falls off a narrow box, then what the connectors say.
    let now = Local::now();
    let label = "Now  ";
    let mut now_spans = vec![Span::raw(label).fg(MUTED)];
    match app.sessions_for(&item).first() {
        Some(session) => {
            let badge = status::session(session, now);
            let text = format!(" {} ", badge.text);
            app.hits.claude = Some(Rect {
                x: inner.x + label.len() as u16,
                y: inner.y + 3,
                width: text.chars().count() as u16,
                height: 1,
            });
            now_spans.push(Span::styled(
                text,
                Style::new().fg(Color::Black).bg(badge.tone.color()),
            ));
            now_spans.push(" click: copy its command".fg(MUTED));
        }
        None => now_spans.push("no Claude session".fg(DONE)),
    }
    for phrase in status::detail(&app.statuses, &item, now) {
        now_spans.push(" · ".fg(DONE));
        now_spans.push(phrase.text.fg(phrase.tone.color()));
    }

    let lines = vec![
        Line::from(first),
        Line::from(facts.join(" · ").fg(MUTED)),
        Line::from(link_spans),
        Line::from(now_spans),
    ];
    // No wrapping: the link row has to stay on the row its click areas assume.
    frame.render_widget(Paragraph::new(lines), inner);
}
