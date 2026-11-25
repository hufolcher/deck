//! GitHub: a pull request's state, checks and review, from the REST API.
//! The token is `DECK_GITHUB_TOKEN`, the keychain item `deck-github`,
//! or the `gh` CLI's own login.

use super::http::{self, Http, Scheme, Token};
use super::shell::{Invocation, Shell};
use super::{Connector, Error, ItemRef};
use crate::snapshot::Item;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

/// `owner/repo#n`, as the skill's `pr` asks carry it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PullRef {
    pub repo: String,
    pub number: u32,
}

impl FromStr for PullRef {
    type Err = ();

    fn from_str(text: &str) -> Result<PullRef, ()> {
        let (repo, number) = text.trim().split_once('#').ok_or(())?;
        let (owner, name) = repo.split_once('/').ok_or(())?;
        if owner.is_empty() || name.is_empty() {
            return Err(());
        }
        Ok(PullRef {
            repo: repo.to_string(),
            number: number.parse().map_err(|_| ())?,
        })
    }
}

impl std::fmt::Display for PullRef {
    /// `repo#n`, without the owner.
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        let name = self.repo.rsplit('/').next().unwrap_or(&self.repo);
        write!(formatter, "{name}#{}", self.number)
    }
}

impl ItemRef for PullRef {
    fn of(item: &Item) -> Vec<PullRef> {
        item.asks_of("pr")
            .iter()
            .filter_map(|text| text.parse().ok())
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PullState {
    Open,
    Merged,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Review {
    Approved,
    ChangesRequested,
    /// Reviewers asked, none answered yet.
    Required,
    None,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Word {
    Completed,
    Success,
    Neutral,
    Skipped,
    Failure,
    Error,
    TimedOut,
    Cancelled,
    ActionRequired,
    StartupFailure,
    Pending,
    #[serde(other)]
    Other,
}

/// A check run (status + conclusion) or a commit status (state).
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
struct Check {
    status: Option<Word>,
    conclusion: Option<Word>,
    state: Option<Word>,
}

impl Check {
    fn failed(self) -> bool {
        use Word::*;
        matches!(
            self.conclusion,
            Some(Failure | TimedOut | Cancelled | ActionRequired | StartupFailure)
        ) || matches!(self.state, Some(Failure | Error))
    }

    fn pending(self) -> bool {
        self.status.is_some_and(|status| status != Word::Completed)
            || self.state == Some(Word::Pending)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Checks {
    None,
    Pending,
    Passing,
    Failing,
}

impl Checks {
    fn of(checks: &[Check]) -> Checks {
        if checks.is_empty() {
            Checks::None
        } else if checks.iter().any(|check| check.failed()) {
            Checks::Failing
        } else if checks.iter().any(|check| check.pending()) {
            Checks::Pending
        } else {
            Checks::Passing
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum OpenOrClosed {
    Open,
    Closed,
}

#[derive(Deserialize)]
struct Head {
    sha: String,
}

#[derive(Deserialize)]
struct PullAnswer {
    state: OpenOrClosed,
    #[serde(default)]
    draft: bool,
    merged_at: Option<String>,
    head: Head,
    #[serde(default)]
    requested_reviewers: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

#[derive(Deserialize)]
struct ReviewAnswer {
    user: Option<Login>,
    state: String,
}

#[derive(Deserialize)]
struct CheckRuns {
    check_runs: Vec<Check>,
}

#[derive(Deserialize)]
struct CombinedStatus {
    #[serde(default)]
    statuses: Vec<Check>,
}

/// GitHub answers an error as `{"message": …}` with the same shape for every
/// call: a missing pull request, a bad token.
#[derive(Deserialize)]
#[serde(untagged)]
enum Answer<T> {
    Found(T),
    Refused { message: String },
}

impl Review {
    /// Each reviewer's latest verdict counts; a change request outweighs
    /// approvals, as GitHub's own decision does.
    fn of(reviews: &[ReviewAnswer], requested: bool) -> Review {
        let mut latest: BTreeMap<&str, &str> = BTreeMap::new();
        for review in reviews {
            if matches!(
                review.state.as_str(),
                "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED"
            ) {
                let by = review.user.as_ref().map_or("", |user| user.login.as_str());
                latest.insert(by, &review.state);
            }
        }
        if latest.values().any(|state| *state == "CHANGES_REQUESTED") {
            Review::ChangesRequested
        } else if latest.values().any(|state| *state == "APPROVED") {
            Review::Approved
        } else if requested {
            Review::Required
        } else {
            Review::None
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pull {
    pub state: PullState,
    pub draft: bool,
    pub review: Review,
    pub checks: Checks,
}

pub struct GitHub<S: Shell> {
    shell: S,
    api: String,
}

impl<S: Shell> GitHub<S> {
    pub fn new(shell: S, api: &str) -> GitHub<S> {
        GitHub {
            shell,
            api: api.trim_end_matches('/').into(),
        }
    }

    fn token(&self) -> Result<Token, Error> {
        http::token(&self.shell, Self::NAME).or_else(|_| {
            let output = self
                .shell
                .run(&Invocation::new("gh").args(["auth", "token"]))
                .ok()
                .filter(|output| output.success)
                .ok_or(Error::NotConnected(Self::NAME))?;
            Token::new(output.stdout.trim()).ok_or(Error::NotConnected(Self::NAME))
        })
    }

    /// Some(answer), None when GitHub says there is no such thing.
    fn get<T: DeserializeOwned>(&self, token: &Token, path: &str) -> Result<Option<T>, Error> {
        let answer: Answer<T> =
            Http::new(&self.shell).get(&format!("{}/{path}", self.api), token, Scheme::Bearer)?;
        match answer {
            Answer::Found(found) => Ok(Some(found)),
            Answer::Refused { message } if message == "Not Found" => Ok(None),
            Answer::Refused { message } if message.contains("credentials") => {
                Err(Error::NotConnected(Self::NAME))
            }
            Answer::Refused { message } => Err(Error::Api(message)),
        }
    }

    fn pull(&self, token: &Token, pull: &PullRef) -> Result<Option<Pull>, Error> {
        let base = format!("repos/{}", pull.repo);
        let Some(answer) =
            self.get::<PullAnswer>(token, &format!("{base}/pulls/{}", pull.number))?
        else {
            return Ok(None);
        };
        let reviews: Vec<ReviewAnswer> = self
            .get(
                token,
                &format!("{base}/pulls/{}/reviews?per_page=100", pull.number),
            )?
            .unwrap_or_default();
        let sha = &answer.head.sha;
        let mut checks: Vec<Check> = self
            .get::<CheckRuns>(
                token,
                &format!("{base}/commits/{sha}/check-runs?per_page=100"),
            )?
            .map(|runs| runs.check_runs)
            .unwrap_or_default();
        if let Some(combined) =
            self.get::<CombinedStatus>(token, &format!("{base}/commits/{sha}/status"))?
        {
            checks.extend(combined.statuses);
        }
        Ok(Some(Pull {
            state: match (answer.state, answer.merged_at) {
                (_, Some(_)) => PullState::Merged,
                (OpenOrClosed::Open, None) => PullState::Open,
                (OpenOrClosed::Closed, None) => PullState::Closed,
            },
            draft: answer.draft,
            review: Review::of(&reviews, !answer.requested_reviewers.is_empty()),
            checks: Checks::of(&checks),
        }))
    }
}

impl<S: Shell> Connector for GitHub<S> {
    type Ref = PullRef;
    type Status = Pull;
    const NAME: &'static str = "github";

    fn fetch(&self, pulls: &[PullRef]) -> Result<HashMap<PullRef, Pull>, Error> {
        let token = self.token()?;
        let mut found = HashMap::new();
        for pull in pulls {
            if let Some(status) = self.pull(&token, pull)? {
                found.insert(pull.clone(), status);
            }
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::super::shell::fake::Fake;
    use super::*;
    use serde_json::json;

    const API: &str = "https://api.github.com";

    fn pull(text: &str) -> PullRef {
        text.parse().unwrap()
    }

    /// A pull request whose answers are given; the rest is empty.
    fn github(
        pull_answer: serde_json::Value,
        reviews: serde_json::Value,
        runs: serde_json::Value,
    ) -> Fake {
        Fake::default()
            .answer("deck-github", "ghp_test")
            .answer("/reviews", &reviews.to_string())
            .answer("/check-runs", &json!({ "check_runs": runs }).to_string())
            .answer("/status", r#"{"state": "pending", "statuses": []}"#)
            .answer("/pulls/", &pull_answer.to_string())
    }

    fn open() -> serde_json::Value {
        json!({ "state": "open", "draft": false, "merged_at": null, "head": { "sha": "abc" }, "requested_reviewers": [] })
    }

    fn fetch(fake: &Fake) -> Pull {
        GitHub::new(fake, API).fetch(&[pull("acme/api#1")]).unwrap()[&pull("acme/api#1")].clone()
    }

    #[test]
    fn checks_fail_before_pending_before_pass() {
        let run = |status: &str, conclusion: Option<&str>| json!({ "status": status, "conclusion": conclusion });
        let checks = |runs| fetch(&github(open(), json!([]), runs)).checks;
        assert_eq!(checks(json!([])), Checks::None);
        assert_eq!(
            checks(json!([run("completed", Some("success"))])),
            Checks::Passing
        );
        assert_eq!(
            checks(json!([
                run("completed", Some("success")),
                run("in_progress", None)
            ])),
            Checks::Pending
        );
        assert_eq!(
            checks(json!([
                run("queued", None),
                run("completed", Some("failure"))
            ])),
            Checks::Failing
        );
        // The first matching rule answers: the failing status comes first.
        let fake = Fake::default()
            .answer(
                "/status",
                r#"{"state": "failure", "statuses": [{"state": "error"}]}"#,
            )
            .answer("deck-github", "ghp_test")
            .answer("/reviews", "[]")
            .answer("/check-runs", r#"{"check_runs": []}"#)
            .answer("/pulls/", &open().to_string());
        assert_eq!(fetch(&fake).checks, Checks::Failing);
    }

    #[test]
    fn latest_review_per_person_decides() {
        let review = |by: &str, state: &str| json!({ "user": { "login": by }, "state": state });
        let decided = |reviews, requested: bool| {
            let mut answer = open();
            answer["requested_reviewers"] = if requested {
                json!([{ "login": "ann" }])
            } else {
                json!([])
            };
            fetch(&github(answer, reviews, json!([]))).review
        };
        assert_eq!(decided(json!([]), false), Review::None);
        assert_eq!(decided(json!([]), true), Review::Required);
        assert_eq!(
            decided(json!([review("ann", "APPROVED")]), false),
            Review::Approved
        );
        assert_eq!(
            decided(
                json!([
                    review("ann", "CHANGES_REQUESTED"),
                    review("bob", "APPROVED")
                ]),
                false
            ),
            Review::ChangesRequested
        );
        assert_eq!(
            decided(
                json!([
                    review("ann", "CHANGES_REQUESTED"),
                    review("ann", "COMMENTED"),
                    review("ann", "APPROVED")
                ]),
                false
            ),
            Review::Approved,
            "a later approval replaces the change request; a comment changes nothing"
        );
    }

    #[test]
    fn merged_draft_and_the_right_calls() {
        let merged = json!({ "state": "closed", "draft": true, "merged_at": "2026-11-25T10:00:00Z", "head": { "sha": "f00" } });
        let fake = github(merged, json!([]), json!([]));
        let status = GitHub::new(&fake, "https://ghe.example/api/v3/")
            .fetch(&[pull("acme/api#7")])
            .unwrap();
        assert_eq!(
            status[&pull("acme/api#7")],
            Pull {
                state: PullState::Merged,
                draft: true,
                review: Review::None,
                checks: Checks::None
            }
        );
        assert!(fake.ran("https://ghe.example/api/v3/repos/acme/api/pulls/7"));
        assert!(fake.ran("https://ghe.example/api/v3/repos/acme/api/commits/f00/check-runs"));
        assert!(
            !fake.ran("ghp_test"),
            "the token stays off the command line"
        );
    }

    #[test]
    fn missing_pull_skipped_bad_token_refused() {
        let missing = Fake::default()
            .answer("deck-github", "ghp_test")
            .answer("/pulls/", r#"{"message": "Not Found", "status": "404"}"#);
        assert!(
            GitHub::new(&missing, API)
                .fetch(&[pull("acme/api#9")])
                .unwrap()
                .is_empty()
        );
        let refused = Fake::default().answer("deck-github", "ghp_old").answer(
            "/pulls/",
            r#"{"message": "Bad credentials", "status": "401"}"#,
        );
        assert_eq!(
            GitHub::new(&refused, API).fetch(&[pull("acme/api#9")]),
            Err(Error::NotConnected("github"))
        );
    }

    #[test]
    fn token_falls_back_to_the_gh_login() {
        let fake = Fake::default()
            .answer("gh auth token", "gho_cli\n")
            .answer("/pulls/", r#"{"message": "Not Found"}"#);
        assert!(
            GitHub::new(&fake, API)
                .fetch(&[pull("acme/api#9")])
                .unwrap()
                .is_empty()
        );
        assert!(fake.ran("gh auth token"));
        let nothing = Fake::default();
        assert_eq!(
            GitHub::new(&nothing, API).fetch(&[pull("acme/api#9")]),
            Err(Error::NotConnected("github"))
        );
    }

    #[test]
    fn refs_parse_and_show_short() {
        assert_eq!(pull("acme/api#12").to_string(), "api#12");
        assert!("api#12".parse::<PullRef>().is_err());
        assert!("acme/api#x".parse::<PullRef>().is_err());
        let item: Item = serde_json::from_value(
            json!({ "key": "k", "asks": { "pr": ["acme/api#1", "acme/web#2"] } }),
        )
        .unwrap();
        assert_eq!(
            PullRef::of(&item),
            vec![pull("acme/api#1"), pull("acme/web#2")]
        );
    }
}
