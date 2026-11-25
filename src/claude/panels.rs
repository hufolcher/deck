//! What the side panels show, read in the background: the repo's changed
//! files (git), your open pull requests (gh), a watched command, and the
//! embedded session as Claude Code registers it.

use crate::config::{Watch, home};
use crate::connectors::shell::{Invocation, Shell, System};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct FileChange {
    /// git's two status letters, e.g. ` M`, `A `, `??`.
    pub code: String,
    pub path: String,
    pub added: Option<u32>,
    pub removed: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<FileChange>,
}

impl Changes {
    /// `git status --porcelain=v1 --branch` and `git diff --numstat HEAD`.
    pub fn parse(status: &str, numstat: &str) -> Changes {
        let mut changes = Changes::default();
        for line in status.lines() {
            if let Some(head) = line.strip_prefix("## ") {
                let (names, counts) = head.split_once(" [").unwrap_or((head, ""));
                changes.branch = names
                    .split("...")
                    .next()
                    .unwrap_or("")
                    .trim_start_matches("No commits yet on ")
                    .to_string();
                for part in counts.trim_end_matches(']').split(", ") {
                    if let Some(n) = part.strip_prefix("ahead ") {
                        changes.ahead = n.parse().unwrap_or(0);
                    } else if let Some(n) = part.strip_prefix("behind ") {
                        changes.behind = n.parse().unwrap_or(0);
                    }
                }
            } else if line.len() > 3 {
                let path = line[3..].rsplit(" -> ").next().unwrap_or("").to_string();
                changes.files.push(FileChange {
                    code: line[..2].to_string(),
                    path,
                    added: None,
                    removed: None,
                });
            }
        }
        for line in numstat.lines() {
            let mut parts = line.splitn(3, '\t');
            let (Some(added), Some(removed), Some(path)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let path = path
                .rsplit(" => ")
                .next()
                .unwrap_or(path)
                .trim_end_matches('}');
            if let Some(file) = changes.files.iter_mut().find(|file| file.path == path) {
                file.added = added.parse().ok();
                file.removed = removed.parse().ok();
            }
        }
        changes
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Review {
    Approved,
    ChangesRequested,
    Required,
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Checks {
    pub failed: u32,
    pub pending: u32,
    pub passed: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pr {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub branch: String,
    pub draft: bool,
    pub conflicts: bool,
    pub review: Review,
    pub checks: Checks,
}

#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum Check {
    CheckRun {
        status: String,
        #[serde(default)]
        conclusion: String,
    },
    StatusContext {
        state: String,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPr {
    number: u64,
    title: String,
    url: String,
    head_ref_name: String,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    review_decision: Option<String>,
    #[serde(default)]
    merge_state_status: Option<String>,
    #[serde(default)]
    status_check_rollup: Vec<Check>,
}

pub const GH_FIELDS: &str =
    "number,title,url,headRefName,isDraft,reviewDecision,mergeStateStatus,statusCheckRollup";

/// `gh pr list --json GH_FIELDS`, the current branch's pull request first.
pub fn parse_prs(json: &str, branch: &str) -> Result<Vec<Pr>, String> {
    let listed: Vec<GhPr> = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let mut prs: Vec<Pr> = listed
        .into_iter()
        .map(|pr| {
            let mut checks = Checks::default();
            for check in &pr.status_check_rollup {
                let counter = match check {
                    Check::CheckRun { status, .. } if status != "COMPLETED" => &mut checks.pending,
                    Check::CheckRun { conclusion, .. }
                        if matches!(conclusion.as_str(), "SUCCESS" | "NEUTRAL" | "SKIPPED") =>
                    {
                        &mut checks.passed
                    }
                    Check::CheckRun { .. } => &mut checks.failed,
                    Check::StatusContext { state } => match state.as_str() {
                        "SUCCESS" => &mut checks.passed,
                        "PENDING" | "EXPECTED" => &mut checks.pending,
                        _ => &mut checks.failed,
                    },
                };
                *counter += 1;
            }
            Pr {
                number: pr.number,
                title: pr.title,
                url: pr.url,
                draft: pr.is_draft,
                conflicts: pr.merge_state_status.as_deref() == Some("DIRTY"),
                review: match pr.review_decision.as_deref() {
                    Some("APPROVED") => Review::Approved,
                    Some("CHANGES_REQUESTED") => Review::ChangesRequested,
                    Some("REVIEW_REQUIRED") => Review::Required,
                    _ => Review::None,
                },
                checks,
                branch: pr.head_ref_name,
            }
        })
        .collect();
    prs.sort_by_key(|pr| pr.branch != branch);
    Ok(prs)
}

/// The embedded session, from `~/.claude/sessions/<pid>.json`.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub session_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
    /// Milliseconds since the epoch.
    #[serde(default)]
    pub started_at: i64,
}

impl Session {
    pub fn path(pid: u32) -> PathBuf {
        home().join(format!(".claude/sessions/{pid}.json"))
    }

    pub fn read(path: &Path) -> Option<Session> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }
}

/// Text without the colour and cursor escapes a command printed.
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            // CSI: parameters up to a final letter.
            Some('[') => {
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() || next == '~' {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ESC \.
            Some(']') => {
                while let Some(next) = chars.next() {
                    if next == '\x07' || (next == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            // Character set: one more byte.
            Some('(' | ')') => {
                chars.next();
            }
            _ => {}
        }
    }
    out
}

pub enum Update {
    Changes(Result<Changes, String>),
    Prs(Result<Vec<Pr>, String>),
    Watch(Result<Vec<String>, String>),
}

fn run<S: Shell>(shell: &S, invocation: Invocation) -> Result<String, String> {
    let output = shell.run(&invocation).map_err(|error| error.to_string())?;
    if output.success {
        Ok(output.stdout)
    } else {
        Err(output.reason())
    }
}

pub fn changes<S: Shell>(shell: &S, cwd: &Path) -> Result<Changes, String> {
    let git = |args: &[&str]| {
        Invocation::new("git")
            .args(args.iter().copied())
            .cwd(cwd.into())
    };
    let status = run(shell, git(&["status", "--porcelain=v1", "--branch"]))?;
    // No HEAD yet in a new repo: the counts stay empty.
    let numstat = run(shell, git(&["diff", "--numstat", "HEAD"])).unwrap_or_default();
    Ok(Changes::parse(&status, &numstat))
}

pub fn prs<S: Shell>(shell: &S, cwd: &Path, branch: &str) -> Result<Vec<Pr>, String> {
    let listed = run(
        shell,
        Invocation::new("gh")
            .args([
                "pr", "list", "--author", "@me", "--state", "open", "--json", GH_FIELDS,
            ])
            .cwd(cwd.into()),
    )?;
    parse_prs(&listed, branch)
}

pub fn watch<S: Shell>(shell: &S, cwd: &Path, command: &str) -> Result<Vec<String>, String> {
    let output = run(
        shell,
        Invocation::new("sh").args(["-c", command]).cwd(cwd.into()),
    )?;
    Ok(plain(&output).lines().map(str::to_string).collect())
}

/// One thread per panel, each at its own pace; they stop with the app.
pub fn spawn(cwd: PathBuf, watched: Watch, tx: Sender<Update>) {
    let files_tx = tx.clone();
    let files_cwd = cwd.clone();
    std::thread::spawn(move || {
        loop {
            if files_tx
                .send(Update::Changes(changes(&System, &files_cwd)))
                .is_err()
            {
                return;
            }
            std::thread::sleep(Duration::from_secs(3));
        }
    });
    let prs_tx = tx.clone();
    let prs_cwd = cwd.clone();
    std::thread::spawn(move || {
        loop {
            let branch = changes(&System, &prs_cwd)
                .map(|c| c.branch)
                .unwrap_or_default();
            if prs_tx
                .send(Update::Prs(prs(&System, &prs_cwd, &branch)))
                .is_err()
            {
                return;
            }
            std::thread::sleep(Duration::from_secs(60));
        }
    });
    if watched.command.trim().is_empty() {
        return;
    }
    std::thread::spawn(move || {
        loop {
            if tx
                .send(Update::Watch(watch(&System, &cwd, &watched.command)))
                .is_err()
            {
                return;
            }
            std::thread::sleep(Duration::from_secs(u64::from(watched.every_seconds.max(1))));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::shell::fake::Fake;

    #[test]
    fn status_and_numstat_make_the_changes() {
        let changes = Changes::parse(
            "## feature...origin/feature [ahead 2, behind 1]\n M src/app.rs\nA  src/new.rs\nR  old.rs -> moved.rs\n?? notes.md\n",
            "3\t1\tsrc/app.rs\n10\t0\tsrc/new.rs\n0\t0\t{old.rs => moved.rs}\n",
        );
        assert_eq!(changes.branch, "feature");
        assert_eq!((changes.ahead, changes.behind), (2, 1));
        let paths: Vec<&str> = changes.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["src/app.rs", "src/new.rs", "moved.rs", "notes.md"]);
        assert_eq!(changes.files[0].added, Some(3));
        assert_eq!(changes.files[0].removed, Some(1));
        assert_eq!(changes.files[3].added, None);
    }

    #[test]
    fn a_branch_without_upstream_or_commits() {
        assert_eq!(Changes::parse("## main\n", "").branch, "main");
        assert_eq!(
            Changes::parse("## No commits yet on main\n", "").branch,
            "main"
        );
    }

    #[test]
    fn prs_summarised_current_branch_first() {
        let json = r#"[
          {"number": 7, "title": "Other", "url": "u7", "headRefName": "other", "isDraft": true,
           "reviewDecision": "", "mergeStateStatus": "DIRTY", "statusCheckRollup": []},
          {"number": 9, "title": "Mine", "url": "u9", "headRefName": "feature", "isDraft": false,
           "reviewDecision": "CHANGES_REQUESTED", "mergeStateStatus": "CLEAN", "statusCheckRollup": [
             {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "FAILURE"},
             {"__typename": "CheckRun", "status": "IN_PROGRESS", "conclusion": ""},
             {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SKIPPED"},
             {"__typename": "StatusContext", "state": "SUCCESS"}]}]"#;
        let prs = parse_prs(json, "feature").unwrap();
        assert_eq!(prs[0].number, 9);
        assert_eq!(prs[0].review, Review::ChangesRequested);
        assert_eq!(
            prs[0].checks,
            Checks {
                failed: 1,
                pending: 1,
                passed: 2
            }
        );
        assert!(prs[1].draft && prs[1].conflicts);
        assert_eq!(prs[1].review, Review::None);
    }

    #[test]
    fn gh_refusing_says_why() {
        let shell = Fake::default().fail("gh pr list", "no git remotes found");
        assert_eq!(
            prs(&shell, Path::new("/repo"), "main"),
            Err("no git remotes found".into())
        );
    }

    #[test]
    fn watched_output_loses_its_escapes() {
        assert_eq!(plain("\x1b[1;32mfree\x1b[0m ok\x1b(B"), "free ok");
        assert_eq!(plain("\x1b]0;title\x07up"), "up");
    }

    #[test]
    fn session_file_read() {
        let file = std::env::temp_dir().join(format!("deck-session-{}.json", std::process::id()));
        std::fs::write(
            &file,
            r#"{"pid": 1, "sessionId": "abc", "name": "x", "status": "busy", "startedAt": 5, "cwd": "/"}"#,
        )
        .unwrap();
        let session = Session::read(&file).unwrap();
        std::fs::remove_file(&file).unwrap();
        assert_eq!(
            (session.session_id.as_str(), session.status.as_str()),
            ("abc", "busy")
        );
        assert_eq!(Session::read(&file), None);
    }
}
