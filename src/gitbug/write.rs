//! Write actions with the same semantics as the roadmap plugin's `gb` script, so the app never disagrees with it.
//! Each action reads the issue first, sends only the label diff (git-bug exits 1 otherwise) and returns the re-read issue.

use std::path::Path;

use super::{GitBugError, Issue, IssueDetail, Status, cli, show};

const STATE_PREFIX: &str = "state:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateTarget {
    InProgress,
    Blocked,
    Clear,
}

impl StateTarget {
    fn label(self) -> Option<&'static str> {
        match self {
            StateTarget::InProgress => Some("state:in-progress"),
            StateTarget::Blocked => Some("state:blocked"),
            StateTarget::Clear => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOp {
    Close,
    Reopen,
    State(StateTarget),
    Comment(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    RemoveLabels(Vec<String>),
    AddLabel(String),
    SetStatus(Status),
}

impl Step {
    fn args(&self, id: &str) -> Vec<String> {
        let own = |parts: &[&str]| parts.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        match self {
            Step::RemoveLabels(labels) => {
                let mut args = own(&["bug", "label", "rm", id]);
                args.extend(labels.iter().cloned());
                args
            }
            Step::AddLabel(label) => {
                let mut args = own(&["bug", "label", "new", id]);
                args.push(label.clone());
                args
            }
            Step::SetStatus(Status::Closed) => own(&["bug", "status", "close", id]),
            Step::SetStatus(Status::Open) => own(&["bug", "status", "open", id]),
        }
    }
}

fn state_labels(issue: &Issue) -> Vec<String> {
    issue
        .labels
        .iter()
        .filter(|l| l.starts_with(STATE_PREFIX))
        .cloned()
        .collect()
}

fn plan_close(issue: &Issue) -> Vec<Step> {
    if issue.status == Status::Closed {
        return Vec::new();
    }
    let stale = state_labels(issue);
    let mut steps = Vec::new();
    if !stale.is_empty() {
        steps.push(Step::RemoveLabels(stale));
    }
    steps.push(Step::SetStatus(Status::Closed));
    steps
}

fn plan_reopen(issue: &Issue) -> Vec<Step> {
    if issue.status == Status::Closed {
        vec![Step::SetStatus(Status::Open)]
    } else {
        Vec::new()
    }
}

fn plan_state(issue: &Issue, target: StateTarget) -> Result<Vec<Step>, GitBugError> {
    if issue.status == Status::Closed {
        return Err(GitBugError::Closed(issue.human_id.clone()));
    }
    let want = target.label();
    let stale: Vec<String> = state_labels(issue)
        .into_iter()
        .filter(|l| Some(l.as_str()) != want)
        .collect();
    let mut steps = Vec::new();
    if !stale.is_empty() {
        steps.push(Step::RemoveLabels(stale));
    }
    if let Some(want) = want
        && !issue.labels.iter().any(|l| l == want)
    {
        steps.push(Step::AddLabel(want.to_owned()));
    }
    Ok(steps)
}

async fn run_steps(root: &Path, id: &str, steps: &[Step]) -> Result<(), GitBugError> {
    for step in steps {
        let args = step.args(id);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        cli::run(root, &args).await?;
    }
    Ok(())
}

async fn apply_steps(
    root: &Path,
    id: &str,
    plan: impl FnOnce(&Issue) -> Result<Vec<Step>, GitBugError>,
) -> Result<IssueDetail, GitBugError> {
    let current = show(root, id).await?;
    let full = current.issue.id.clone();
    run_steps(root, &full, &plan(&current.issue)?).await?;
    show(root, &full).await
}

/// Drops every `state:*` label first, so a closed issue never keeps one. No-op when already closed.
pub async fn close(root: &Path, id: &str) -> Result<IssueDetail, GitBugError> {
    apply_steps(root, id, |i| Ok(plan_close(i))).await
}

/// No-op when already open.
pub async fn reopen(root: &Path, id: &str) -> Result<IssueDetail, GitBugError> {
    apply_steps(root, id, |i| Ok(plan_reopen(i))).await
}

/// Refuses closed issues; leaves exactly one `state:*` label, or none for `Clear`.
pub async fn set_state(
    root: &Path,
    id: &str,
    target: StateTarget,
) -> Result<IssueDetail, GitBugError> {
    apply_steps(root, id, |i| plan_state(i, target)).await
}

/// The body goes through `--message=`; `-F` would strip `#` headings.
pub async fn comment(root: &Path, id: &str, body: &str) -> Result<IssueDetail, GitBugError> {
    if body.trim().is_empty() {
        return Err(GitBugError::EmptyComment);
    }
    let current = show(root, id).await?;
    let full = current.issue.id;
    let message = format!("--message={body}");
    cli::run(
        root,
        &[
            "bug",
            "comment",
            "new",
            &full,
            &message,
            "--non-interactive",
        ],
    )
    .await?;
    show(root, &full).await
}

pub async fn apply(root: &Path, id: &str, op: WriteOp) -> Result<IssueDetail, GitBugError> {
    match op {
        WriteOp::Close => close(root, id).await,
        WriteOp::Reopen => reopen(root, id).await,
        WriteOp::State(target) => set_state(root, id, target).await,
        WriteOp::Comment(body) => comment(root, id, &body).await,
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::testutil::init_repo;

    fn issue(status: Status, labels: &[&str]) -> Issue {
        Issue {
            id: "a".repeat(64),
            human_id: "aaaaaaa".into(),
            title: "t".into(),
            status,
            kind: None,
            epic: None,
            state: None,
            layer: None,
            labels: labels.iter().map(|l| (*l).to_owned()).collect(),
            author: "me".into(),
            created: String::new(),
            edited: String::new(),
            comment_count: 1,
        }
    }

    fn rm(labels: &[&str]) -> Step {
        Step::RemoveLabels(labels.iter().map(|l| (*l).to_owned()).collect())
    }

    #[test]
    fn close_drops_state_labels_only_then_closes() {
        let i = issue(
            Status::Open,
            &["kind:task", "state:blocked", "state:in-progress"],
        );
        assert_eq!(
            plan_close(&i),
            [
                rm(&["state:blocked", "state:in-progress"]),
                Step::SetStatus(Status::Closed)
            ]
        );
        assert_eq!(
            plan_close(&issue(Status::Open, &["kind:task"])),
            [Step::SetStatus(Status::Closed)]
        );
        assert!(plan_close(&issue(Status::Closed, &["state:blocked"])).is_empty());
    }

    #[test]
    fn reopen_only_when_closed() {
        assert_eq!(
            plan_reopen(&issue(Status::Closed, &[])),
            [Step::SetStatus(Status::Open)]
        );
        assert!(plan_reopen(&issue(Status::Open, &[])).is_empty());
    }

    #[test]
    fn state_sends_only_the_label_diff() {
        let target = StateTarget::InProgress;
        let none = issue(Status::Open, &["kind:task"]);
        assert_eq!(
            plan_state(&none, target).unwrap(),
            [Step::AddLabel("state:in-progress".into())]
        );

        let same = issue(Status::Open, &["state:in-progress"]);
        assert!(plan_state(&same, target).unwrap().is_empty());

        let other = issue(Status::Open, &["state:blocked"]);
        assert_eq!(
            plan_state(&other, target).unwrap(),
            [
                rm(&["state:blocked"]),
                Step::AddLabel("state:in-progress".into())
            ]
        );

        let both = issue(Status::Open, &["state:blocked", "state:in-progress"]);
        assert_eq!(plan_state(&both, target).unwrap(), [rm(&["state:blocked"])]);

        assert_eq!(
            plan_state(&other, StateTarget::Clear).unwrap(),
            [rm(&["state:blocked"])]
        );
        assert!(plan_state(&none, StateTarget::Clear).unwrap().is_empty());
    }

    #[test]
    fn state_refuses_closed_issues() {
        let closed = issue(Status::Closed, &[]);
        assert!(matches!(
            plan_state(&closed, StateTarget::Blocked),
            Err(GitBugError::Closed(_))
        ));
    }

    #[test]
    fn steps_map_to_git_bug_arguments() {
        assert_eq!(
            rm(&["state:a", "state:b"]).args("id"),
            ["bug", "label", "rm", "id", "state:a", "state:b"]
        );
        assert_eq!(
            Step::AddLabel("state:a".into()).args("id"),
            ["bug", "label", "new", "id", "state:a"]
        );
        assert_eq!(
            Step::SetStatus(Status::Closed).args("id"),
            ["bug", "status", "close", "id"]
        );
        assert_eq!(
            Step::SetStatus(Status::Open).args("id"),
            ["bug", "status", "open", "id"]
        );
    }

    #[tokio::test]
    async fn empty_comment_is_rejected_before_touching_git_bug() {
        let err = comment(Path::new("."), "--help", "  \n\t")
            .await
            .unwrap_err();
        assert!(matches!(err, GitBugError::EmptyComment));
    }

    async fn git_bug_available() -> bool {
        let found = tokio::process::Command::new("git-bug")
            .arg("version")
            .output()
            .await
            .is_ok_and(|o| o.status.success());
        if !found {
            eprintln!("skipped: git-bug is not installed");
        }
        found
    }

    async fn sandbox() -> (TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        cli::run(
            root,
            &[
                "user",
                "new",
                "--name",
                "Tester",
                "--email",
                "tester@example.com",
                "--non-interactive",
            ],
        )
        .await
        .unwrap();
        cli::run(
            root,
            &[
                "bug",
                "new",
                "--title",
                "L1 — sandbox",
                "--message",
                "body",
                "--non-interactive",
            ],
        )
        .await
        .unwrap();
        let id = super::super::list(root).await.unwrap().remove(0).id;
        cli::run(
            root,
            &["bug", "label", "new", &id, "kind:task", "state:in-progress"],
        )
        .await
        .unwrap();
        (dir, id)
    }

    fn state_count(issue: &Issue) -> usize {
        issue
            .labels
            .iter()
            .filter(|l| l.starts_with(STATE_PREFIX))
            .count()
    }

    #[tokio::test]
    async fn close_drops_state_labels_and_reopen_restores_open() {
        if !git_bug_available().await {
            return;
        }
        let (dir, id) = sandbox().await;
        let root = dir.path();

        let closed = close(root, &id).await.unwrap().issue;
        assert_eq!(closed.status, Status::Closed);
        assert_eq!(state_count(&closed), 0);
        assert!(closed.labels.contains(&"kind:task".to_owned()));

        let again = close(root, &id).await.unwrap().issue;
        assert_eq!(again.status, Status::Closed);

        assert!(matches!(
            set_state(root, &id, StateTarget::Blocked).await,
            Err(GitBugError::Closed(_))
        ));

        let reopened = reopen(root, &id).await.unwrap().issue;
        assert_eq!(reopened.status, Status::Open);
        assert_eq!(reopen(root, &id).await.unwrap().issue.status, Status::Open);
    }

    #[tokio::test]
    async fn state_switch_leaves_exactly_one_state_label() {
        if !git_bug_available().await {
            return;
        }
        let (dir, id) = sandbox().await;
        let root = dir.path();

        let blocked = set_state(root, &id, StateTarget::Blocked)
            .await
            .unwrap()
            .issue;
        assert_eq!(state_count(&blocked), 1);
        assert!(blocked.labels.contains(&"state:blocked".to_owned()));

        let same = set_state(root, &id, StateTarget::Blocked)
            .await
            .unwrap()
            .issue;
        assert_eq!(same.labels, blocked.labels);

        let cleared = set_state(root, &id, StateTarget::Clear)
            .await
            .unwrap()
            .issue;
        assert_eq!(state_count(&cleared), 0);
        assert!(cleared.labels.contains(&"kind:task".to_owned()));

        let back = set_state(root, &id, StateTarget::InProgress)
            .await
            .unwrap()
            .issue;
        assert_eq!(state_count(&back), 1);
    }

    #[tokio::test]
    async fn comment_keeps_markdown_headings() {
        if !git_bug_available().await {
            return;
        }
        let (dir, id) = sandbox().await;
        let body = "## Heading\n\n- item\n\n# Top";
        let detail = comment(dir.path(), &id, body).await.unwrap();
        assert_eq!(detail.comments.len(), 1);
        assert_eq!(detail.comments[0].message, body);
        assert_eq!(detail.issue.comment_count, 2);
    }
}
