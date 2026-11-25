//! Drawing. Every draw function reads the app; the only thing it writes is
//! `app.hits`, the clickable areas of this frame.

mod day;
mod popup;
pub mod status;
mod todo;

use crate::app::{App, View};
use crate::jobs::Kind;
use crate::keymap::Action;
use crate::snapshot::BUCKETS;
use chrono::Local;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};

// The terminal's own ANSI colours, as tmux's default theme uses them (green
// status bar, yellow messages): deck follows the terminal theme like tmux.
pub const ACCENT: Color = Color::Green;
pub const MUTED: Color = Color::Gray;
pub const DONE: Color = Color::DarkGray;
pub const RED: Color = Color::Red;
pub const AMBER: Color = Color::Yellow;
pub const GREEN: Color = Color::Green;
pub const SELECTED_BG: Color = Color::Black;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner() -> &'static str {
    let tenth = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() / 100);
    SPINNER[tenth as usize % SPINNER.len()]
}

pub fn panel(title: Line<'static>, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { ACCENT } else { MUTED }))
        .title(title)
}

/// What fits: the day panel beside the list from `SIDE_MIN` columns, the
/// detail box from `DETAIL_MIN` rows. Below, the list gets the room.
pub const SIDE_MIN: u16 = 90;
pub const DETAIL_MIN: u16 = 18;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub side: bool,
    pub detail: bool,
}

impl Layout {
    pub fn for_size(width: u16, height: u16) -> Layout {
        Layout {
            side: width >= SIDE_MIN,
            detail: height >= DETAIL_MIN,
        }
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.hits = Default::default();
    let size = frame.area();
    let fits = Layout::for_size(size.width, size.height);
    let detail_rows = if app.view == View::Todo && fits.detail {
        6
    } else {
        0
    };
    let [header, body, detail, footer] = ratatui::layout::Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(detail_rows),
        Constraint::Length(1),
    ])
    .areas(size);

    draw_header(frame, header, app);
    match app.view {
        View::Todo if fits.side => {
            let day_width = 46.min(body.width / 3).max(30);
            let [list, day] = ratatui::layout::Layout::horizontal([
                Constraint::Min(40),
                Constraint::Length(day_width),
            ])
            .areas(body);
            todo::draw_list(frame, list, app);
            day::draw_day(frame, day, app);
        }
        View::Todo => todo::draw_list(frame, body, app),
        View::Team => day::draw_team(frame, body, app),
    }
    if detail_rows > 0 {
        todo::draw_detail(frame, detail, app);
    }
    draw_footer(frame, footer, app);
    if app.settings.is_some() {
        popup::draw_settings(frame, app);
    }
    if app.ticket.is_some() {
        popup::draw_ticket(frame, app);
    }
    if app.help {
        popup::draw_help(frame, app);
    }
    if app.menu.is_some() {
        popup::draw_menu(frame, app);
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let at = app.snapshot.at().map(|at| at.with_timezone(&Local));
    let day = at
        .unwrap_or_else(Local::now)
        .format("%a %-d %b")
        .to_string();
    let open = |bucket: &str| {
        app.snapshot
            .items
            .iter()
            .filter(|item| item.bucket == bucket && item.done_text().is_none())
            .count()
    };
    let done = app
        .snapshot
        .items
        .iter()
        .filter(|item| item.done_text().is_some())
        .count();
    let mut left = vec![
        format!(" {day} ").bold(),
        "· ".fg(MUTED),
        format!("{} now", open(BUCKETS[0].0)).fg(RED),
        format!(
            " · {} today · {} later",
            open(BUCKETS[1].0),
            open(BUCKETS[2].0)
        )
        .fg(MUTED),
    ];
    if done > 0 {
        left.push(format!(" · {done} done").fg(DONE));
    }
    let now = Local::now().format("%H:%M").to_string();
    let days = app.days();
    if let Some(next) = days
        .first()
        .and_then(|me| me.events.iter().find(|event| event.start > now))
    {
        let wait = crate::agenda::minutes(&next.start) - crate::agenda::minutes(&now);
        let wait = if wait >= 60 {
            format!("{}h{:02}", wait / 60, wait % 60)
        } else {
            format!("{wait} min")
        };
        left.push("   next ".fg(MUTED));
        left.push(format!("{} {}", next.start, next.title).fg(ACCENT).bold());
        left.push(format!(" in {wait}").fg(MUTED));
    }

    let mut right: Vec<Span> = Vec::new();
    for kind in [Kind::Refresh, Kind::Agenda, Kind::Draft, Kind::Create] {
        if let Some((since, _)) = app.running.get(&kind) {
            let seconds = since.elapsed().as_secs();
            right.push(
                format!(
                    "{} {} {}:{:02}  ",
                    spinner(),
                    kind.label(),
                    seconds / 60,
                    seconds % 60
                )
                .fg(ACCENT),
            );
        }
    }
    if let Some(at) = at {
        right.push(format!("updated {} ", at.format("%H:%M")).fg(MUTED));
    }
    let schedule = match app.next_refresh() {
        None => "· auto off ".to_string(),
        Some(_) if !app.active_at(Local::now()) => "· paused ".to_string(),
        Some(_) if app.running.contains_key(&Kind::Refresh) => String::new(),
        Some(next) => format!("· next {} ", next.format("%H:%M")),
    };
    right.push(schedule.fg(MUTED));
    let right = Line::from(right);
    let [top, _] =
        ratatui::layout::Layout::vertical([Constraint::Length(1), Constraint::Length(1)])
            .areas(area);
    let [left_area, right_area] = ratatui::layout::Layout::horizontal([
        Constraint::Min(10),
        Constraint::Length(right.width() as u16),
    ])
    .areas(top);
    frame.render_widget(Paragraph::new(Line::from(left)), left_area);
    frame.render_widget(
        Paragraph::new(right.alignment(Alignment::Right)),
        right_area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    if let Some(toast) = &app.toast {
        let color = if toast.error { RED } else { GREEN };
        frame.render_widget(Paragraph::new(format!(" {}", toast.text).fg(color)), area);
        return;
    }
    let shown: &[(Action, &str)] = match app.view {
        View::Todo => &[
            (Action::Open, "actions"),
            (Action::NextPerson, "agenda"),
            (Action::Team, "team"),
            (Action::Refresh, "refresh"),
            (Action::Settings, "settings"),
            (Action::Quit, "quit"),
        ],
        View::Team => &[
            (Action::Back, "back"),
            (Action::Refresh, "refresh"),
            (Action::Quit, "quit"),
        ],
    };
    let help = format!("{} help", app.keymap.label(Action::Help));
    // Keys as long as they fit, help always last.
    let room = (area.width as usize).saturating_sub(help.chars().count() + 2);
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1;
    for &(action, label) in shown {
        let key = app.keymap.label(action);
        let width = key.chars().count() + label.chars().count() + 3;
        if used + width > room {
            break;
        }
        used += width;
        spans.push(key.fg(ACCENT).bold());
        spans.push(format!(" {label}  ").fg(MUTED));
    }
    spans.push(app.keymap.label(Action::Help).fg(ACCENT).bold());
    spans.push(" help".fg(MUTED));
    let config = &app.config;
    let every = if config.refresh_minutes == 0 {
        "auto off".to_string()
    } else {
        format!("every {} min", config.refresh_minutes)
    };
    // The schedule only where it does not push keys out.
    let right = if area.width >= 110 {
        Line::from(format!("{every} · depth {} ", config.depth.label()).fg(MUTED))
    } else {
        Line::raw("")
    };
    let [left_area, right_area] = ratatui::layout::Layout::horizontal([
        Constraint::Min(10),
        Constraint::Length(right.width() as u16),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(Line::from(spans)), left_area);
    frame.render_widget(
        Paragraph::new(right.alignment(Alignment::Right)),
        right_area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_panel_and_detail_only_when_they_fit() {
        assert_eq!(
            Layout::for_size(160, 40),
            Layout {
                side: true,
                detail: true
            }
        );
        assert_eq!(
            Layout::for_size(SIDE_MIN - 1, 40),
            Layout {
                side: false,
                detail: true
            }
        );
        assert_eq!(
            Layout::for_size(160, DETAIL_MIN - 1),
            Layout {
                side: true,
                detail: false
            }
        );
        assert_eq!(
            Layout::for_size(40, 10),
            Layout {
                side: false,
                detail: false
            }
        );
    }

    #[test]
    fn theme_uses_only_the_terminal_palette() {
        for colour in [ACCENT, MUTED, DONE, RED, AMBER, GREEN, SELECTED_BG] {
            assert!(
                !matches!(colour, Color::Rgb(..) | Color::Indexed(_)),
                "{colour:?}"
            );
        }
    }
}
