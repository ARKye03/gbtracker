//! Check Done: finds open issues whose `Refs:` trailer is already on the base branch. Nothing is fetched, so a
//! stale `origin/<base>` only hides work, never invents it. Same rules as the roadmap plugin's `gb merged`.

use std::collections::HashSet;
use std::path::Path;

use crate::epics;
use crate::gitbug::{Issue, Kind, Status};
use crate::workspace::{GitError, git};

const TRUNKS: [&str; 4] = ["main", "trunk", "master", "develop"];
const MIN_PREFIX: usize = 7;

#[derive(Debug, thiserror::Error)]
pub enum DoneError {
    #[error(
        "no base branch: set baseBranch in .claude/roadmap.json or create main, trunk, master or develop"
    )]
    NoBase,
    #[error("invalid base branch {0:?}")]
    InvalidBase(String),
    #[error("neither {0} nor origin/{0} exists")]
    MissingBase(String),
    #[error(transparent)]
    Git(#[from] GitError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub sha: String,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub human_id: String,
    pub title: String,
    pub kind: Option<Kind>,
    /// Newest commit first.
    pub evidence: Vec<Evidence>,
}

/// An open epic whose children are all closed or listed as candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicCandidate {
    pub id: String,
    pub human_id: String,
    pub title: String,
    pub children: usize,
    /// Open children; the epic closes only when every one of them does.
    pub needs: Vec<NeededChild>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeededChild {
    pub id: String,
    pub human_id: String,
}

impl EpicCandidate {
    pub fn is_allowed(&self, picked: &HashSet<String>) -> bool {
        self.needs.iter().all(|c| picked.contains(&c.id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub base: String,
    /// Refs the log was read from: `<base>` and/or `origin/<base>`.
    pub refs: Vec<String>,
    pub open_count: usize,
    pub bugs: Vec<Candidate>,
    pub epics: Vec<EpicCandidate>,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.bugs.is_empty() && self.epics.is_empty()
    }

    /// Every row ticked: the starting selection.
    pub fn all_ids(&self) -> HashSet<String> {
        self.bugs
            .iter()
            .map(|b| &b.id)
            .chain(self.epics.iter().map(|e| &e.id))
            .cloned()
            .collect()
    }

    /// Ticked bugs, and ticked epics whose children are all ticked too, in that order.
    pub fn chosen<'a>(
        &'a self,
        picked: &HashSet<String>,
    ) -> (Vec<&'a Candidate>, Vec<&'a EpicCandidate>) {
        let bugs = self
            .bugs
            .iter()
            .filter(|b| picked.contains(&b.id))
            .collect();
        let epics = self
            .epics
            .iter()
            .filter(|e| picked.contains(&e.id) && e.is_allowed(picked))
            .collect();
        (bugs, epics)
    }
}

fn valid_branch(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
}

async fn ref_exists(root: &Path, name: &str) -> bool {
    git(root, &["show-ref", "--verify", "--quiet", name])
        .await
        .is_ok()
}

fn configured_base(root: &Path) -> Option<String> {
    let bytes = std::fs::read(root.join(".claude/roadmap.json")).ok()?;
    let config: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let name = config.get("baseBranch")?.as_str()?;
    (!name.is_empty()).then(|| name.to_owned())
}

/// `baseBranch` from `.claude/roadmap.json`, then `origin/HEAD`, then the first of the usual trunk names that
/// exists locally or on origin. `explicit` wins over all of them.
pub async fn resolve_base(root: &Path, explicit: Option<&str>) -> Option<String> {
    if let Some(name) = explicit.filter(|n| !n.is_empty()) {
        return Some(name.to_owned());
    }
    if let Some(name) = configured_base(root) {
        return Some(name);
    }
    if let Ok(head) = git(
        root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .await
    {
        let head = head.trim();
        let name = head.strip_prefix("origin/").unwrap_or(head);
        if !name.is_empty() {
            return Some(name.to_owned());
        }
    }
    for name in TRUNKS {
        if ref_exists(root, &format!("refs/heads/{name}")).await
            || ref_exists(root, &format!("refs/remotes/origin/{name}")).await
        {
            return Some(name.to_owned());
        }
    }
    None
}

/// One commit on base that carries `Refs:` trailers: its evidence and the lowercased ids it names.
#[derive(Debug, PartialEq, Eq)]
struct RefsCommit {
    evidence: Evidence,
    ids: Vec<String>,
}

fn parse_log(log: &str) -> Vec<RefsCommit> {
    log.lines()
        .filter_map(|line| {
            let mut parts = line.split('\u{1f}');
            let (sha, subject, trailers) = (parts.next()?, parts.next()?, parts.next()?);
            let ids: Vec<String> = trailers
                .split_whitespace()
                .filter(|id| id.len() >= MIN_PREFIX)
                .map(str::to_ascii_lowercase)
                .collect();
            (!sha.is_empty() && !ids.is_empty()).then(|| RefsCommit {
                evidence: Evidence {
                    sha: sha.to_owned(),
                    subject: subject.to_owned(),
                },
                ids,
            })
        })
        .collect()
}

fn candidates(issues: &[Issue], commits: &[RefsCommit]) -> Vec<Candidate> {
    issues
        .iter()
        .filter(|i| i.status == Status::Open && i.kind != Some(Kind::Epic))
        .filter_map(|issue| {
            let evidence: Vec<Evidence> = commits
                .iter()
                .filter(|c| c.ids.iter().any(|p| issue.id.starts_with(p.as_str())))
                .map(|c| c.evidence.clone())
                .collect();
            (!evidence.is_empty()).then(|| Candidate {
                id: issue.id.clone(),
                human_id: issue.human_id.clone(),
                title: issue.title.clone(),
                kind: issue.kind.clone(),
                evidence,
            })
        })
        .collect()
}

/// Positions of open epics with at least one child, every child closed or in `closing`. Commit evidence alone
/// never closes an epic.
pub fn ready_epics(issues: &[Issue], closing: &HashSet<&str>) -> Vec<usize> {
    issues
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == Some(Kind::Epic) && e.status == Status::Open)
        .filter(|(_, e)| {
            let kids = epics::children_of(issues, e);
            !kids.is_empty()
                && kids.iter().all(|&c| {
                    issues[c].status == Status::Closed || closing.contains(issues[c].id.as_str())
                })
        })
        .map(|(pos, _)| pos)
        .collect()
}

fn epic_candidates(issues: &[Issue], bugs: &[Candidate]) -> Vec<EpicCandidate> {
    let closing: HashSet<&str> = bugs.iter().map(|b| b.id.as_str()).collect();
    ready_epics(issues, &closing)
        .into_iter()
        .map(|pos| {
            let epic = &issues[pos];
            let kids = epics::children_of(issues, epic);
            EpicCandidate {
                id: epic.id.clone(),
                human_id: epic.human_id.clone(),
                title: epic.title.clone(),
                children: kids.len(),
                needs: kids
                    .into_iter()
                    .map(|c| &issues[c])
                    .filter(|c| c.status == Status::Open)
                    .map(|c| NeededChild {
                        id: c.id.clone(),
                        human_id: c.human_id.clone(),
                    })
                    .collect(),
            }
        })
        .collect()
}

/// `issues` are the already-loaded ones; no git-bug call is made.
pub async fn find(
    root: &Path,
    issues: &[Issue],
    explicit_base: Option<&str>,
) -> Result<Report, DoneError> {
    let base = resolve_base(root, explicit_base)
        .await
        .ok_or(DoneError::NoBase)?;
    if !valid_branch(&base) {
        return Err(DoneError::InvalidBase(base));
    }
    let mut refs = Vec::new();
    if ref_exists(root, &format!("refs/heads/{base}")).await {
        refs.push(base.clone());
    }
    let remote = format!("origin/{base}");
    if ref_exists(root, &format!("refs/remotes/{remote}")).await {
        refs.push(remote);
    }
    if refs.is_empty() {
        return Err(DoneError::MissingBase(base));
    }

    let mut args = vec!["log"];
    args.extend(refs.iter().map(String::as_str));
    args.extend([
        "--grep=^Refs: ",
        "--format=%h%x1f%s%x1f%(trailers:key=Refs,valueonly,separator=%x20)",
        "--",
    ]);
    let log = git(root, &args).await?;
    let bugs = candidates(issues, &parse_log(&log));
    let epics = epic_candidates(issues, &bugs);
    Ok(Report {
        base,
        refs,
        open_count: issues.iter().filter(|i| i.status == Status::Open).count(),
        bugs,
        epics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{git as git_sync, init_repo};

    fn issue(id: &str, kind: Kind, status: Status, epic: Option<&str>) -> Issue {
        Issue {
            id: id.to_owned(),
            human_id: id[..7].to_owned(),
            title: format!("title {}", &id[..7]),
            status,
            kind: Some(kind),
            epic: epic.map(str::to_owned),
            state: None,
            layer: None,
            labels: Vec::new(),
            author: "a".into(),
            created: String::new(),
            edited: String::new(),
            comment_count: 1,
        }
    }

    fn id(prefix: &str) -> String {
        format!("{prefix}{}", "0".repeat(64 - prefix.len()))
    }

    fn open_task(prefix: &str) -> Issue {
        issue(&id(prefix), Kind::Task, Status::Open, None)
    }

    fn repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        git_sync(tmp.path(), &["branch", "-M", "main"]);
        tmp
    }

    fn commit(dir: &Path, subject: &str, trailer: Option<&str>) {
        let mut args = vec!["commit", "-q", "--allow-empty", "-m", subject];
        let body;
        if let Some(t) = trailer {
            body = format!("Refs: {t}");
            args.extend(["-m", &body]);
        }
        git_sync(dir, &args);
    }

    fn set_set<'a>(ids: &[&'a str]) -> HashSet<&'a str> {
        ids.iter().copied().collect()
    }

    #[tokio::test]
    async fn trailer_prefix_makes_an_open_bug_a_candidate() {
        let tmp = repo();
        commit(tmp.path(), "feat: one", Some(&id("abcdef1")));
        commit(tmp.path(), "feat: two", Some("2222222"));
        commit(tmp.path(), "feat: untracked", None);
        let issues = [
            open_task("abcdef1"),
            open_task("bbbbbbb"),
            open_task("2222222"),
        ];
        let report = find(tmp.path(), &issues, None).await.unwrap();
        let hit: Vec<&str> = report.bugs.iter().map(|b| b.human_id.as_str()).collect();
        assert_eq!(hit, ["abcdef1", "2222222"]);
        assert_eq!(report.bugs[0].evidence.len(), 1);
        assert_eq!(report.bugs[0].evidence[0].subject, "feat: one");
        assert_eq!(report.open_count, 3);
        assert_eq!(report.refs, ["main"]);
    }

    #[tokio::test]
    async fn commits_without_the_trailer_never_match() {
        let tmp = repo();
        commit(tmp.path(), "feat: mentions abcdef1 in the subject", None);
        let issues = [open_task("abcdef1")];
        let report = find(tmp.path(), &issues, None).await.unwrap();
        assert!(report.is_empty());
    }

    #[tokio::test]
    async fn short_trailer_ids_uppercase_and_multiple_trailers_are_handled() {
        let tmp = repo();
        commit(tmp.path(), "feat: short", Some("abcdef"));
        commit(tmp.path(), "feat: upper", Some("ABCDEF1"));
        git_sync(
            tmp.path(),
            &[
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "feat: two",
                "-m",
                "Refs: 3333333\nRefs: 4444444",
            ],
        );
        let issues = [
            open_task("abcdef1"),
            open_task("3333333"),
            open_task("4444444"),
        ];
        let report = find(tmp.path(), &issues, None).await.unwrap();
        let hit: Vec<&str> = report.bugs.iter().map(|b| b.human_id.as_str()).collect();
        assert_eq!(hit, ["abcdef1", "3333333", "4444444"]);
        assert_eq!(
            report.bugs[0].evidence.len(),
            1,
            "the 6-char trailer is ignored"
        );
    }

    #[tokio::test]
    async fn closed_issues_and_epics_are_never_bug_candidates() {
        let tmp = repo();
        commit(tmp.path(), "feat", Some("abcdef1 eeeeeee ccccccc"));
        let issues = [
            issue(&id("abcdef1"), Kind::Task, Status::Closed, None),
            issue(&id("eeeeeee"), Kind::Epic, Status::Open, None),
            open_task("ccccccc"),
        ];
        let report = find(tmp.path(), &issues, None).await.unwrap();
        let hit: Vec<&str> = report.bugs.iter().map(|b| b.human_id.as_str()).collect();
        assert_eq!(hit, ["ccccccc"]);
    }

    #[tokio::test]
    async fn evidence_comes_from_the_base_not_other_branches() {
        let tmp = repo();
        git_sync(tmp.path(), &["checkout", "-q", "-b", "feature"]);
        commit(tmp.path(), "feat: on feature", Some(&id("abcdef1")));
        let issues = [open_task("abcdef1")];
        assert!(find(tmp.path(), &issues, None).await.unwrap().is_empty());
        git_sync(tmp.path(), &["checkout", "-q", "main"]);
        git_sync(tmp.path(), &["merge", "-q", "--ff-only", "feature"]);
        assert_eq!(find(tmp.path(), &issues, None).await.unwrap().bugs.len(), 1);
    }

    #[tokio::test]
    async fn origin_base_is_read_too() {
        let tmp = repo();
        git_sync(tmp.path(), &["checkout", "-q", "-b", "feature"]);
        commit(tmp.path(), "feat: remote only", Some(&id("abcdef1")));
        git_sync(
            tmp.path(),
            &["update-ref", "refs/remotes/origin/main", "feature"],
        );
        git_sync(tmp.path(), &["checkout", "-q", "main"]);
        let report = find(tmp.path(), &[open_task("abcdef1")], None)
            .await
            .unwrap();
        assert_eq!(report.refs, ["main", "origin/main"]);
        assert_eq!(report.bugs.len(), 1);
    }

    #[tokio::test]
    async fn base_branch_config_wins_over_trunk_names() {
        let tmp = repo();
        git_sync(tmp.path(), &["branch", "release"]);
        assert_eq!(
            resolve_base(tmp.path(), None).await.as_deref(),
            Some("main")
        );
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        std::fs::write(
            tmp.path().join(".claude/roadmap.json"),
            r#"{"baseBranch": "release"}"#,
        )
        .unwrap();
        assert_eq!(
            resolve_base(tmp.path(), None).await.as_deref(),
            Some("release")
        );
        assert_eq!(
            resolve_base(tmp.path(), Some("develop")).await.as_deref(),
            Some("develop")
        );
        let report = find(tmp.path(), &[], None).await.unwrap();
        assert_eq!(report.base, "release");
    }

    #[tokio::test]
    async fn empty_or_broken_config_falls_through() {
        let tmp = repo();
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        let config = tmp.path().join(".claude/roadmap.json");
        std::fs::write(&config, r#"{"baseBranch": ""}"#).unwrap();
        assert_eq!(
            resolve_base(tmp.path(), None).await.as_deref(),
            Some("main")
        );
        std::fs::write(&config, "{nope").unwrap();
        assert_eq!(
            resolve_base(tmp.path(), None).await.as_deref(),
            Some("main")
        );
    }

    #[tokio::test]
    async fn origin_head_wins_over_trunk_names() {
        let tmp = repo();
        git_sync(tmp.path(), &["branch", "trunk"]);
        git_sync(
            tmp.path(),
            &["update-ref", "refs/remotes/origin/trunk", "HEAD"],
        );
        git_sync(
            tmp.path(),
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/trunk",
            ],
        );
        assert_eq!(
            resolve_base(tmp.path(), None).await.as_deref(),
            Some("trunk")
        );
    }

    #[tokio::test]
    async fn missing_base_is_reported() {
        let tmp = repo();
        git_sync(tmp.path(), &["branch", "-m", "work"]);
        assert!(matches!(
            find(tmp.path(), &[], None).await,
            Err(DoneError::NoBase)
        ));
        assert!(matches!(
            find(tmp.path(), &[], Some("nope")).await,
            Err(DoneError::MissingBase(_))
        ));
        assert!(matches!(
            find(tmp.path(), &[], Some("--all")).await,
            Err(DoneError::InvalidBase(_))
        ));
    }

    #[test]
    fn epic_qualifies_only_when_every_child_is_closed_or_selected() {
        let epic = id("e000000");
        let child = |p: &str, status| issue(&id(p), Kind::Task, status, Some(&epic));
        let issues = vec![
            issue(&epic, Kind::Epic, Status::Open, None),
            child("c111111", Status::Closed),
            child("c222222", Status::Open),
            child("c333333", Status::Open),
        ];
        let c2 = id("c222222");
        let c3 = id("c333333");
        assert!(ready_epics(&issues, &set_set(&[])).is_empty());
        assert!(ready_epics(&issues, &set_set(&[&c2])).is_empty());
        assert_eq!(ready_epics(&issues, &set_set(&[&c2, &c3])), [0]);
    }

    #[test]
    fn epic_with_all_children_closed_qualifies_and_without_children_does_not() {
        let epic = id("e000000");
        let lone = id("f000000");
        let issues = vec![
            issue(&epic, Kind::Epic, Status::Open, None),
            issue(&id("c111111"), Kind::Task, Status::Closed, Some(&epic)),
            issue(&lone, Kind::Epic, Status::Open, None),
            issue(&id("e999999"), Kind::Epic, Status::Closed, None),
        ];
        assert_eq!(ready_epics(&issues, &set_set(&[])), [0]);
    }

    #[tokio::test]
    async fn report_offers_epics_and_follows_the_selection() {
        let tmp = repo();
        commit(tmp.path(), "feat", Some(&id("c222222")));
        let epic = id("e000000");
        let issues = vec![
            issue(&epic, Kind::Epic, Status::Open, None),
            issue(&id("c111111"), Kind::Task, Status::Closed, Some(&epic)),
            issue(&id("c222222"), Kind::Task, Status::Open, Some(&epic)),
        ];
        let report = find(tmp.path(), &issues, None).await.unwrap();
        assert_eq!(report.bugs.len(), 1);
        assert_eq!(report.epics.len(), 1);
        assert_eq!(report.epics[0].children, 2);
        assert_eq!(report.epics[0].needs.len(), 1);

        let mut picked = report.all_ids();
        let (bugs, epics) = report.chosen(&picked);
        assert_eq!((bugs.len(), epics.len()), (1, 1));

        picked.remove(&id("c222222"));
        let (bugs, epics) = report.chosen(&picked);
        assert_eq!((bugs.len(), epics.len()), (0, 0), "epic waits on its child");
    }

    #[test]
    fn log_lines_without_trailers_or_with_blank_lines_are_skipped() {
        let log = "abc1234\u{1f}feat: x\u{1f}aaaaaaa bbbbbbbb\n\nfff0000\u{1f}chore\u{1f}\n";
        let parsed = parse_log(log);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].ids, ["aaaaaaa", "bbbbbbbb"]);
    }
}
