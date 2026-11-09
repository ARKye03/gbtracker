mod cli;
mod write;

use std::path::Path;

use serde::{Deserialize, Serialize};

pub use write::{
    NewIssue, NewKind, StateTarget, WriteOp, apply, close, comment, create, reopen, set_state,
};

#[derive(Debug, thiserror::Error)]
pub enum GitBugError {
    #[error("failed to run git-bug: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("git-bug locked by another process{}", pid_suffix(*.pid))]
    Locked { pid: Option<u32> },
    #[error("git-bug {command} failed: {stderr}")]
    Failed { command: String, stderr: String },
    #[error("git-bug {command} aborted: {stderr}")]
    Aborted { command: String, stderr: String },
    #[error("invalid bug id {0:?}: expected 7-64 hex chars")]
    InvalidId(String),
    #[error("git-bug returned bug {got} for requested id {requested}")]
    IdMismatch { requested: String, got: String },
    #[error("issue {0} is closed; reopen it first")]
    Closed(String),
    #[error("comment is empty")]
    EmptyComment,
    #[error("title is empty")]
    EmptyTitle,
    #[error("unexpected output from git-bug bug new: {0:?}")]
    UnexpectedOutput(String),
    #[error("created {id} but could not label it: {cause}")]
    Unlabelled { id: String, cause: String },
    #[error("unexpected git-bug output: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("git-bug returned no comments for {0}")]
    MissingBody(String),
}

fn pid_suffix(pid: Option<u32>) -> String {
    pid.map(|p| format!(" (pid {p})")).unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Epic,
    Task,
    Bug,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    InProgress,
    Blocked,
    Done,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Open,
    Closed,
}

impl Kind {
    fn from_label(value: &str) -> Self {
        match value {
            "epic" => Kind::Epic,
            "task" => Kind::Task,
            "bug" => Kind::Bug,
            other => Kind::Other(other.to_owned()),
        }
    }
}

impl State {
    fn from_label(value: &str) -> Self {
        match value {
            "in-progress" => State::InProgress,
            "blocked" => State::Blocked,
            "done" => State::Done,
            other => State::Other(other.to_owned()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub id: String,
    pub human_id: String,
    pub title: String,
    pub status: Status,
    pub kind: Option<Kind>,
    pub epic: Option<String>,
    pub state: Option<State>,
    pub layer: Option<u32>,
    pub labels: Vec<String>,
    pub author: String,
    pub created: String,
    pub edited: String,
    /// Includes the body, which git-bug stores as the first comment.
    pub comment_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub human_id: String,
    pub author: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueDetail {
    pub issue: Issue,
    pub body: String,
    pub comments: Vec<Comment>,
}

/// git-bug prints `"labels": null` for an issue that never had one.
fn null_as_empty<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<Vec<String>>::deserialize(deserializer).map(Option::unwrap_or_default)
}

#[derive(Deserialize)]
struct RawTime {
    time: String,
}

#[derive(Deserialize)]
struct RawAuthor {
    name: String,
}

#[derive(Deserialize)]
struct RawHead {
    id: String,
    human_id: String,
    create_time: RawTime,
    edit_time: RawTime,
    status: Status,
    #[serde(default, deserialize_with = "null_as_empty")]
    labels: Vec<String>,
    title: String,
    author: RawAuthor,
}

#[derive(Deserialize)]
struct RawRow {
    #[serde(flatten)]
    head: RawHead,
    comments: usize,
}

#[derive(Deserialize)]
struct RawComment {
    id: String,
    human_id: String,
    author: RawAuthor,
    message: String,
}

#[derive(Deserialize)]
struct RawShow {
    #[serde(flatten)]
    head: RawHead,
    comments: Vec<RawComment>,
}

fn label_value<'a>(labels: &'a [String], prefix: &str) -> Option<&'a str> {
    labels.iter().find_map(|l| l.strip_prefix(prefix))
}

/// `L<digits> — <title>`; the separator is an em dash.
fn parse_layer(title: &str) -> Option<u32> {
    let rest = title.strip_prefix('L')?;
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    if end == 0 || !rest[end..].starts_with(" — ") {
        return None;
    }
    rest[..end].parse().ok()
}

impl Issue {
    fn from_head(head: RawHead, comment_count: usize) -> Self {
        Issue {
            kind: label_value(&head.labels, "kind:").map(Kind::from_label),
            epic: label_value(&head.labels, "epic:").map(str::to_owned),
            state: label_value(&head.labels, "state:").map(State::from_label),
            layer: parse_layer(&head.title),
            id: head.id,
            human_id: head.human_id,
            title: head.title,
            status: head.status,
            labels: head.labels,
            author: head.author.name,
            created: head.create_time.time,
            edited: head.edit_time.time,
            comment_count,
        }
    }
}

/// Layered issues first, ascending by layer number; the rest keep their input order.
pub fn parse_list(json: &str) -> Result<Vec<Issue>, GitBugError> {
    let rows: Vec<RawRow> = serde_json::from_str(json)?;
    let mut issues: Vec<Issue> = rows
        .into_iter()
        .map(|r| Issue::from_head(r.head, r.comments))
        .collect();
    issues.sort_by_key(|i| i.layer.unwrap_or(u32::MAX));
    Ok(issues)
}

pub fn parse_show(json: &str) -> Result<IssueDetail, GitBugError> {
    let raw: RawShow = serde_json::from_str(json)?;
    let total = raw.comments.len();
    let mut comments = raw.comments.into_iter();
    let body = comments
        .next()
        .ok_or_else(|| GitBugError::MissingBody(raw.head.id.clone()))?
        .message;
    let comments = comments
        .map(|c| Comment {
            id: c.id,
            human_id: c.human_id,
            author: c.author.name,
            message: c.message,
        })
        .collect();
    Ok(IssueDetail {
        issue: Issue::from_head(raw.head, total),
        body,
        comments,
    })
}

pub async fn list(root: &Path) -> Result<Vec<Issue>, GitBugError> {
    let out = cli::run(
        root,
        &[
            "bug",
            "-f",
            "json",
            "--by",
            "creation",
            "--direction",
            "asc",
        ],
    )
    .await?;
    parse_list(&out)
}

pub async fn show(root: &Path, full_id: &str) -> Result<IssueDetail, GitBugError> {
    let valid = (7..=64).contains(&full_id.len()) && full_id.bytes().all(|b| b.is_ascii_hexdigit());
    if !valid {
        return Err(GitBugError::InvalidId(full_id.to_owned()));
    }
    let out = cli::run(root, &["bug", "show", full_id, "-f", "json"]).await?;
    let detail = parse_show(&out)?;
    // A prefix that matches nothing makes git-bug fall back to its last selected bug.
    if !detail.issue.id.starts_with(&full_id.to_ascii_lowercase()) {
        return Err(GitBugError::IdMismatch {
            requested: full_id.to_owned(),
            got: detail.issue.id,
        });
    }
    Ok(detail)
}

/// The identity git-bug acts as in a repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

pub async fn current_identity(root: &Path) -> Result<Identity, GitBugError> {
    let name = cli::run(root, &["user", "show", "--field", "name"]).await?;
    let email = cli::run(root, &["user", "show", "--field", "email"]).await?;
    Ok(Identity {
        name: name.trim().to_owned(),
        email: email.trim().to_owned(),
    })
}

/// Up to two letters: first letters of the first two words, or the first two letters of a single word.
pub fn initials(name: &str) -> String {
    let mut words = name.split_whitespace();
    let letters: String = match (words.next(), words.next()) {
        (Some(a), Some(b)) => a.chars().take(1).chain(b.chars().take(1)).collect(),
        (Some(a), None) => a.chars().take(2).collect(),
        _ => String::new(),
    };
    if letters.is_empty() {
        "?".to_owned()
    } else {
        letters.to_uppercase()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::Duration;

    use super::cli::{interpret, retry_locked};
    use super::*;

    const LIST: &str = include_str!("../../tests/fixtures/list.json");
    const SHOW: &str = include_str!("../../tests/fixtures/show.json");

    #[test]
    fn list_extracts_labels_and_layers() {
        let issues = parse_list(LIST).unwrap();
        let l1 = issues.iter().find(|i| i.human_id == "490a542").unwrap();
        assert_eq!(l1.kind, Some(Kind::Task));
        assert_eq!(l1.state, Some(State::InProgress));
        assert_eq!(l1.layer, Some(1));
        assert_eq!(l1.status, Status::Open);
        assert_eq!(l1.author, "ARKye03");
        assert_eq!(l1.comment_count, 2);
        assert!(l1.epic.as_deref().unwrap().starts_with("e90c710"));
        assert_eq!(l1.created, "2026-11-08T18:19:24-03:00");

        let epic = issues.iter().find(|i| i.human_id == "e90c710").unwrap();
        assert_eq!(epic.kind, Some(Kind::Epic));
        assert_eq!(epic.epic, None);
        assert_eq!(epic.state, None);
        assert_eq!(epic.layer, None);

        let bug = issues.iter().find(|i| i.human_id == "b0b0b0b").unwrap();
        assert_eq!(bug.kind, Some(Kind::Bug));
        assert_eq!(bug.state, Some(State::Blocked));
        assert_eq!(bug.status, Status::Closed);
    }

    #[test]
    fn null_labels_parse_as_empty() {
        let mut rows: serde_json::Value = serde_json::from_str(LIST).unwrap();
        rows[0]["labels"] = serde_json::Value::Null;
        let issues = parse_list(&rows.to_string()).unwrap();
        assert!(issues.iter().any(|i| i.labels.is_empty()));
    }

    #[test]
    fn initials_from_name() {
        assert_eq!(initials("Ada Lovelace"), "AL");
        assert_eq!(initials("  ada   king lovelace "), "AK");
        assert_eq!(initials("ARKye03"), "AR");
        assert_eq!(initials("x"), "X");
        assert_eq!(initials(""), "?");
        assert_eq!(initials("  \n"), "?");
    }

    #[test]
    fn list_orders_layered_first_by_number() {
        let titles: Vec<_> = parse_list(LIST)
            .unwrap()
            .into_iter()
            .map(|i| i.title)
            .collect();
        assert_eq!(
            titles,
            [
                "L1 — git-bug read adapter and issue model",
                "L2 — Workspace config and git-bug repo discovery",
                "L10 — Late layer",
                "gbtracker: multi-repo git-bug manager",
                "Loose bug without layer",
            ]
        );
    }

    #[test]
    fn layer_requires_digits_and_separator() {
        assert_eq!(parse_layer("L12 — x"), Some(12));
        assert_eq!(parse_layer("L — x"), None);
        assert_eq!(parse_layer("L1 - x"), None);
        assert_eq!(parse_layer("L1"), None);
        assert_eq!(parse_layer("Launch — x"), None);
    }

    #[test]
    fn show_splits_body_from_comments() {
        let d = parse_show(SHOW).unwrap();
        assert!(d.body.starts_with("## Context"));
        assert_eq!(d.comments.len(), 1);
        assert_eq!(d.comments[0].author, "ARKye03");
        assert_eq!(d.comments[0].message, "Picked up by the runner.");
        assert_eq!(d.issue.layer, Some(1));
        assert_eq!(d.issue.kind, Some(Kind::Task));
        assert_eq!(d.issue.comment_count, 2);
    }

    #[test]
    fn show_without_comments_is_an_error() {
        let mut value: serde_json::Value = serde_json::from_str(SHOW).unwrap();
        value["comments"] = serde_json::json!([]);
        assert!(matches!(
            parse_show(&value.to_string()),
            Err(GitBugError::MissingBody(_))
        ));
    }

    #[test]
    fn detail_roundtrips_through_serde() {
        let d = parse_show(SHOW).unwrap();
        let back: IssueDetail = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn lock_stderr_maps_to_locked() {
        let err = interpret(
            "bug",
            false,
            "Building cache ... done\nrepository is already locked by the process with pid 4242",
            String::new(),
        )
        .unwrap_err();
        assert!(matches!(err, GitBugError::Locked { pid: Some(4242) }));
    }

    #[test]
    fn aborting_on_exit_zero_is_an_error() {
        let err = interpret(
            "bug comment",
            true,
            "Aborting due to empty message",
            String::new(),
        )
        .unwrap_err();
        assert!(matches!(err, GitBugError::Aborted { .. }));
    }

    #[test]
    fn failure_drops_cache_noise_and_clean_run_passes_stdout() {
        let err = interpret(
            "bug show",
            false,
            "Building cache ... 1/2\nno bug found",
            String::new(),
        )
        .unwrap_err();
        match err {
            GitBugError::Failed { stderr, .. } => assert_eq!(stderr, "no bug found"),
            other => panic!("{other:?}"),
        }
        let ok = interpret("bug", true, "Building cache ... done", "[]".into()).unwrap();
        assert_eq!(ok, "[]");
    }

    #[tokio::test]
    async fn locked_is_retried_then_succeeds() {
        let calls = Cell::new(0);
        let out = retry_locked(5, Duration::ZERO, || {
            calls.set(calls.get() + 1);
            let n = calls.get();
            async move {
                if n < 3 {
                    Err(GitBugError::Locked { pid: Some(1) })
                } else {
                    Ok("ok".to_owned())
                }
            }
        })
        .await;
        assert_eq!(out.unwrap(), "ok");
        assert_eq!(calls.get(), 3);
    }

    #[tokio::test]
    async fn locked_gives_up_after_retry_budget() {
        let calls = Cell::new(0);
        let out = retry_locked(5, Duration::ZERO, || {
            calls.set(calls.get() + 1);
            async { Err(GitBugError::Locked { pid: None }) }
        })
        .await;
        assert!(matches!(out, Err(GitBugError::Locked { .. })));
        assert_eq!(calls.get(), 6);
    }

    #[tokio::test]
    async fn show_rejects_non_hex_ids() {
        let err = show(Path::new("."), "--help").await.unwrap_err();
        assert!(matches!(err, GitBugError::InvalidId(_)));
    }
}
