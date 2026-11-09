use std::collections::{BTreeSet, HashMap};
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

const DEFAULT_SCAN_DEPTH: u8 = 4;
const SKIP_DIRS: [&str; 2] = ["node_modules", "target"];

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("workspace config i/o failed: {0}")]
    Io(#[from] io::Error),
    #[error("invalid workspace config: {0}")]
    Parse(#[from] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("failed to run git: {0}")]
    Spawn(#[source] io::Error),
    #[error("git {args} failed: {stderr}")]
    Failed { args: String, stderr: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RootKind {
    /// The directory itself is one project.
    Project,
    /// The directory is a parent searched for repos.
    Scan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Root {
    pub path: PathBuf,
    pub kind: RootKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePref {
    pub fn next(self) -> Self {
        match self {
            ThemePref::System => ThemePref::Light,
            ThemePref::Light => ThemePref::Dark,
            ThemePref::Dark => ThemePref::System,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "WorkspaceFile")]
pub struct Workspace {
    pub roots: Vec<Root>,
    pub scan_depth: u8,
    pub theme: ThemePref,
    pub sidebar_collapsed: bool,
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace {
            roots: Vec::new(),
            scan_depth: DEFAULT_SCAN_DEPTH,
            theme: ThemePref::default(),
            sidebar_collapsed: false,
        }
    }
}

/// On-disk shape; carries the pre-`theme` `dark` flag so old configs keep their look.
#[derive(Deserialize)]
#[serde(default)]
struct WorkspaceFile {
    roots: Vec<Root>,
    scan_depth: u8,
    theme: Option<ThemePref>,
    dark: Option<bool>,
    sidebar_collapsed: bool,
}

impl Default for WorkspaceFile {
    fn default() -> Self {
        let ws = Workspace::default();
        WorkspaceFile {
            roots: ws.roots,
            scan_depth: ws.scan_depth,
            theme: None,
            dark: None,
            sidebar_collapsed: ws.sidebar_collapsed,
        }
    }
}

impl From<WorkspaceFile> for Workspace {
    fn from(f: WorkspaceFile) -> Self {
        let theme = match (f.theme, f.dark) {
            (Some(theme), _) => theme,
            (None, Some(true)) => ThemePref::Dark,
            (None, _) => ThemePref::System,
        };
        Workspace {
            roots: f.roots,
            scan_depth: f.scan_depth,
            theme,
            sidebar_collapsed: f.sidebar_collapsed,
        }
    }
}

pub(crate) fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("dev", "arkye03", "gbtracker")
}

impl Workspace {
    pub fn config_path() -> Option<PathBuf> {
        project_dirs().map(|d| d.config_dir().join("workspace.json"))
    }

    /// Missing config yields the default workspace; a corrupt one is an error so it is never overwritten silently.
    pub fn load() -> Result<Self, ConfigError> {
        match Self::config_path() {
            Some(path) => Self::load_from(&path),
            None => Ok(Self::default()),
        }
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        let path = Self::config_path()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home directory"))?;
        self.save_to(&path)
    }

    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        atomic_write(path, &serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}

/// Writes through a sibling temp file and renames it into place.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let tmp = PathBuf::from(tmp);
    let result = (|| -> io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoInfo {
    pub root: PathBuf,
    pub name: String,
    pub has_git_bug: bool,
    /// git-bug state lives here, shared by every linked worktree.
    pub common_dir: PathBuf,
}

pub(crate) async fn git(dir: &Path, args: &[&str]) -> Result<String, GitError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .kill_on_drop(true)
        .output()
        .await
        .map_err(GitError::Spawn)?;
    if !out.status.success() {
        return Err(GitError::Failed {
            args: args.join(" "),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn is_skipped(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref())
}

/// Directories containing `.git` (dir or file) up to `max_depth` below `root`; repos are not descended into.
fn scan(root: &Path, max_depth: u8) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0u8)];
    while let Some((dir, depth)) = stack.pop() {
        if dir.join(".git").symlink_metadata().is_ok() {
            found.push(dir);
            continue;
        }
        if depth >= max_depth {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            // `file_type` does not follow symlinks, so linked dirs report false here.
            if entry.file_type().is_ok_and(|t| t.is_dir()) && !is_skipped(&entry.file_name()) {
                stack.push((entry.path(), depth + 1));
            }
        }
    }
    found
}

async fn probe(root: PathBuf) -> Option<RepoInfo> {
    let common = git(
        &root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .await
    .ok()?;
    let refs = git(
        &root,
        &["for-each-ref", "--count=1", "refs/bugs", "refs/identities"],
    )
    .await
    .ok()?;
    Some(RepoInfo {
        name: root.file_name().map_or_else(
            || root.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        ),
        has_git_bug: !refs.trim().is_empty(),
        common_dir: PathBuf::from(common.trim_end_matches(['\n', '\r'])),
        root,
    })
}

/// The main worktree (`.git` is a directory) wins; ties break on path so results are deterministic.
fn preferred(a: &RepoInfo, b: &RepoInfo) -> bool {
    let rank = |r: &RepoInfo| (!r.root.join(".git").is_dir(), r.root.clone());
    rank(a) < rank(b)
}

pub async fn discover(workspace: &Workspace) -> Vec<RepoInfo> {
    let depth = workspace.scan_depth;
    let mut walks = JoinSet::new();
    for root in &workspace.roots {
        let (path, kind) = (root.path.clone(), root.kind);
        walks.spawn_blocking(move || match kind {
            RootKind::Project => vec![path],
            RootKind::Scan => scan(&path, depth),
        });
    }
    let mut candidates = BTreeSet::new();
    while let Some(done) = walks.join_next().await {
        if let Ok(paths) = done {
            candidates.extend(paths);
        }
    }

    let limit = std::thread::available_parallelism().map_or(4, usize::from);
    let permits = Arc::new(Semaphore::new(limit));
    let mut probes = JoinSet::new();
    for path in candidates {
        let permits = Arc::clone(&permits);
        probes.spawn(async move {
            let _permit = permits.acquire_owned().await.ok()?;
            probe(path).await
        });
    }

    let mut by_common: HashMap<PathBuf, RepoInfo> = HashMap::new();
    while let Some(done) = probes.join_next().await {
        let Ok(Some(info)) = done else { continue };
        match by_common.get(&info.common_dir) {
            Some(existing) if !preferred(&info, existing) => {}
            _ => {
                by_common.insert(info.common_dir.clone(), info);
            }
        }
    }
    let mut repos: Vec<RepoInfo> = by_common.into_values().collect();
    repos.sort_by(|a, b| (&a.name, &a.root).cmp(&(&b.name, &b.root)));
    repos
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::testutil::{git, init_repo};

    fn scan_ws(path: &Path, depth: u8) -> Workspace {
        Workspace {
            roots: vec![Root {
                path: path.to_path_buf(),
                kind: RootKind::Scan,
            }],
            scan_depth: depth,
            ..Workspace::default()
        }
    }

    fn names(repos: &[RepoInfo]) -> Vec<&str> {
        repos.iter().map(|r| r.name.as_str()).collect()
    }

    #[tokio::test]
    async fn scan_respects_depth_and_skips() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        init_repo(&r.join("a"));
        init_repo(&r.join("x/y/z/deep4"));
        init_repo(&r.join("x/y/z/w/deep5"));
        init_repo(&r.join("node_modules/pkg"));
        init_repo(&r.join("target/pkg"));
        init_repo(&r.join(".hidden/pkg"));

        let found = discover(&scan_ws(r, 4)).await;
        assert_eq!(names(&found), ["a", "deep4"]);
        let shallow = discover(&scan_ws(r, 1)).await;
        assert_eq!(names(&shallow), ["a"]);
    }

    #[tokio::test]
    async fn scan_stops_at_repo_boundaries() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(&tmp.path().join("outer"));
        init_repo(&tmp.path().join("outer/vendor/inner"));
        let found = discover(&scan_ws(tmp.path(), 4)).await;
        assert_eq!(names(&found), ["outer"]);
    }

    #[tokio::test]
    async fn scan_root_that_is_a_repo_is_found() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        init_repo(&tmp.path().join("child"));
        let found = discover(&scan_ws(tmp.path(), 4)).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].root, tmp.path());
    }

    #[tokio::test]
    async fn linked_worktree_is_deduped_to_main() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        init_repo(&main);
        git(
            &main,
            &["worktree", "add", "-q", "../a-wt", "-b", "feature"],
        );
        let found = discover(&scan_ws(tmp.path(), 2)).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].root, main);
    }

    #[tokio::test]
    async fn linked_worktree_alone_still_discovered() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        init_repo(&main);
        git(&main, &["worktree", "add", "-q", "../wt", "-b", "feature"]);
        let ws = Workspace {
            roots: vec![Root {
                path: tmp.path().join("wt"),
                kind: RootKind::Project,
            }],
            ..Workspace::default()
        };
        let found = discover(&ws).await;
        assert_eq!(names(&found), ["wt"]);
    }

    #[tokio::test]
    async fn git_bug_presence_follows_refs() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let ws = scan_ws(tmp.path(), 2);
        assert!(!discover(&ws).await[0].has_git_bug);
        git(&repo, &["update-ref", "refs/bugs/x", "HEAD"]);
        assert!(discover(&ws).await[0].has_git_bug);
    }

    #[tokio::test]
    async fn identities_ref_also_counts() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        git(tmp.path(), &["update-ref", "refs/identities/x", "HEAD"]);
        assert!(discover(&scan_ws(tmp.path(), 1)).await[0].has_git_bug);
    }

    #[tokio::test]
    async fn project_root_non_repo_is_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = Workspace {
            roots: vec![Root {
                path: tmp.path().to_path_buf(),
                kind: RootKind::Project,
            }],
            ..Workspace::default()
        };
        assert!(discover(&ws).await.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_are_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        init_repo(&outside.path().join("real"));
        fs::create_dir_all(tmp.path().join("scan")).unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("scan/link")).unwrap();
        let found = discover(&scan_ws(&tmp.path().join("scan"), 4)).await;
        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn multiple_roots_merge_and_dedupe() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(&tmp.path().join("p1"));
        init_repo(&tmp.path().join("p2"));
        let ws = Workspace {
            roots: vec![
                Root {
                    path: tmp.path().join("p1"),
                    kind: RootKind::Project,
                },
                Root {
                    path: tmp.path().to_path_buf(),
                    kind: RootKind::Scan,
                },
            ],
            ..Workspace::default()
        };
        assert_eq!(names(&discover(&ws).await), ["p1", "p2"]);
    }

    #[test]
    fn missing_config_is_default() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = Workspace::load_from(&tmp.path().join("nope.json")).unwrap();
        assert_eq!(ws, Workspace::default());
        assert_eq!(ws.scan_depth, 4);
    }

    #[test]
    fn config_roundtrips_and_leaves_no_temp_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/workspace.json");
        let ws = Workspace {
            roots: vec![
                Root {
                    path: "/a".into(),
                    kind: RootKind::Project,
                },
                Root {
                    path: "/b".into(),
                    kind: RootKind::Scan,
                },
            ],
            scan_depth: 2,
            theme: ThemePref::Dark,
            sidebar_collapsed: true,
        };
        ws.save_to(&path).unwrap();
        assert_eq!(Workspace::load_from(&path).unwrap(), ws);
        let files: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn partial_config_fills_defaults_and_corrupt_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("workspace.json");
        fs::write(&path, r#"{"dark": true}"#).unwrap();
        let ws = Workspace::load_from(&path).unwrap();
        assert_eq!(ws.theme, ThemePref::Dark);
        assert_eq!(ws.scan_depth, 4);
        fs::write(&path, "{nope").unwrap();
        assert!(matches!(
            Workspace::load_from(&path),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn theme_defaults_to_system_and_legacy_dark_maps() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("workspace.json");
        assert_eq!(
            Workspace::load_from(&path).unwrap().theme,
            ThemePref::System
        );
        fs::write(&path, r#"{"dark": false}"#).unwrap();
        assert_eq!(
            Workspace::load_from(&path).unwrap().theme,
            ThemePref::System
        );
        fs::write(&path, r#"{"dark": true}"#).unwrap();
        assert_eq!(Workspace::load_from(&path).unwrap().theme, ThemePref::Dark);
        fs::write(&path, r#"{"dark": true, "theme": "Light"}"#).unwrap();
        assert_eq!(Workspace::load_from(&path).unwrap().theme, ThemePref::Light);
    }

    #[test]
    fn sidebar_collapsed_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("workspace.json");
        assert!(!Workspace::load_from(&path).unwrap().sidebar_collapsed);
        let ws = Workspace {
            sidebar_collapsed: true,
            ..Workspace::default()
        };
        ws.save_to(&path).unwrap();
        assert!(Workspace::load_from(&path).unwrap().sidebar_collapsed);
    }
}
