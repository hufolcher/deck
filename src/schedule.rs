//! Runs planned ahead, by Claude through the MCP server or by hand: a todo
//! refresh, or a work session on an item, once or every N minutes. The app
//! runs what is due on its tick; the file is ~/.cache/deck/schedule.json.

use chrono::{DateTime, Duration, FixedOffset};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "job", rename_all = "snake_case")]
pub enum Job {
    /// The todo skill, as `r` runs it.
    Refresh,
    /// A background Claude session on the item, as `w` starts it.
    Work { key: String },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: u32,
    pub at: DateTime<FixedOffset>,
    /// Again every this many minutes; none runs once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_minutes: Option<u32>,
    #[serde(flatten)]
    pub job: Job,
    /// Why it was planned, as the planner put it.
    #[serde(default)]
    pub note: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Schedule {
    pub entries: Vec<Entry>,
}

impl Schedule {
    pub fn path() -> std::path::PathBuf {
        crate::config::cache_dir().join("schedule.json")
    }

    pub fn load(path: &Path) -> Schedule {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(temporary, path)
    }

    pub fn add(
        &mut self,
        at: DateTime<FixedOffset>,
        every_minutes: Option<u32>,
        job: Job,
        note: String,
    ) -> &Entry {
        let id = self.entries.iter().map(|entry| entry.id).max().unwrap_or(0) + 1;
        self.entries.push(Entry {
            id,
            at,
            every_minutes: every_minutes.filter(|minutes| *minutes > 0),
            job,
            note,
        });
        self.entries.sort_by_key(|entry| entry.at);
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .expect("just added")
    }

    pub fn remove(&mut self, id: u32) -> Option<Entry> {
        let index = self.entries.iter().position(|entry| entry.id == id)?;
        Some(self.entries.remove(index))
    }

    /// The jobs due at `now`, once each: a one-off leaves the schedule, a
    /// repeating one moves past `now` (runs missed while the app was closed
    /// are not caught up).
    pub fn take_due(&mut self, now: DateTime<FixedOffset>) -> Vec<Job> {
        let mut due = Vec::new();
        self.entries.retain_mut(|entry| {
            if entry.at > now {
                return true;
            }
            due.push(entry.job.clone());
            match entry.every_minutes {
                Some(minutes) => {
                    let step = Duration::minutes(i64::from(minutes));
                    while entry.at <= now {
                        entry.at += step;
                    }
                    true
                }
                None => false,
            }
        });
        self.entries.sort_by_key(|entry| entry.at);
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(text).unwrap()
    }

    #[test]
    fn once_runs_once_and_leaves() {
        let mut schedule = Schedule::default();
        schedule.add(
            at("2026-11-25T14:00:00+01:00"),
            None,
            Job::Refresh,
            "after the call".into(),
        );
        assert!(
            schedule
                .take_due(at("2026-11-25T13:59:00+01:00"))
                .is_empty()
        );
        assert_eq!(
            schedule.take_due(at("2026-11-25T14:00:00+01:00")),
            vec![Job::Refresh]
        );
        assert!(schedule.entries.is_empty());
    }

    #[test]
    fn repeating_moves_past_now_without_catching_up() {
        let mut schedule = Schedule::default();
        let work = Job::Work {
            key: "ACME-1".into(),
        };
        schedule.add(
            at("2026-11-25T09:00:00+01:00"),
            Some(60),
            work.clone(),
            String::new(),
        );
        assert_eq!(
            schedule.take_due(at("2026-11-25T12:30:00+01:00")),
            vec![work]
        );
        assert_eq!(schedule.entries[0].at, at("2026-11-25T13:00:00+01:00"));
        assert!(
            schedule
                .take_due(at("2026-11-25T12:59:00+01:00"))
                .is_empty()
        );
    }

    #[test]
    fn ids_grow_and_remove_finds_them() {
        let mut schedule = Schedule::default();
        let first = schedule
            .add(
                at("2026-11-25T15:00:00+01:00"),
                Some(0),
                Job::Refresh,
                String::new(),
            )
            .id;
        assert_eq!(schedule.entries[0].every_minutes, None);
        let second = schedule
            .add(
                at("2026-11-25T14:00:00+01:00"),
                None,
                Job::Refresh,
                String::new(),
            )
            .id;
        assert_eq!((first, second), (1, 2));
        assert_eq!(schedule.entries[0].id, 2, "sorted by time");
        assert_eq!(schedule.remove(1).map(|entry| entry.id), Some(1));
        assert_eq!(schedule.remove(1), None);
    }

    #[test]
    fn file_round_trip() {
        let path = std::env::temp_dir().join(format!("deck-schedule-{}.json", std::process::id()));
        let mut schedule = Schedule::default();
        schedule.add(
            at("2026-11-25T14:00:00+01:00"),
            Some(30),
            Job::Work {
                key: "ACME-1".into(),
            },
            "x".into(),
        );
        schedule.save(&path).unwrap();
        assert_eq!(Schedule::load(&path), schedule);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#""job": "work""#) && text.contains(r#""key": "ACME-1""#));
        assert_eq!(
            Schedule::load(Path::new("/nonexistent")),
            Schedule::default()
        );
    }
}
