//! Today's meetings for each person, written by the agenda job to
//! ~/.cache/daily-todo/agenda.json.

use crate::config::todo_dir;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Agenda {
    /// When the job ran, ISO.
    #[serde(default)]
    pub at: String,
    /// The day it describes, YYYY-MM-DD.
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub people: Vec<PersonDay>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct PersonDay {
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub events: Vec<Event>,
    /// Why the calendar could not be read (not shared, unknown address).
    #[serde(default)]
    pub error: Option<String>,
    /// HH:MM of the last run that read this calendar, when the latest could
    /// not and its events were kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept_from: Option<String>,
}

impl PersonDay {
    fn unread(&self) -> bool {
        self.events.is_empty() && self.error.is_some()
    }
}

/// The new run's days, where a calendar it could not read keeps the events an
/// earlier run of the same day read, marked with when.
pub fn merge(previous: Option<&Agenda>, date: &str, mut people: Vec<PersonDay>) -> Vec<PersonDay> {
    let Some(previous) = previous.filter(|previous| previous.date == date) else {
        return people;
    };
    let read_at = previous.at.get(11..16).unwrap_or("").to_string();
    for day in people.iter_mut().filter(|day| day.unread()) {
        if let Some(old) = previous
            .people
            .iter()
            .find(|old| old.name == day.name && !old.events.is_empty())
        {
            day.events = old.events.clone();
            day.kept_from = Some(old.kept_from.clone().unwrap_or_else(|| read_at.clone()));
        }
    }
    people
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Event {
    /// HH:MM, local time.
    pub start: String,
    pub end: String,
    pub title: String,
    #[serde(default)]
    pub location: String,
}

pub fn path() -> PathBuf {
    todo_dir().join("agenda.json")
}

pub fn load() -> Option<Agenda> {
    serde_json::from_str(&std::fs::read_to_string(path()).ok()?).ok()
}

pub fn save(agenda: &Agenda) -> std::io::Result<()> {
    std::fs::create_dir_all(todo_dir())?;
    let text = serde_json::to_string_pretty(agenda).map_err(std::io::Error::other)?;
    std::fs::write(path(), text)
}

/// Minutes since midnight for "HH:MM"; anything else sorts first.
pub fn minutes(time: &str) -> u32 {
    let (hours, rest) = time.split_once(':').unwrap_or(("0", "0"));
    hours.trim().parse::<u32>().unwrap_or(0) * 60
        + rest.get(..2).unwrap_or("0").parse::<u32>().unwrap_or(0)
}

/// Fallback for your own day while no agenda exists: the snapshot's
/// "09:45 Tech Team Daily Sync" lines, 30 minutes each since they carry no end.
pub fn from_snapshot_meetings(lines: &[String]) -> Vec<Event> {
    lines
        .iter()
        .filter_map(|line| {
            let (start, title) = line.split_once(' ')?;
            if start.len() != 5 || start.as_bytes()[2] != b':' {
                return None;
            }
            let end_minutes = minutes(start) + 30;
            Some(Event {
                start: start.into(),
                end: format!("{:02}:{:02}", end_minutes / 60, end_minutes % 60),
                title: title.trim().into(),
                location: String::new(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(name: &str, events: usize, error: Option<&str>) -> PersonDay {
        PersonDay {
            name: name.into(),
            events: (0..events)
                .map(|index| Event {
                    start: format!("{:02}:00", 9 + index),
                    end: format!("{:02}:30", 9 + index),
                    title: format!("{name} {index}"),
                    location: String::new(),
                })
                .collect(),
            error: error.map(String::from),
            ..PersonDay::default()
        }
    }

    #[test]
    fn unread_calendar_keeps_the_earlier_read() {
        let earlier = Agenda {
            at: "2026-11-25T09:05:00+01:00".into(),
            date: "2026-11-25".into(),
            people: vec![day("Ann", 2, None), day("Bob", 0, Some("not shared"))],
        };
        let now = vec![
            day("Ann", 0, Some("no calendar tool")),
            day("Bob", 0, Some("not shared")),
            day("Cy", 1, None),
        ];
        let merged = merge(Some(&earlier), "2026-11-25", now.clone());
        assert_eq!(merged[0].events.len(), 2);
        assert_eq!(merged[0].kept_from.as_deref(), Some("09:05"));
        assert_eq!(merged[0].error.as_deref(), Some("no calendar tool"));
        assert!(merged[1].events.is_empty() && merged[1].kept_from.is_none());
        assert_eq!(merged[2].events.len(), 1);
        // Kept again later: still says when it was read.
        let later = Agenda {
            at: "2026-11-25T10:05:00+01:00".into(),
            date: "2026-11-25".into(),
            people: merged,
        };
        assert_eq!(
            merge(Some(&later), "2026-11-25", now.clone())[0]
                .kept_from
                .as_deref(),
            Some("09:05")
        );
        // Yesterday's agenda is never carried over.
        assert!(
            merge(Some(&earlier), "2026-11-26", now)[0]
                .events
                .is_empty()
        );
    }

    #[test]
    fn minutes_of_a_time() {
        assert_eq!(minutes("09:45"), 585);
        assert_eq!(minutes("garbage"), 0);
    }

    #[test]
    fn snapshot_meetings_get_half_an_hour() {
        let events =
            from_snapshot_meetings(&["17:30 Sync on data model".into(), "no time here".into()]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].end, "18:00");
        assert_eq!(events[0].title, "Sync on data model");
    }
}
