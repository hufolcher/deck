//! The daily-todo skill's snapshot (~/.cache/daily-todo/last.json): read to
//! draw the list, patched when a ticket is created for an item.

use crate::config::{LinkBases, todo_dir};
use chrono::{DateTime, FixedOffset};
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

pub const BUCKETS: [(&str, &str); 3] = [
    ("now", "NOW"),
    ("today", "TODAY"),
    ("later", "WHEN THERE IS TIME"),
];

static BASES: std::sync::RwLock<Option<LinkBases>> = std::sync::RwLock::new(None);

/// Set once from the config at start-up (and by tests).
pub fn set_link_bases(bases: LinkBases) {
    *BASES.write().unwrap_or_else(|poison| poison.into_inner()) = Some(bases);
}

fn bases() -> LinkBases {
    BASES
        .read()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
        .unwrap_or_default()
}

/// A tracker link: under the configured Linear base, or any linear.app issue.
fn is_ticket_url(url: &str) -> bool {
    let base = bases().linear;
    (!base.is_empty() && url.starts_with(&base)) || url.contains("linear.app/")
}

#[derive(Deserialize, Default, Clone)]
pub struct Snapshot {
    #[serde(default)]
    pub at: String,
    #[serde(default)]
    pub meetings: Vec<String>,
    /// One malformed item is skipped; it does not cost the whole list.
    #[serde(default, deserialize_with = "lenient_items")]
    pub items: Vec<Item>,
}

fn lenient_items<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<Item>, D::Error> {
    let values = Vec::<Value>::deserialize(deserializer)?;
    Ok(values
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect())
}

#[derive(Clone, PartialEq, Debug)]
pub struct Link {
    pub label: String,
    pub url: String,
}

/// The skill writes `{label, url}`; a bare URL is accepted and labelled by
/// its site.
impl<'de> Deserialize<'de> for Link {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Shape {
            Url(String),
            Full {
                #[serde(alias = "title")]
                label: String,
                url: String,
            },
        }
        Ok(match Shape::deserialize(deserializer)? {
            Shape::Full { label, url } => Link { label, url },
            Shape::Url(url) => Link {
                label: label_for(&url),
                url,
            },
        })
    }
}

fn label_for(url: &str) -> String {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url);
    let known = [
        ("linear.app", "Linear"),
        ("slack.com", "Slack"),
        ("mail.google.com", "Gmail"),
        ("github.com", "GitHub"),
        ("notion.so", "Notion"),
    ];
    match known.iter().find(|(domain, _)| host.ends_with(domain)) {
        Some((_, "Linear")) => {
            let id = url
                .split("/issue/")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .unwrap_or("");
            format!("Linear {id}").trim().to_string()
        }
        Some((_, "GitHub")) if url.contains("/pull/") => {
            let parts: Vec<&str> = url.trim_end_matches('/').rsplit('/').collect();
            match parts.as_slice() {
                [number, "pull", repo, ..] => format!("{repo}#{number}"),
                _ => "GitHub".into(),
            }
        }
        Some((_, name)) => name.to_string(),
        None => host.to_string(),
    }
}

#[derive(Deserialize, Clone)]
pub struct Item {
    pub key: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub line: String,
    #[serde(default)]
    pub asks: Value,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub done: Option<Value>,
    #[serde(default)]
    pub added_at: Option<String>,
    /// Written by the skill; older snapshots have none and get derived links.
    #[serde(default)]
    pub links: Vec<Link>,
}

pub fn path() -> PathBuf {
    todo_dir().join("last.json")
}

pub fn load() -> Option<Snapshot> {
    serde_json::from_str(&std::fs::read_to_string(path()).ok()?).ok()
}

/// The skill writes `2026-11-04T09:37+01:00`, without seconds.
pub fn parse_time(text: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(text)
        .or_else(|_| DateTime::parse_from_str(text, "%Y-%m-%dT%H:%M%:z"))
        .ok()
}

impl Snapshot {
    pub fn at(&self) -> Option<DateTime<FixedOffset>> {
        parse_time(&self.at)
    }

    /// Item indexes in display order: by bucket, keeping the skill's order
    /// inside a bucket; unknown buckets last.
    pub fn display_order(&self) -> Vec<usize> {
        let rank = |bucket: &str| {
            BUCKETS
                .iter()
                .position(|(name, _)| *name == bucket)
                .unwrap_or(BUCKETS.len())
        };
        let mut order: Vec<usize> = (0..self.items.len()).collect();
        order.sort_by_key(|&index| rank(&self.items[index].bucket));
        order
    }
}

impl Item {
    pub fn done_text(&self) -> Option<String> {
        match self.done.as_ref()? {
            Value::Null => None,
            Value::String(text) => Some(text.clone()),
            Value::Object(done) => Some(
                done.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("done")
                    .to_string(),
            ),
            _ => Some("done".into()),
        }
    }

    /// The skill's links first, then the ones that can be rebuilt from the
    /// ids it stores in `asks`, without duplicates.
    pub fn all_links(&self) -> Vec<Link> {
        let mut links = self.links.clone();
        for link in derived_links(&self.key, &self.asks, &bases()) {
            if !links.iter().any(|known| same_target(&known.url, &link.url)) {
                links.push(link);
            }
        }
        links
    }

    /// The texts the skill stored under one kind of ask (`linear`, `slack`, `pr`…).
    pub fn asks_of(&self, kind: &str) -> Vec<String> {
        self.asks.get(kind).map(strings).unwrap_or_default()
    }

    pub fn has_ticket(&self) -> bool {
        self.all_links().iter().any(|link| is_ticket_url(&link.url))
    }
}

/// Linear URLs end with an optional title slug: compare on the identifier.
fn same_target(a: &str, b: &str) -> bool {
    let linear = bases().linear;
    let trim = |url: &str| -> String {
        if let Some(rest) = url
            .strip_prefix(linear.as_str())
            .filter(|_| !linear.is_empty())
        {
            return rest.split('/').next().unwrap_or(rest).to_string();
        }
        // Gmail: /u/0/ and /u/<address>/ open the same thread.
        if url.contains("mail.google.com") {
            return format!("gmail:{}", url.rsplit('/').next().unwrap_or(url));
        }
        url.trim_end_matches('/').to_string()
    };
    trim(a) == trim(b)
}

fn strings(value: &Value) -> Vec<String> {
    match value {
        Value::String(text) => vec![text.clone()],
        Value::Array(values) => values.iter().flat_map(strings).collect(),
        _ => Vec::new(),
    }
}

static TICKET_PREFIXES: std::sync::RwLock<Vec<String>> = std::sync::RwLock::new(Vec::new());

/// Set once from the config at start-up: which `ABC-123` prefixes are tickets.
/// Empty: any uppercase prefix of two letters or more.
pub fn set_ticket_prefixes(prefixes: Vec<String>) {
    *TICKET_PREFIXES
        .write()
        .unwrap_or_else(|poison| poison.into_inner()) = prefixes;
}

/// Tracker ids in a text: `ABC-123` words, under the configured prefixes.
pub fn ticket_ids(text: &str) -> Vec<String> {
    let prefixes = TICKET_PREFIXES
        .read()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    text.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
        .filter(|token| {
            token.split_once('-').is_some_and(|(prefix, number)| {
                prefix.len() >= 2
                    && prefix.chars().all(|ch| ch.is_ascii_uppercase())
                    && (prefixes.is_empty() || prefixes.iter().any(|known| known == prefix))
                    && !number.is_empty()
                    && number.chars().all(|ch| ch.is_ascii_digit())
            })
        })
        .map(str::to_string)
        .collect()
}

fn derived_links(key: &str, asks: &Value, bases: &LinkBases) -> Vec<Link> {
    let mut links = Vec::new();
    let mut push = |label: String, base: &str, rest: String| {
        let url = format!("{base}{rest}");
        let usable = !base.is_empty() || rest.starts_with("https://");
        if usable && !links.iter().any(|link: &Link| link.url == url) {
            links.push(Link { label, url });
        }
    };
    for id in ticket_ids(key) {
        push(format!("Linear {id}"), &bases.linear, id);
    }
    let Value::Object(asks) = asks else {
        return links;
    };
    for (kind, value) in asks {
        for text in strings(value) {
            match kind.as_str() {
                "linear" => {
                    for id in ticket_ids(&text) {
                        push(format!("Linear {id}"), &bases.linear, id);
                    }
                }
                "gmail" => {
                    if let Some(thread) = text
                        .split_whitespace()
                        .next()
                        .filter(|id| id.len() >= 10 && id.chars().all(|ch| ch.is_ascii_hexdigit()))
                    {
                        push("Gmail".into(), &bases.gmail, thread.to_string());
                    }
                }
                "slack" => {
                    let channel = text.split_whitespace().find(|token| {
                        token.len() >= 9
                            && token.starts_with(['C', 'D', 'G'])
                            && token
                                .chars()
                                .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
                    });
                    let ts = text.split_whitespace().find(|token| {
                        token.split_once('.').is_some_and(|(seconds, micros)| {
                            seconds.len() == 10
                                && seconds
                                    .chars()
                                    .chain(micros.chars())
                                    .all(|ch| ch.is_ascii_digit())
                        })
                    });
                    if let (Some(channel), Some(ts)) = (channel, ts) {
                        push(
                            "Slack".into(),
                            &bases.slack,
                            format!("{channel}/p{}", ts.replace('.', "")),
                        );
                    }
                }
                "pr" => {
                    if let Some((repo, number)) = text.split_once('#') {
                        let short = repo.rsplit('/').next().unwrap_or(repo);
                        push(
                            format!("{short}#{number}"),
                            "https://github.com/",
                            format!("{repo}/pull/{number}"),
                        );
                    }
                }
                "team_inbox" => push("Team inbox".into(), &bases.team_inbox, String::new()),
                _ if text.starts_with("https://") => push(kind.clone(), "", text.clone()),
                _ => {}
            }
        }
    }
    links
}

/// Why an edit of the snapshot did not happen.
#[derive(Debug, PartialEq)]
pub enum EditError {
    NoItem(String),
    Taken(String),
    Bucket(String),
    File(String),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            EditError::NoItem(key) => write!(formatter, "no item {key} in the todo"),
            EditError::Taken(key) => write!(formatter, "an item {key} is already in the todo"),
            EditError::Bucket(bucket) => {
                let names: Vec<&str> = BUCKETS.iter().map(|(name, _)| *name).collect();
                write!(formatter, "no bucket {bucket}: one of {}", names.join(", "))
            }
            EditError::File(reason) => write!(formatter, "snapshot: {reason}"),
        }
    }
}

/// Edits the snapshot file in place, keeping every field as the skill wrote
/// it, then replaces it atomically; the app redraws on the new mtime.
fn edit_items<T>(
    change: impl FnOnce(&mut Vec<Value>) -> Result<T, EditError>,
) -> Result<T, EditError> {
    let file = |error: std::io::Error| EditError::File(error.to_string());
    let mut snapshot: Value = match std::fs::read_to_string(path()) {
        Ok(text) => {
            serde_json::from_str(&text).map_err(|error| EditError::File(error.to_string()))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            serde_json::json!({ "items": [] })
        }
        Err(error) => return Err(file(error)),
    };
    let items = snapshot
        .get_mut("items")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| EditError::File("no items".into()))?;
    let result = change(items)?;
    let pretty = serde_json::to_string_pretty(&snapshot)
        .map_err(|error| EditError::File(error.to_string()))?;
    if let Some(dir) = path().parent() {
        std::fs::create_dir_all(dir).map_err(file)?;
    }
    let temporary = path().with_extension("json.tmp");
    std::fs::write(&temporary, pretty).map_err(file)?;
    std::fs::rename(temporary, path()).map_err(file)?;
    Ok(result)
}

fn find<'a>(items: &'a mut [Value], key: &str) -> Result<&'a mut Value, EditError> {
    items
        .iter_mut()
        .find(|item| item.get("key").and_then(Value::as_str) == Some(key))
        .ok_or_else(|| EditError::NoItem(key.to_string()))
}

fn known_bucket(bucket: &str) -> Result<(), EditError> {
    if BUCKETS.iter().any(|(name, _)| *name == bucket) {
        Ok(())
    } else {
        Err(EditError::Bucket(bucket.to_string()))
    }
}

/// A new item, at the end of its bucket.
pub fn add_item(
    key: &str,
    bucket: &str,
    line: &str,
    links: &[Link],
    added_at: &str,
) -> Result<(), EditError> {
    known_bucket(bucket)?;
    edit_items(|items| {
        if find(items, key).is_ok() {
            return Err(EditError::Taken(key.to_string()));
        }
        let links: Vec<Value> = links
            .iter()
            .map(|link| serde_json::json!({ "label": link.label, "url": link.url }))
            .collect();
        items.push(serde_json::json!({
            "key": key, "bucket": bucket, "line": line, "asks": {}, "links": links,
            "done": null, "added_at": added_at,
        }));
        Ok(())
    })
}

/// Done with what closed it, or open again with None.
pub fn set_done(key: &str, done: Option<&str>) -> Result<(), EditError> {
    edit_items(|items| {
        find(items, key)?["done"] =
            done.map_or(Value::Null, |text| serde_json::json!({ "text": text }));
        Ok(())
    })
}

pub fn move_item(key: &str, bucket: &str) -> Result<(), EditError> {
    known_bucket(bucket)?;
    edit_items(|items| {
        find(items, key)?["bucket"] = Value::String(bucket.to_string());
        Ok(())
    })
}

/// After a ticket is created for `key`: the item takes the ticket's id as its
/// key, so the skill's next run follows the ticket like any other, and the
/// link goes first. The rest of the file is kept as the skill wrote it.
pub fn attach_ticket(key: &str, identifier: &str, url: &str) -> Result<(), EditError> {
    edit_items(|items| {
        let item = find(items, key)?;
        item["key"] = Value::String(identifier.into());
        if !item.get("asks").is_some_and(Value::is_object) {
            item["asks"] = serde_json::json!({});
        }
        item["asks"]["linear"] = Value::String(identifier.into());
        let mut links =
            vec![serde_json::json!({ "label": format!("Linear {identifier}"), "url": url })];
        if let Some(Value::Array(old)) = item.get("links") {
            links.extend(old.iter().cloned());
        }
        item["links"] = Value::Array(links);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const LINEAR: &str = "https://linear.app/acme/issue/";
    const SLACK: &str = "https://acme.slack.com/archives/";
    const GMAIL: &str = "https://mail.google.com/mail/u/0/#all/";
    const TEAM_INBOX: &str = "https://admin.acme.test/inbox";

    fn test_bases() -> LinkBases {
        LinkBases {
            linear: LINEAR.into(),
            slack: SLACK.into(),
            gmail: GMAIL.into(),
            team_inbox: TEAM_INBOX.into(),
        }
    }

    fn urls(key: &str, asks: Value) -> Vec<String> {
        derived_links(key, &asks, &test_bases())
            .into_iter()
            .map(|link| link.url)
            .collect()
    }

    #[test]
    fn no_base_no_link() {
        let got = derived_links(
            "ACME-1",
            &json!({ "gmail": "1a15d9f8c90f9162" }),
            &LinkBases::default(),
        );
        assert!(got.is_empty());
    }

    #[test]
    fn linear_from_key_and_asks_once() {
        let got = urls(
            "ACME-53",
            json!({ "linear": ["ACME-53", "ACME-52 comment bbd4b2bd"] }),
        );
        assert_eq!(
            got,
            vec![format!("{LINEAR}ACME-53"), format!("{LINEAR}ACME-52")]
        );
    }

    #[test]
    fn gmail_thread_id_before_a_timestamp() {
        let got = urls(
            "supplier-sftp",
            json!({ "gmail": "1a15d9f8c90f9162 2026-11-03T16:24Z" }),
        );
        assert_eq!(got, vec![format!("{GMAIL}1a15d9f8c90f9162")]);
    }

    #[test]
    fn slack_needs_channel_and_ts() {
        let got = urls(
            "x",
            json!({ "slack": "C08E7MKK6NS thread 1787569904.655969" }),
        );
        assert_eq!(got, vec![format!("{SLACK}C08E7MKK6NS/p1787569904655969")]);
        assert!(urls("x", json!({ "slack": "#dev-chat 2026-11-03 12:25" })).is_empty());
    }

    #[test]
    fn pull_requests_and_team_inbox() {
        let got = urls(
            "x",
            json!({ "pr": ["acme/api#2706"], "team_inbox": "Re: Planning" }),
        );
        assert_eq!(
            got,
            vec![
                "https://github.com/acme/api/pull/2706".to_string(),
                TEAM_INBOX.to_string()
            ]
        );
    }

    #[test]
    fn ticket_only_with_a_linear_link() {
        set_link_bases(test_bases());
        let item = |key: &str, asks: Value| Item {
            key: key.into(),
            bucket: "now".into(),
            line: String::new(),
            asks,
            status: None,
            done: None,
            added_at: None,
            links: Vec::new(),
        };
        assert!(item("ACME-1", json!({})).has_ticket());
        assert!(!item("supplier-sftp", json!({ "gmail": "1a15d9f8c90f9162" })).has_ticket());
    }

    #[test]
    fn same_gmail_thread_under_two_accounts_is_one_link() {
        set_link_bases(test_bases());
        let item = Item {
            key: "mail".into(),
            bucket: "now".into(),
            line: String::new(),
            asks: json!({ "gmail": "1a15d9f8c90f9162" }),
            status: None,
            done: None,
            added_at: None,
            links: vec![Link {
                label: "Gmail".into(),
                url: "https://mail.google.com/mail/u/0/#all/1a15d9f8c90f9162".into(),
            }],
        };
        assert_eq!(item.all_links().len(), 1);
    }

    #[test]
    fn skill_links_win_over_derived_ones() {
        set_link_bases(test_bases());
        let item = Item {
            key: "ACME-66".into(),
            bucket: "now".into(),
            line: String::new(),
            asks: json!({ "linear": "ACME-66" }),
            status: None,
            done: None,
            added_at: None,
            links: vec![Link {
                label: "Linear ACME-66".into(),
                url: format!("{LINEAR}ACME-66/supplier-feed"),
            }],
        };
        assert_eq!(item.all_links().len(), 1);
    }

    #[test]
    fn bare_url_links_are_labelled() {
        let item: Item = serde_json::from_value(json!({
            "key": "x",
            "links": [
                "https://mail.google.com/mail/u/0/#all/1a15",
                "https://linear.app/acme/issue/ACME-12/some-title",
                "https://github.com/acme/api/pull/2706",
                { "title": "Thread", "url": "https://acme.slack.com/archives/C1/p1" }
            ]
        }))
        .unwrap();
        let labels: Vec<&str> = item.links.iter().map(|link| link.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["Gmail", "Linear ACME-12", "api#2706", "Thread"]
        );
    }

    #[test]
    fn a_bad_item_does_not_drop_the_list() {
        let snapshot: Snapshot = serde_json::from_value(json!({
            "at": "2026-11-04T09:37+01:00",
            "items": [{ "key": "good" }, { "bucket": "now" }, { "key": "also-good", "links": [42] }]
        }))
        .unwrap();
        let keys: Vec<&str> = snapshot
            .items
            .iter()
            .map(|item| item.key.as_str())
            .collect();
        assert_eq!(keys, vec!["good"]);
    }

    #[test]
    fn time_without_seconds() {
        assert!(parse_time("2026-11-04T09:37+01:00").is_some());
        assert!(parse_time("2026-11-04T09:37:12+01:00").is_some());
    }
}
