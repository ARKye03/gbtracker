use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::gitbug::Issue;
use crate::workspace::{GitError, RepoInfo, atomic_write, git, project_dirs};

const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedIssues {
    pub fingerprint: String,
    pub issues: Vec<Issue>,
}

/// Everything cached on disk; issues are keyed by repo root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub repos: Vec<RepoInfo>,
    pub issues: HashMap<PathBuf, CachedIssues>,
}

#[derive(Serialize, Deserialize)]
struct ReposFile {
    version: u32,
    repos: Vec<RepoInfo>,
}

#[derive(Serialize, Deserialize)]
struct IssuesFile {
    version: u32,
    fingerprint: String,
    issues: Vec<Issue>,
}

/// FNV-1a: stable across runs and Rust versions, unlike `DefaultHasher`.
fn hash_hex(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}

/// Changes on any bug write; costs one git call and takes no git-bug lock.
pub async fn fingerprint(root: &Path) -> Result<String, GitError> {
    let refs = git(
        root,
        &[
            "for-each-ref",
            "refs/bugs",
            "--format=%(refname) %(objectname)",
        ],
    )
    .await?;
    Ok(hash_hex(refs.as_bytes()))
}

/// Unreadable repos count as stale.
pub async fn is_stale(root: &Path, cached_fp: &str) -> bool {
    fingerprint(root).await.map_or(true, |fp| fp != cached_fp)
}

pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Cache { dir: dir.into() }
    }

    pub fn default_location() -> Option<Self> {
        project_dirs().map(|d| Cache::new(d.cache_dir()))
    }

    fn repos_path(&self) -> PathBuf {
        self.dir.join("repos.json")
    }

    fn issues_path(&self, repo: &RepoInfo) -> PathBuf {
        let key = hash_hex(repo.common_dir.to_string_lossy().as_bytes());
        self.dir.join("issues").join(format!("{key}.json"))
    }

    /// Corrupt or version-mismatched files are skipped, never fatal.
    pub fn load_snapshot(&self) -> Snapshot {
        let repos = read_json::<ReposFile>(&self.repos_path())
            .filter(|f| f.version == VERSION)
            .map(|f| f.repos)
            .unwrap_or_default();
        let issues = repos
            .iter()
            .filter_map(|r| Some((r.root.clone(), self.load_issues(r)?)))
            .collect();
        Snapshot { repos, issues }
    }

    pub fn load_issues(&self, repo: &RepoInfo) -> Option<CachedIssues> {
        read_json::<IssuesFile>(&self.issues_path(repo))
            .filter(|f| f.version == VERSION)
            .map(|f| CachedIssues {
                fingerprint: f.fingerprint,
                issues: f.issues,
            })
    }

    pub fn save_repos(&self, repos: &[RepoInfo]) -> io::Result<()> {
        let file = ReposFile {
            version: VERSION,
            repos: repos.to_vec(),
        };
        atomic_write(&self.repos_path(), &serde_json::to_vec(&file)?)
    }

    pub fn save_issues(
        &self,
        repo: &RepoInfo,
        fingerprint: &str,
        issues: &[Issue],
    ) -> io::Result<()> {
        let file = IssuesFile {
            version: VERSION,
            fingerprint: fingerprint.to_owned(),
            issues: issues.to_vec(),
        };
        atomic_write(&self.issues_path(repo), &serde_json::to_vec(&file)?)
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Snapshot from the default cache dir; empty when there is no home directory.
pub fn load_snapshot() -> Snapshot {
    Cache::default_location()
        .map(|c| c.load_snapshot())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::gitbug::{Kind, Status};
    use crate::testutil::{git, init_repo};

    fn repo(root: &str) -> RepoInfo {
        RepoInfo {
            root: root.into(),
            name: "r".into(),
            has_git_bug: true,
            common_dir: format!("{root}/.git").into(),
        }
    }

    fn issue() -> Issue {
        Issue {
            id: "a".repeat(64),
            human_id: "aaaaaaa".into(),
            title: "L1 — x".into(),
            status: Status::Open,
            kind: Some(Kind::Task),
            epic: None,
            state: None,
            layer: Some(1),
            labels: vec!["kind:task".into()],
            author: "me".into(),
            created: "2026-11-08T18:19:24-03:00".into(),
            edited: "2026-11-08T18:19:24-03:00".into(),
            comment_count: 1,
        }
    }

    #[tokio::test]
    async fn fingerprint_changes_only_when_bug_refs_change() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let empty = fingerprint(tmp.path()).await.unwrap();
        git(tmp.path(), &["update-ref", "refs/bugs/x", "HEAD"]);
        let one = fingerprint(tmp.path()).await.unwrap();
        assert_eq!(one, fingerprint(tmp.path()).await.unwrap());
        git(tmp.path(), &["update-ref", "refs/bugs/y", "HEAD"]);
        let two = fingerprint(tmp.path()).await.unwrap();
        assert_ne!(one, two);
        assert_ne!(empty, one);
        assert!(!is_stale(tmp.path(), &two).await);
        assert!(is_stale(tmp.path(), &one).await);
    }

    #[tokio::test]
    async fn fingerprint_outside_a_repo_errors_and_is_stale() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(fingerprint(tmp.path()).await.is_err());
        assert!(is_stale(tmp.path(), "x").await);
    }

    #[test]
    fn snapshot_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache::new(tmp.path().join("cache"));
        let (a, b) = (repo("/a"), repo("/b"));
        cache.save_repos(&[a.clone(), b.clone()]).unwrap();
        cache.save_issues(&a, "fp1", &[issue()]).unwrap();

        let snap = cache.load_snapshot();
        assert_eq!(snap.repos, [a.clone(), b]);
        assert_eq!(snap.issues.len(), 1);
        let cached = &snap.issues[&a.root];
        assert_eq!(cached.fingerprint, "fp1");
        assert_eq!(cached.issues, [issue()]);
    }

    #[test]
    fn empty_cache_dir_loads_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let snap = Cache::new(tmp.path().join("none")).load_snapshot();
        assert_eq!(snap, Snapshot::default());
    }

    #[test]
    fn corrupt_files_load_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache::new(tmp.path());
        let a = repo("/a");
        cache.save_repos(std::slice::from_ref(&a)).unwrap();
        cache.save_issues(&a, "fp", &[issue()]).unwrap();

        fs::write(cache.issues_path(&a), "{garbage").unwrap();
        let snap = cache.load_snapshot();
        assert_eq!(snap.repos, [a]);
        assert!(snap.issues.is_empty());

        fs::write(cache.repos_path(), "\u{0}\u{0}").unwrap();
        assert_eq!(cache.load_snapshot(), Snapshot::default());
    }

    #[test]
    fn version_mismatch_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache::new(tmp.path());
        let a = repo("/a");
        fs::write(
            cache.repos_path(),
            serde_json::to_vec(&ReposFile {
                version: VERSION + 1,
                repos: vec![a.clone()],
            })
            .unwrap(),
        )
        .unwrap();
        assert!(cache.load_snapshot().repos.is_empty());

        cache.save_repos(std::slice::from_ref(&a)).unwrap();
        fs::create_dir_all(tmp.path().join("issues")).unwrap();
        fs::write(
            cache.issues_path(&a),
            serde_json::to_vec(&IssuesFile {
                version: VERSION + 1,
                fingerprint: "fp".into(),
                issues: vec![issue()],
            })
            .unwrap(),
        )
        .unwrap();
        assert!(cache.load_snapshot().issues.is_empty());
    }

    #[test]
    fn issues_are_keyed_by_common_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache::new(tmp.path());
        let (a, mut wt) = (repo("/a"), repo("/a-wt"));
        wt.common_dir = a.common_dir.clone();
        assert_eq!(cache.issues_path(&a), cache.issues_path(&wt));
        assert_ne!(cache.issues_path(&a), cache.issues_path(&repo("/b")));
    }
}
