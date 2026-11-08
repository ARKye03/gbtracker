//! Application state and update loop. `update` and `view` never block: every git, git-bug and fs call runs in a `Task`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gbtracker::cache::{self, Cache, Snapshot};
use gbtracker::gitbug::{self, GitBugError, Issue, Status};
use gbtracker::workspace::{self, RepoInfo, Root, RootKind, Workspace};
use iced::{Element, Subscription, Task, Theme, event, time, window};
use tokio::sync::Semaphore;

use crate::theme::Mode;
use crate::{Section, ui};

const MAX_CONCURRENT: usize = 4;
const TRACK_EVERY: Duration = Duration::from_secs(5);
const SPIN_EVERY: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadState {
    Cached,
    Loading,
    Refreshing,
    Ready(usize),
    NoGitBug,
    Locked,
    Error(String),
}

impl LoadState {
    pub fn is_busy(&self) -> bool {
        matches!(self, LoadState::Loading | LoadState::Refreshing)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoEntry {
    pub info: RepoInfo,
    pub state: LoadState,
    /// Set only together with `issues`, so a match proves the issues are current.
    pub fingerprint: Option<String>,
    pub issues: Vec<Issue>,
}

impl RepoEntry {
    fn fresh(info: RepoInfo) -> Self {
        let state = if info.has_git_bug {
            LoadState::Loading
        } else {
            LoadState::NoGitBug
        };
        RepoEntry {
            info,
            state,
            fingerprint: None,
            issues: Vec::new(),
        }
    }

    pub fn open_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.status == Status::Open)
            .count()
    }

    pub fn closed_count(&self) -> usize {
        self.issues.len() - self.open_count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadFailure {
    Locked,
    Failed(String),
}

impl From<GitBugError> for LoadFailure {
    fn from(e: GitBugError) -> Self {
        match e {
            GitBugError::Locked { .. } => LoadFailure::Locked,
            other => LoadFailure::Failed(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Batch {
    pub total: usize,
    pub done: usize,
    /// Quiet re-checks stay hidden until something actually has to be reloaded.
    pub visible: bool,
}

pub struct App {
    pub mode: Mode,
    pub section: Section,
    pub workspace: Workspace,
    pub repos: Vec<RepoEntry>,
    pub discovering: bool,
    pub batch: Option<Batch>,
    pub last_synced: Option<SystemTime>,
    pub notice: Option<String>,
    pub spin: usize,
    /// Set when the config file was unreadable; saving is disabled so it is never overwritten.
    config_locked: bool,
    epoch: u64,
    cache: Option<Arc<Cache>>,
    permits: Arc<Semaphore>,
}

#[derive(Debug, Clone)]
pub enum Message {
    ToggleTheme,
    Navigate(Section),
    Noop,
    Discovered {
        epoch: u64,
        repos: Vec<RepoInfo>,
    },
    Fingerprinted {
        epoch: u64,
        root: PathBuf,
        result: Result<String, String>,
    },
    Loaded {
        epoch: u64,
        root: PathBuf,
        fingerprint: String,
        result: Result<Vec<Issue>, LoadFailure>,
    },
    Persisted(Result<(), String>),
    PickFolder(RootKind),
    FolderPicked(RootKind, Option<PathBuf>),
    RemoveRoot(usize),
    Rescan,
    Tick,
    Refocused,
    Spin,
}

impl App {
    pub fn boot() -> (Self, Task<Message>) {
        let (workspace, config_error) = match Workspace::load() {
            Ok(ws) => (ws, None),
            Err(e) => (Workspace::default(), Some(e.to_string())),
        };
        let cache = Cache::default_location().map(Arc::new);
        let snapshot = cache.as_ref().map(|c| c.load_snapshot());
        let mut app = Self::from_parts(workspace, snapshot.unwrap_or_default(), cache);
        if let Some(e) = config_error {
            app.config_locked = true;
            app.notice = Some(format!("{e} — changes will not be saved"));
        }
        if let Ok(theme) = std::env::var("GBTRACKER_THEME") {
            app.mode = if theme == "dark" {
                Mode::Dark
            } else {
                Mode::Light
            };
        }
        let task = app.start_discovery();
        (app, task)
    }

    pub fn from_parts(workspace: Workspace, snapshot: Snapshot, cache: Option<Arc<Cache>>) -> Self {
        let Snapshot { repos, mut issues } = snapshot;
        let repos = repos
            .into_iter()
            .map(|info| match issues.remove(&info.root) {
                Some(cached) if info.has_git_bug => RepoEntry {
                    state: LoadState::Cached,
                    fingerprint: Some(cached.fingerprint),
                    issues: cached.issues,
                    info,
                },
                _ => RepoEntry::fresh(info),
            })
            .collect();
        App {
            mode: if workspace.dark {
                Mode::Dark
            } else {
                Mode::Light
            },
            section: Section::default(),
            workspace,
            repos,
            discovering: false,
            batch: None,
            last_synced: None,
            notice: None,
            spin: 0,
            config_locked: false,
            epoch: 0,
            cache,
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT)),
        }
    }

    pub fn start_discovery(&mut self) -> Task<Message> {
        self.epoch += 1;
        self.discovering = true;
        self.batch = None;
        let (epoch, workspace) = (self.epoch, self.workspace.clone());
        Task::perform(
            async move { workspace::discover(&workspace).await },
            move |repos| Message::Discovered { epoch, repos },
        )
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ToggleTheme => {
                self.mode = self.mode.toggle();
                self.workspace.dark = self.mode == Mode::Dark;
                self.persist_workspace()
            }
            Message::Navigate(section) => {
                self.section = section;
                Task::none()
            }
            Message::Noop => Task::none(),
            Message::Discovered { epoch, repos } => self.on_discovered(epoch, repos),
            Message::Fingerprinted {
                epoch,
                root,
                result,
            } => self.on_fingerprinted(epoch, root, result),
            Message::Loaded {
                epoch,
                root,
                fingerprint,
                result,
            } => self.on_loaded(epoch, root, fingerprint, result),
            Message::Persisted(result) => {
                if let Err(e) = result {
                    self.notice = Some(format!("save failed: {e}"));
                }
                Task::none()
            }
            Message::PickFolder(kind) => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .pick_folder()
                        .await
                        .map(|h| h.path().to_path_buf())
                },
                move |path| Message::FolderPicked(kind, path),
            ),
            Message::FolderPicked(kind, path) => {
                let Some(path) = path else {
                    return Task::none();
                };
                if self.workspace.roots.iter().any(|r| r.path == path) {
                    return Task::none();
                }
                self.workspace.roots.push(Root { path, kind });
                Task::batch([self.persist_workspace(), self.start_discovery()])
            }
            Message::RemoveRoot(index) => {
                if index >= self.workspace.roots.len() {
                    return Task::none();
                }
                self.workspace.roots.remove(index);
                Task::batch([self.persist_workspace(), self.start_discovery()])
            }
            Message::Rescan => self.start_discovery(),
            Message::Tick | Message::Refocused => self.check_tracked(),
            Message::Spin => {
                self.spin = self.spin.wrapping_add(1);
                Task::none()
            }
        }
    }

    fn on_discovered(&mut self, epoch: u64, found: Vec<RepoInfo>) -> Task<Message> {
        if epoch != self.epoch {
            return Task::none();
        }
        self.discovering = false;

        let mut previous = std::mem::take(&mut self.repos);
        self.repos = found
            .iter()
            .cloned()
            .map(|info| {
                let Some(pos) = previous.iter().position(|e| e.info.root == info.root) else {
                    return RepoEntry::fresh(info);
                };
                let mut entry = previous.swap_remove(pos);
                let gained = info.has_git_bug && !entry.info.has_git_bug;
                entry.info = info;
                if !entry.info.has_git_bug {
                    entry.state = LoadState::NoGitBug;
                } else if gained {
                    entry.state = LoadState::Loading;
                }
                entry
            })
            .collect();

        let mut tasks = vec![self.persist_repos(found)];
        let roots: Vec<PathBuf> = self
            .repos
            .iter()
            .filter(|e| e.info.has_git_bug)
            .map(|e| e.info.root.clone())
            .collect();
        if roots.is_empty() {
            self.finish_batch();
        } else {
            self.batch = Some(Batch {
                total: roots.len(),
                done: 0,
                visible: true,
            });
            tasks.extend(roots.into_iter().map(|r| self.fingerprint_task(r)));
        }
        Task::batch(tasks)
    }

    fn on_fingerprinted(
        &mut self,
        epoch: u64,
        root: PathBuf,
        result: Result<String, String>,
    ) -> Task<Message> {
        if epoch != self.epoch {
            return Task::none();
        }
        let Some(entry) = self.repos.iter_mut().find(|e| e.info.root == root) else {
            return Task::none();
        };
        match result {
            Err(msg) => {
                entry.state = LoadState::Error(msg);
                self.complete_one();
                Task::none()
            }
            Ok(fp) if entry.fingerprint.as_deref() == Some(fp.as_str()) => {
                entry.state = LoadState::Ready(entry.issues.len());
                self.complete_one();
                Task::none()
            }
            Ok(fp) => {
                entry.state = if entry.fingerprint.is_some() {
                    LoadState::Refreshing
                } else {
                    LoadState::Loading
                };
                if let Some(batch) = &mut self.batch {
                    batch.visible = true;
                }
                self.list_task(epoch, root, fp)
            }
        }
    }

    fn on_loaded(
        &mut self,
        epoch: u64,
        root: PathBuf,
        fingerprint: String,
        result: Result<Vec<Issue>, LoadFailure>,
    ) -> Task<Message> {
        if epoch != self.epoch {
            return Task::none();
        }
        let Some(entry) = self.repos.iter_mut().find(|e| e.info.root == root) else {
            return Task::none();
        };
        let task = match result {
            Ok(issues) => {
                entry.state = LoadState::Ready(issues.len());
                entry.issues = issues;
                entry.fingerprint = Some(fingerprint.clone());
                let (info, issues) = (entry.info.clone(), entry.issues.clone());
                self.persist_issues(info, fingerprint, issues)
            }
            Err(LoadFailure::Locked) => {
                entry.state = LoadState::Locked;
                Task::none()
            }
            Err(LoadFailure::Failed(msg)) => {
                entry.state = LoadState::Error(msg);
                Task::none()
            }
        };
        self.complete_one();
        task
    }

    /// Re-fingerprints settled repos; skipped while any refresh is in flight.
    fn check_tracked(&mut self) -> Task<Message> {
        if self.discovering || self.batch.is_some() {
            return Task::none();
        }
        let roots: Vec<PathBuf> = self
            .repos
            .iter()
            .filter(|e| matches!(e.state, LoadState::Ready(_) | LoadState::Locked))
            .map(|e| e.info.root.clone())
            .collect();
        if roots.is_empty() {
            return Task::none();
        }
        self.epoch += 1;
        self.batch = Some(Batch {
            total: roots.len(),
            done: 0,
            visible: false,
        });
        Task::batch(roots.into_iter().map(|r| self.fingerprint_task(r)))
    }

    fn complete_one(&mut self) {
        let Some(batch) = &mut self.batch else {
            return;
        };
        batch.done += 1;
        if batch.done >= batch.total {
            self.finish_batch();
        }
    }

    fn finish_batch(&mut self) {
        self.batch = None;
        self.last_synced = Some(SystemTime::now());
    }

    fn fingerprint_task(&self, root: PathBuf) -> Task<Message> {
        let (epoch, permits) = (self.epoch, Arc::clone(&self.permits));
        Task::perform(
            async move {
                let _permit = permits.acquire_owned().await;
                let result = cache::fingerprint(&root).await.map_err(|e| e.to_string());
                (root, result)
            },
            move |(root, result)| Message::Fingerprinted {
                epoch,
                root,
                result,
            },
        )
    }

    fn list_task(&self, epoch: u64, root: PathBuf, fingerprint: String) -> Task<Message> {
        let permits = Arc::clone(&self.permits);
        Task::perform(
            async move {
                let _permit = permits.acquire_owned().await;
                let result = gitbug::list(&root).await.map_err(LoadFailure::from);
                (root, fingerprint, result)
            },
            move |(root, fingerprint, result)| Message::Loaded {
                epoch,
                root,
                fingerprint,
                result,
            },
        )
    }

    fn persist_workspace(&self) -> Task<Message> {
        if self.config_locked {
            return Task::none();
        }
        let workspace = self.workspace.clone();
        blocking(move || workspace.save().map_err(|e| e.to_string()))
    }

    fn persist_repos(&self, repos: Vec<RepoInfo>) -> Task<Message> {
        let Some(cache) = self.cache.clone() else {
            return Task::none();
        };
        blocking(move || cache.save_repos(&repos).map_err(|e| e.to_string()))
    }

    fn persist_issues(
        &self,
        info: RepoInfo,
        fingerprint: String,
        issues: Vec<Issue>,
    ) -> Task<Message> {
        let Some(cache) = self.cache.clone() else {
            return Task::none();
        };
        blocking(move || {
            cache
                .save_issues(&info, &fingerprint, &issues)
                .map_err(|e| e.to_string())
        })
    }

    pub fn is_busy(&self) -> bool {
        self.discovering || self.batch.is_some_and(|b| b.visible)
    }

    pub fn failed_count(&self) -> usize {
        self.repos
            .iter()
            .filter(|e| matches!(e.state, LoadState::Error(_)))
            .count()
    }

    pub fn view(&self) -> Element<'_, Message> {
        ui::screen::view(self)
    }

    pub fn theme(&self) -> Theme {
        self.mode.theme()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            time::every(TRACK_EVERY).map(|_| Message::Tick),
            event::listen_with(|event, _, _| match event {
                event::Event::Window(window::Event::Focused) => Some(Message::Refocused),
                _ => None,
            }),
        ];
        if self.is_busy() {
            subs.push(time::every(SPIN_EVERY).map(|_| Message::Spin));
        }
        Subscription::batch(subs)
    }
}

fn blocking(job: impl FnOnce() -> Result<(), String> + Send + 'static) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(job)
                .await
                .unwrap_or_else(|e| Err(e.to_string()))
        },
        Message::Persisted,
    )
}

/// "just now", "5m ago", "3h ago", "2d ago".
pub fn ago(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0..=9 => "just now".to_owned(),
        10..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use gbtracker::cache::CachedIssues;

    use super::*;

    fn info(name: &str) -> RepoInfo {
        RepoInfo {
            root: format!("/r/{name}").into(),
            name: name.into(),
            has_git_bug: true,
            common_dir: format!("/r/{name}/.git").into(),
        }
    }

    fn issue(status: Status) -> Issue {
        Issue {
            id: "a".repeat(64),
            human_id: "aaaaaaa".into(),
            title: "t".into(),
            status,
            kind: None,
            epic: None,
            state: None,
            layer: None,
            labels: vec![],
            author: "me".into(),
            created: String::new(),
            edited: String::new(),
            comment_count: 1,
        }
    }

    fn snapshot(names: &[&str]) -> Snapshot {
        let repos: Vec<RepoInfo> = names.iter().map(|n| info(n)).collect();
        let issues: HashMap<_, _> = repos
            .iter()
            .map(|r| {
                (
                    r.root.clone(),
                    CachedIssues {
                        fingerprint: format!("fp-{}", r.name),
                        issues: vec![issue(Status::Open)],
                    },
                )
            })
            .collect();
        Snapshot { repos, issues }
    }

    impl App {
        fn send(&mut self, message: Message) {
            let _ = self.update(message);
        }
    }

    fn state<'a>(app: &'a App, name: &str) -> &'a LoadState {
        &app.repos
            .iter()
            .find(|e| e.info.name == name)
            .unwrap()
            .state
    }

    fn booted(names: &[&str]) -> App {
        let mut app = App::from_parts(Workspace::default(), snapshot(names), None);
        app.discovering = true;
        app.epoch = 1;
        let task = app.update(Message::Discovered {
            epoch: 1,
            repos: names.iter().map(|n| info(n)).collect(),
        });
        assert_eq!(task.units(), names.len());
        app
    }

    #[test]
    fn snapshot_boot_renders_cached_before_any_load() {
        let app = App::from_parts(Workspace::default(), snapshot(&["a", "b"]), None);
        assert_eq!(app.repos.len(), 2);
        assert!(app.repos.iter().all(|e| e.state == LoadState::Cached));
        assert_eq!(app.repos[0].issues.len(), 1);
    }

    #[test]
    fn snapshot_repo_without_cached_issues_is_loading() {
        let mut snap = snapshot(&["a"]);
        snap.issues.clear();
        let app = App::from_parts(Workspace::default(), snap, None);
        assert_eq!(app.repos[0].state, LoadState::Loading);
    }

    #[test]
    fn loaded_moves_to_ready_and_locked_keeps_others() {
        let mut app = booted(&["a", "b", "c"]);
        for (name, fp) in [("a", "old"), ("b", "old"), ("c", "old")] {
            app.send(Message::Fingerprinted {
                epoch: 1,
                root: info(name).root,
                result: Ok(fp.into()),
            });
        }
        assert!(app.repos.iter().all(|e| e.state == LoadState::Refreshing));

        app.send(Message::Loaded {
            epoch: 1,
            root: info("a").root,
            fingerprint: "old".into(),
            result: Ok(vec![issue(Status::Open), issue(Status::Closed)]),
        });
        app.send(Message::Loaded {
            epoch: 1,
            root: info("b").root,
            fingerprint: "old".into(),
            result: Err(LoadFailure::Locked),
        });
        assert_eq!(*state(&app, "a"), LoadState::Ready(2));
        assert_eq!(*state(&app, "b"), LoadState::Locked);
        assert_eq!(*state(&app, "c"), LoadState::Refreshing);
        assert_eq!(app.repos.len(), 3);
        assert_eq!(app.repos[0].open_count(), 1);
        assert_eq!(app.repos[0].closed_count(), 1);
        assert_eq!(
            app.repos[1].issues.len(),
            1,
            "locked repo keeps cached issues"
        );
    }

    #[test]
    fn unchanged_fingerprint_spawns_no_list_task() {
        let mut app = booted(&["a"]);
        let task = app.update(Message::Fingerprinted {
            epoch: 1,
            root: info("a").root,
            result: Ok("fp-a".into()),
        });
        assert_eq!(task.units(), 0);
        assert_eq!(*state(&app, "a"), LoadState::Ready(1));
        assert!(app.batch.is_none());
        assert!(app.last_synced.is_some());
    }

    #[test]
    fn changed_fingerprint_spawns_one_list_task() {
        let mut app = booted(&["a"]);
        let task = app.update(Message::Fingerprinted {
            epoch: 1,
            root: info("a").root,
            result: Ok("new".into()),
        });
        assert_eq!(task.units(), 1);
        assert_eq!(*state(&app, "a"), LoadState::Refreshing);
    }

    #[test]
    fn fingerprint_error_marks_only_that_repo() {
        let mut app = booted(&["a", "b"]);
        app.send(Message::Fingerprinted {
            epoch: 1,
            root: info("a").root,
            result: Err("gone".into()),
        });
        assert_eq!(*state(&app, "a"), LoadState::Error("gone".into()));
        assert_eq!(*state(&app, "b"), LoadState::Cached);
        assert_eq!(app.failed_count(), 1);
    }

    #[test]
    fn stale_epoch_results_are_dropped() {
        let mut app = booted(&["a"]);
        app.send(Message::Rescan);
        let task = app.update(Message::Fingerprinted {
            epoch: 1,
            root: info("a").root,
            result: Ok("new".into()),
        });
        assert_eq!(task.units(), 0);
        assert_eq!(*state(&app, "a"), LoadState::Cached);
    }

    #[test]
    fn tick_is_skipped_while_refresh_in_flight_and_checks_ready_when_idle() {
        let mut app = booted(&["a", "b"]);
        assert_eq!(app.update(Message::Tick).units(), 0);

        for n in ["a", "b"] {
            app.send(Message::Fingerprinted {
                epoch: 1,
                root: info(n).root,
                result: Ok(format!("fp-{n}")),
            });
        }
        assert!(app.batch.is_none());
        let task = app.update(Message::Tick);
        assert_eq!(task.units(), 2);
        assert!(!app.is_busy(), "quiet check shows no progress");
        assert_eq!(app.update(Message::Refocused).units(), 0);
    }

    #[test]
    fn discovery_drops_removed_repos_and_flags_missing_git_bug() {
        let mut app = booted(&["a", "b"]);
        let mut a = info("a");
        a.has_git_bug = false;
        app.epoch = 2;
        app.send(Message::Discovered {
            epoch: 2,
            repos: vec![a],
        });
        assert_eq!(app.repos.len(), 1);
        assert_eq!(*state(&app, "a"), LoadState::NoGitBug);
        assert!(app.batch.is_none());
    }

    #[test]
    fn folder_pick_adds_root_once() {
        let mut app = App::from_parts(Workspace::default(), Snapshot::default(), None);
        app.config_locked = true;
        app.send(Message::FolderPicked(RootKind::Scan, Some("/x".into())));
        app.send(Message::FolderPicked(RootKind::Project, Some("/x".into())));
        app.send(Message::FolderPicked(RootKind::Project, None));
        assert_eq!(app.workspace.roots.len(), 1);
        assert_eq!(app.workspace.roots[0].kind, RootKind::Scan);
        assert!(app.discovering);
        app.send(Message::RemoveRoot(5));
        app.send(Message::RemoveRoot(0));
        assert!(app.workspace.roots.is_empty());
    }

    #[test]
    fn theme_toggle_updates_workspace_flag() {
        let mut app = App::from_parts(Workspace::default(), Snapshot::default(), None);
        app.config_locked = true;
        app.send(Message::ToggleTheme);
        assert!(app.workspace.dark);
        app.send(Message::ToggleTheme);
        assert!(!app.workspace.dark);
    }

    #[test]
    fn workspace_dark_selects_initial_mode() {
        let ws = Workspace {
            dark: true,
            ..Workspace::default()
        };
        let app = App::from_parts(ws, Snapshot::default(), None);
        assert_eq!(app.mode, Mode::Dark);
    }

    #[test]
    fn ago_formats() {
        assert_eq!(ago(Duration::from_secs(3)), "just now");
        assert_eq!(ago(Duration::from_secs(30)), "30s ago");
        assert_eq!(ago(Duration::from_secs(120)), "2m ago");
        assert_eq!(ago(Duration::from_secs(7200)), "2h ago");
        assert_eq!(ago(Duration::from_secs(3 * 86_400)), "3d ago");
    }
}
