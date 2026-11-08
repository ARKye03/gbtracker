//! Application state and update loop. `update` and `view` never block: every git, git-bug and fs call runs in a `Task`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gbtracker::cache::{self, Cache, Snapshot};
use gbtracker::detail::{DetailCache, LoadedDetail};
use gbtracker::epics::{self, EpicSummary};
use gbtracker::filter::{self, Filter, StateFilter, StatusFilter};
use gbtracker::gitbug::{self, GitBugError, Issue, IssueDetail, Kind, Status, WriteOp};
use gbtracker::workspace::{self, RepoInfo, Root, RootKind, Workspace};
use gbtracker::writes::WriteQueue;
use iced::keyboard::{self, key::Named};
use iced::widget::text_editor;
use iced::{Element, Subscription, Task, Theme, clipboard, event, time, window};
use tokio::sync::Semaphore;

use crate::theme::Mode;
use crate::{Section, ui};

const MAX_CONCURRENT: usize = 4;
const TRACK_EVERY: Duration = Duration::from_secs(5);
const SPIN_EVERY: Duration = Duration::from_millis(250);
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);
pub const PAGE_SIZE: usize = 200;

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
    /// Lowercased search text per issue, parallel to `issues`.
    keys: Vec<String>,
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
            keys: Vec::new(),
        }
    }

    fn replace_issue(&mut self, pos: usize, issue: Issue) {
        self.keys[pos] = filter::search_key(&issue);
        self.issues[pos] = issue;
    }

    fn set_issues(&mut self, issues: Vec<Issue>) {
        self.keys = issues.iter().map(filter::search_key).collect();
        self.issues = issues;
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

/// One visible issue, addressed by position in `App::repos` and that repo's `issues`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub repo: usize,
    pub issue: usize,
    pub depth: usize,
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

#[derive(Debug, Clone)]
pub enum DetailState {
    Loading,
    Ready(Rc<LoadedDetail>),
    Failed(String),
}

/// The open issue pane. `summary` is the list row the pane was opened from; its `edited` decides what to fetch.
#[derive(Debug, Clone)]
pub struct DetailPane {
    pub root: PathBuf,
    pub summary: Issue,
    pub state: DetailState,
}

impl DetailPane {
    fn is(&self, root: &Path, id: &str, edited: &str) -> bool {
        self.root == root && self.summary.id == id && self.summary.edited == edited
    }
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
    pub filter: Filter,
    /// Raw search box content; `filter` only sees it once typing settles.
    pub search: String,
    pub roadmap: bool,
    /// Status and repo only; the Epics page has no other controls.
    pub epic_filter: Filter,
    pub epics: Vec<EpicSummary>,
    /// Expanded epic cards, keyed by repo root and epic id so they survive reloads.
    pub expanded: HashSet<(PathBuf, String)>,
    pub detail: Option<DetailPane>,
    detail_cache: DetailCache,
    /// Issues with a write in flight, by repo root and id; their actions stay disabled.
    pub pending: HashSet<(PathBuf, String)>,
    /// Last failed write per issue; cleared when the next one starts.
    pub write_errors: HashMap<(PathBuf, String), String>,
    /// Unsent comment text per issue, kept across pane switches.
    pub drafts: HashMap<(PathBuf, String), text_editor::Content>,
    writes: WriteQueue,
    pub initials: String,
    identity_asked: bool,
    /// Filtered (and, for the roadmap view, ordered) result, recomputed in `update`.
    pub rows: Vec<Row>,
    pub shown: usize,
    search_seq: u64,
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
    ToggleKind(Kind),
    SetStatus(StatusFilter),
    SetState(StateFilter),
    PickRepo(Option<PathBuf>),
    SearchInput(String),
    SearchSettled(u64),
    ToggleRoadmap,
    SetEpicStatus(StatusFilter),
    PickEpicRepo(Option<PathBuf>),
    ToggleEpic(PathBuf, String),
    SelectIssue {
        root: PathBuf,
        id: String,
    },
    CloseDetail,
    DetailLoaded {
        root: PathBuf,
        id: String,
        edited: String,
        result: Result<Box<IssueDetail>, String>,
    },
    Write {
        root: PathBuf,
        id: String,
        op: WriteOp,
    },
    WriteDone {
        root: PathBuf,
        id: String,
        op: WriteOp,
        result: Result<Box<IssueDetail>, String>,
    },
    EditComment {
        root: PathBuf,
        id: String,
        action: text_editor::Action,
    },
    SubmitComment {
        root: PathBuf,
        id: String,
    },
    Copy(String),
    Identified(Option<String>),
    ClearFilters,
    ShowMore,
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
                Some(cached) if info.has_git_bug => {
                    let mut entry = RepoEntry {
                        state: LoadState::Cached,
                        fingerprint: Some(cached.fingerprint),
                        issues: Vec::new(),
                        keys: Vec::new(),
                        info,
                    };
                    entry.set_issues(cached.issues);
                    entry
                }
                _ => RepoEntry::fresh(info),
            })
            .collect();
        let mut app = App {
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
            filter: Filter::default(),
            search: String::new(),
            roadmap: false,
            epic_filter: Filter::default(),
            epics: Vec::new(),
            expanded: HashSet::new(),
            detail: None,
            detail_cache: DetailCache::default(),
            pending: HashSet::new(),
            write_errors: HashMap::new(),
            drafts: HashMap::new(),
            writes: WriteQueue::default(),
            initials: "?".to_owned(),
            identity_asked: false,
            rows: Vec::new(),
            shown: PAGE_SIZE,
            search_seq: 0,
            config_locked: false,
            epoch: 0,
            cache,
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT)),
        };
        app.refilter();
        app
    }

    /// Recomputes `rows` from the loaded issues; the only place filtering and ordering happen.
    fn refilter(&mut self) {
        let hits: Vec<(usize, usize)> = self
            .repos
            .iter()
            .enumerate()
            .flat_map(|(r, entry)| {
                let root = entry.info.root.as_path();
                let filter = &self.filter;
                entry
                    .issues
                    .iter()
                    .zip(&entry.keys)
                    .enumerate()
                    .filter(move |(_, (issue, key))| filter::matches(issue, root, key, filter))
                    .map(move |(i, _)| (r, i))
            })
            .collect();

        self.rows = if self.roadmap {
            let refs: Vec<(usize, &Issue)> = hits
                .iter()
                .map(|&(r, i)| (r, &self.repos[r].issues[i]))
                .collect();
            filter::roadmap_order(&refs)
                .into_iter()
                .map(|p| Row {
                    repo: hits[p.index].0,
                    issue: hits[p.index].1,
                    depth: p.depth,
                })
                .collect()
        } else {
            hits.into_iter()
                .map(|(repo, issue)| Row {
                    repo,
                    issue,
                    depth: 0,
                })
                .collect()
        };
        self.refresh_epics();
    }

    fn refresh_epics(&mut self) {
        let repos: Vec<(&Path, &[Issue])> = self
            .repos
            .iter()
            .map(|e| (e.info.root.as_path(), e.issues.as_slice()))
            .collect();
        let summaries = epics::summarize(&repos, &self.epic_filter);
        self.epics = summaries;
    }

    fn apply_filter(&mut self) {
        self.shown = PAGE_SIZE;
        self.refilter();
    }

    pub fn total_issues(&self) -> usize {
        self.repos.iter().map(|e| e.issues.len()).sum()
    }

    pub fn loading_repos(&self) -> usize {
        self.repos
            .iter()
            .filter(|e| e.fingerprint.is_none() && e.state.is_busy())
            .count()
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
            Message::ToggleKind(kind) => {
                self.filter.toggle_kind(kind);
                self.apply_filter();
                Task::none()
            }
            Message::SetStatus(status) => {
                self.filter.status = status;
                self.roadmap = false;
                self.apply_filter();
                Task::none()
            }
            Message::SetState(state) => {
                self.filter.state = state;
                self.apply_filter();
                Task::none()
            }
            Message::PickRepo(root) => {
                self.filter.repos.clear();
                self.filter.repos.extend(root);
                self.apply_filter();
                Task::none()
            }
            Message::SearchInput(text) => {
                self.search = text;
                self.search_seq += 1;
                let seq = self.search_seq;
                Task::perform(
                    async { tokio::time::sleep(SEARCH_DEBOUNCE).await },
                    move |()| Message::SearchSettled(seq),
                )
            }
            Message::SearchSettled(seq) => {
                if seq == self.search_seq {
                    self.filter.set_text(&self.search);
                    self.apply_filter();
                }
                Task::none()
            }
            Message::ToggleRoadmap => {
                self.roadmap = !self.roadmap;
                if self.roadmap {
                    self.filter.status = StatusFilter::Open;
                }
                self.apply_filter();
                Task::none()
            }
            Message::SetEpicStatus(status) => {
                self.epic_filter.status = status;
                self.refresh_epics();
                Task::none()
            }
            Message::PickEpicRepo(root) => {
                self.epic_filter.repos.clear();
                self.epic_filter.repos.extend(root);
                self.refresh_epics();
                Task::none()
            }
            Message::ToggleEpic(root, id) => {
                let key = (root, id);
                if !self.expanded.remove(&key) {
                    self.expanded.insert(key);
                }
                Task::none()
            }
            Message::SelectIssue { root, id } => {
                let issue = self
                    .repos
                    .iter()
                    .find(|e| e.info.root == root)
                    .and_then(|e| e.issues.iter().find(|i| i.id == id))
                    .cloned();
                match issue {
                    Some(issue) => self.open_detail(root, issue),
                    None => Task::none(),
                }
            }
            Message::CloseDetail => {
                self.detail = None;
                Task::none()
            }
            Message::DetailLoaded {
                root,
                id,
                edited,
                result,
            } => {
                match result {
                    Ok(detail) => {
                        let loaded = Rc::new(LoadedDetail::new(*detail));
                        self.detail_cache.insert(
                            root.clone(),
                            id.clone(),
                            edited.clone(),
                            loaded.clone(),
                        );
                        if let Some(pane) =
                            self.detail.as_mut().filter(|p| p.is(&root, &id, &edited))
                        {
                            pane.state = DetailState::Ready(loaded);
                        }
                    }
                    Err(msg) => {
                        if let Some(pane) =
                            self.detail.as_mut().filter(|p| p.is(&root, &id, &edited))
                        {
                            pane.state = DetailState::Failed(msg);
                        }
                    }
                }
                Task::none()
            }
            Message::Write { root, id, op } => self.start_write(root, id, op),
            Message::WriteDone {
                root,
                id,
                op,
                result,
            } => self.on_written(root, id, op, result),
            Message::EditComment { root, id, action } => {
                if let Some(draft) = self.drafts.get_mut(&(root, id)) {
                    draft.perform(action);
                }
                Task::none()
            }
            Message::SubmitComment { root, id } => {
                let body = self
                    .drafts
                    .get(&(root.clone(), id.clone()))
                    .map(|d| d.text().trim_end().to_owned())
                    .unwrap_or_default();
                if body.trim().is_empty() {
                    self.write_errors
                        .insert((root, id), GitBugError::EmptyComment.to_string());
                    return Task::none();
                }
                self.start_write(root, id, WriteOp::Comment(body))
            }
            Message::Copy(text) => clipboard::write(text),
            Message::Identified(name) => {
                match name.as_deref().map(gitbug::initials) {
                    Some(initials) => self.initials = initials,
                    None => self.identity_asked = false,
                }
                Task::none()
            }
            Message::ClearFilters => {
                self.filter = Filter::default();
                self.search.clear();
                self.search_seq += 1;
                self.roadmap = false;
                self.apply_filter();
                Task::none()
            }
            Message::ShowMore => {
                self.shown += PAGE_SIZE;
                Task::none()
            }
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

    /// Shows the pane at once from the in-memory summary; fetches only when nothing current is cached or in flight.
    fn open_detail(&mut self, root: PathBuf, issue: Issue) -> Task<Message> {
        self.drafts
            .entry((root.clone(), issue.id.clone()))
            .or_default();
        let same = self.detail.as_ref().is_some_and(|p| {
            p.is(&root, &issue.id, &issue.edited) && !matches!(p.state, DetailState::Failed(_))
        });
        if same {
            return Task::none();
        }
        if let Some(cached) = self.detail_cache.get(&root, &issue.id, &issue.edited) {
            self.detail = Some(DetailPane {
                root,
                summary: issue,
                state: DetailState::Ready(cached),
            });
            return Task::none();
        }
        let (id, edited) = (issue.id.clone(), issue.edited.clone());
        self.detail = Some(DetailPane {
            root: root.clone(),
            summary: issue,
            state: DetailState::Loading,
        });
        self.detail_task(root, id, edited)
    }

    /// Re-opens the pane when a reload brought a newer edit of the shown issue.
    fn sync_detail(&mut self, root: &Path) -> Task<Message> {
        let Some(pane) = self.detail.as_ref().filter(|p| p.root == root) else {
            return Task::none();
        };
        let live = self
            .repos
            .iter()
            .find(|e| e.info.root == root)
            .and_then(|e| e.issues.iter().find(|i| i.id == pane.summary.id))
            .filter(|i| i.edited != pane.summary.edited)
            .cloned();
        match live {
            Some(issue) => self.open_detail(root.to_owned(), issue),
            None => Task::none(),
        }
    }

    pub fn is_pending(&self, root: &Path, id: &str) -> bool {
        self.pending.iter().any(|(r, i)| r == root && i == id)
    }

    /// Queues behind earlier writes to the same repo; other repos and the rest of the UI keep running.
    fn start_write(&mut self, root: PathBuf, id: String, op: WriteOp) -> Task<Message> {
        let key = (root.clone(), id.clone());
        if self.pending.contains(&key) {
            return Task::none();
        }
        self.write_errors.remove(&key);
        self.pending.insert(key);
        let ticket = self.writes.enter(&root);
        Task::perform(
            async move {
                ticket.turn().await;
                let result = gitbug::apply(&root, &id, op.clone())
                    .await
                    .map(Box::new)
                    .map_err(|e| e.to_string());
                (root, id, op, result)
            },
            |(root, id, op, result)| Message::WriteDone {
                root,
                id,
                op,
                result,
            },
        )
    }

    /// A failed write changes nothing locally; a successful one replaces the issue everywhere it is held.
    fn on_written(
        &mut self,
        root: PathBuf,
        id: String,
        op: WriteOp,
        result: Result<Box<IssueDetail>, String>,
    ) -> Task<Message> {
        let key = (root.clone(), id.clone());
        self.pending.remove(&key);
        let detail = match result {
            Ok(detail) => *detail,
            Err(msg) => {
                self.write_errors.insert(key, msg);
                return Task::none();
            }
        };
        if matches!(op, WriteOp::Comment(_)) {
            self.drafts.insert(key, text_editor::Content::new());
        }
        let issue = detail.issue.clone();
        let loaded = Rc::new(LoadedDetail::new(detail));
        self.detail_cache.insert(
            root.clone(),
            id.clone(),
            issue.edited.clone(),
            loaded.clone(),
        );
        if let Some(pane) = self
            .detail
            .as_mut()
            .filter(|p| p.root == root && p.summary.id == id)
        {
            pane.summary = issue.clone();
            pane.state = DetailState::Ready(loaded);
        }
        self.store_issue(&root, issue)
    }

    /// Patches one issue in place and rewrites the snapshot; the old fingerprint stays, so the next check still reloads.
    fn store_issue(&mut self, root: &Path, issue: Issue) -> Task<Message> {
        let Some(entry) = self.repos.iter_mut().find(|e| e.info.root == root) else {
            return Task::none();
        };
        let Some(pos) = entry.issues.iter().position(|i| i.id == issue.id) else {
            return Task::none();
        };
        entry.replace_issue(pos, issue);
        let persist = entry
            .fingerprint
            .clone()
            .map(|fp| (entry.info.clone(), fp, entry.issues.clone()));
        self.refilter();
        match persist {
            Some((info, fp, issues)) => self.persist_issues(info, fp, issues),
            None => Task::none(),
        }
    }

    fn detail_task(&self, root: PathBuf, id: String, edited: String) -> Task<Message> {
        let permits = Arc::clone(&self.permits);
        Task::perform(
            async move {
                let _permit = permits.acquire_owned().await;
                let result = gitbug::show(&root, &id)
                    .await
                    .map(Box::new)
                    .map_err(|e| e.to_string());
                (root, id, edited, result)
            },
            |(root, id, edited, result)| Message::DetailLoaded {
                root,
                id,
                edited,
                result,
            },
        )
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
                    entry.set_issues(Vec::new());
                    entry.fingerprint = None;
                } else if gained {
                    entry.state = LoadState::Loading;
                }
                entry
            })
            .collect();
        let known: Vec<&PathBuf> = self.repos.iter().map(|e| &e.info.root).collect();
        self.filter.repos.retain(|r| known.contains(&r));
        self.epic_filter.repos.retain(|r| known.contains(&r));
        if self
            .detail
            .as_ref()
            .is_some_and(|p| !known.contains(&&p.root))
        {
            self.detail = None;
        }
        self.refilter();

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
            if !self.identity_asked {
                self.identity_asked = true;
                let root = roots[0].clone();
                tasks.push(Task::perform(
                    async move { gitbug::current_user(&root).await.ok() },
                    Message::Identified,
                ));
            }
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
                entry.set_issues(issues);
                entry.fingerprint = Some(fingerprint.clone());
                let (info, issues) = (entry.info.clone(), entry.issues.clone());
                self.refilter();
                Task::batch([
                    self.persist_issues(info, fingerprint, issues),
                    self.sync_detail(&root),
                ])
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
            event::listen_with(|event, status, _| match event {
                event::Event::Window(window::Event::Focused) => Some(Message::Refocused),
                event::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(Named::Escape),
                    ..
                }) if status == event::Status::Ignored => Some(Message::CloseDetail),
                _ => None,
            }),
        ];
        if self.is_busy() || !self.pending.is_empty() {
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
        app.identity_asked = true;
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

    fn titled(title: &str, status: Status) -> Issue {
        Issue {
            title: title.into(),
            ..issue(status)
        }
    }

    fn row_titles(app: &App) -> Vec<&str> {
        app.rows
            .iter()
            .map(|r| app.repos[r.repo].issues[r.issue].title.as_str())
            .collect()
    }

    fn loaded(titles: &[(&str, Status)]) -> App {
        let mut app = booted(&["a"]);
        app.send(Message::Fingerprinted {
            epoch: 1,
            root: info("a").root,
            result: Ok("new".into()),
        });
        app.send(Message::Loaded {
            epoch: 1,
            root: info("a").root,
            fingerprint: "new".into(),
            result: Ok(titles.iter().map(|(t, s)| titled(t, *s)).collect()),
        });
        app
    }

    #[test]
    fn rows_default_to_open_and_follow_loaded_data() {
        let app = loaded(&[("one", Status::Open), ("two", Status::Closed)]);
        assert_eq!(row_titles(&app), ["one"]);
        assert_eq!(app.total_issues(), 2);
    }

    #[test]
    fn filter_messages_recompute_rows() {
        let mut app = loaded(&[("one", Status::Open), ("two", Status::Closed)]);
        app.send(Message::SetStatus(StatusFilter::All));
        assert_eq!(row_titles(&app), ["one", "two"]);
        app.send(Message::SetStatus(StatusFilter::Closed));
        assert_eq!(row_titles(&app), ["two"]);
        app.send(Message::PickRepo(Some("/r/other".into())));
        assert!(app.rows.is_empty());
        app.send(Message::PickRepo(None));
        assert_eq!(row_titles(&app), ["two"]);
        app.send(Message::ClearFilters);
        assert_eq!(row_titles(&app), ["one"]);
    }

    #[test]
    fn search_is_debounced_to_the_latest_keystroke() {
        let mut app = loaded(&[("alpha", Status::Open), ("beta", Status::Open)]);
        assert_eq!(app.update(Message::SearchInput("be".into())).units(), 1);
        app.send(Message::SearchInput("bet".into()));
        app.send(Message::SearchSettled(1));
        assert_eq!(app.rows.len(), 2, "stale timer ignored");
        app.send(Message::SearchSettled(2));
        assert_eq!(row_titles(&app), ["beta"]);
        assert_eq!(app.search, "bet");
    }

    #[test]
    fn roadmap_forces_open_and_status_change_leaves_it() {
        let mut app = loaded(&[("one", Status::Open), ("two", Status::Closed)]);
        app.send(Message::SetStatus(StatusFilter::All));
        app.send(Message::ToggleRoadmap);
        assert!(app.roadmap);
        assert_eq!(app.filter.status, StatusFilter::Open);
        assert_eq!(row_titles(&app), ["one"]);
        app.send(Message::SetStatus(StatusFilter::All));
        assert!(!app.roadmap);
    }

    #[test]
    fn show_more_pages_through_rows_without_refiltering() {
        let mut app = loaded(&[("one", Status::Open)]);
        assert_eq!(app.shown, PAGE_SIZE);
        app.send(Message::ShowMore);
        assert_eq!(app.shown, 2 * PAGE_SIZE);
        app.send(Message::ToggleKind(Kind::Bug));
        assert_eq!(app.shown, PAGE_SIZE);
    }

    fn epic_app() -> App {
        let mut app = booted(&["a"]);
        let epic = Issue {
            kind: Some(Kind::Epic),
            ..titled("epic", Status::Open)
        };
        let kid = Issue {
            kind: Some(Kind::Task),
            epic: Some(epic.id.clone()),
            ..titled("kid", Status::Closed)
        };
        app.send(Message::Fingerprinted {
            epoch: 1,
            root: info("a").root,
            result: Ok("new".into()),
        });
        app.send(Message::Loaded {
            epoch: 1,
            root: info("a").root,
            fingerprint: "new".into(),
            result: Ok(vec![epic, kid]),
        });
        app
    }

    #[test]
    fn epics_follow_loaded_data_and_their_own_filter() {
        let mut app = epic_app();
        assert_eq!(app.epics.len(), 1);
        assert_eq!((app.epics[0].total, app.epics[0].closed), (1, 1));

        app.send(Message::SetEpicStatus(StatusFilter::Closed));
        assert!(app.epics.is_empty());
        app.send(Message::SetEpicStatus(StatusFilter::All));
        assert_eq!(app.epics.len(), 1);
        assert_eq!(app.filter, Filter::default(), "issue filter untouched");

        app.send(Message::PickEpicRepo(Some("/r/other".into())));
        assert!(app.epics.is_empty());
        app.send(Message::PickEpicRepo(None));
        assert_eq!(app.epics.len(), 1);
    }

    #[test]
    fn epic_expansion_toggles_per_repo_and_id() {
        let mut app = epic_app();
        let (root, id) = (info("a").root, "a".repeat(64));
        app.send(Message::ToggleEpic(root.clone(), id.clone()));
        assert!(app.expanded.contains(&(root.clone(), id.clone())));
        app.send(Message::ToggleEpic(root, id));
        assert!(app.expanded.is_empty());
    }

    fn select(app: &mut App) -> Task<Message> {
        app.update(Message::SelectIssue {
            root: info("a").root,
            id: "a".repeat(64),
        })
    }

    fn detail_msg(app: &App, edited: &str, body: &str) -> Message {
        let mut issue = app.repos[0].issues[0].clone();
        issue.edited = edited.into();
        Message::DetailLoaded {
            root: info("a").root,
            id: issue.id.clone(),
            edited: edited.into(),
            result: Ok(Box::new(IssueDetail {
                issue,
                body: body.into(),
                comments: vec![],
            })),
        }
    }

    #[test]
    fn selecting_opens_a_loading_pane_and_emits_one_load() {
        let mut app = loaded(&[("one", Status::Open)]);
        assert!(app.detail.is_none());
        assert_eq!(select(&mut app).units(), 1);
        let pane = app.detail.as_ref().unwrap();
        assert!(matches!(pane.state, DetailState::Loading));
        assert_eq!(pane.summary.title, "one");
        assert_eq!(pane.root, info("a").root);

        assert_eq!(select(&mut app).units(), 0, "already loading");
    }

    #[test]
    fn loaded_detail_is_stored_for_its_repo_and_id() {
        let mut app = loaded(&[("one", Status::Open)]);
        let _ = select(&mut app);
        let msg = detail_msg(&app, "", "## Hi\n\n```rs\nlet x = 1;\n```");
        app.send(msg);
        let Some(DetailState::Ready(loaded)) = app.detail.as_ref().map(|p| &p.state) else {
            panic!("expected a ready pane");
        };
        assert_eq!(loaded.body.items().len(), 2);
        let root = info("a").root;
        assert!(app.detail_cache.get(&root, &"a".repeat(64), "").is_some());
        assert!(
            app.detail_cache
                .get(&info("b").root, &"a".repeat(64), "")
                .is_none()
        );
        assert!(app.detail_cache.get(&root, "other", "").is_none());
    }

    #[test]
    fn reselecting_an_unchanged_issue_emits_no_task() {
        let mut app = loaded(&[("one", Status::Open)]);
        let _ = select(&mut app);
        let msg = detail_msg(&app, "", "body");
        app.send(msg);

        assert_eq!(select(&mut app).units(), 0, "pane already current");
        app.send(Message::CloseDetail);
        assert!(app.detail.is_none());
        assert_eq!(select(&mut app).units(), 0, "served from the cache");
        assert!(matches!(
            app.detail.as_ref().unwrap().state,
            DetailState::Ready(_)
        ));
    }

    #[test]
    fn a_newer_edit_time_refetches() {
        let mut app = loaded(&[("one", Status::Open)]);
        let _ = select(&mut app);
        let msg = detail_msg(&app, "", "old");
        app.send(msg);

        app.repos[0].issues[0].edited = "2026-11-09T00:00:00Z".into();
        assert_eq!(select(&mut app).units(), 1);
        assert!(matches!(
            app.detail.as_ref().unwrap().state,
            DetailState::Loading
        ));
    }

    #[test]
    fn list_reload_with_a_newer_edit_refreshes_the_open_pane() {
        let mut app = loaded(&[("one", Status::Open)]);
        let _ = select(&mut app);
        let msg = detail_msg(&app, "", "old");
        app.send(msg);

        let mut newer = app.repos[0].issues[0].clone();
        newer.edited = "2026-11-09T00:00:00Z".into();
        app.epoch = 2;
        let task = app.update(Message::Loaded {
            epoch: 2,
            root: info("a").root,
            fingerprint: "newer".into(),
            result: Ok(vec![newer]),
        });
        assert_eq!(task.units(), 1);
        assert!(matches!(
            app.detail.as_ref().unwrap().state,
            DetailState::Loading
        ));
    }

    #[test]
    fn failed_load_can_be_retried_and_stale_results_are_ignored() {
        let mut app = loaded(&[("one", Status::Open)]);
        let _ = select(&mut app);
        app.send(Message::DetailLoaded {
            root: info("a").root,
            id: "a".repeat(64),
            edited: "stale".into(),
            result: Err("nope".into()),
        });
        assert!(matches!(
            app.detail.as_ref().unwrap().state,
            DetailState::Loading
        ));

        app.send(Message::DetailLoaded {
            root: info("a").root,
            id: "a".repeat(64),
            edited: String::new(),
            result: Err("nope".into()),
        });
        assert!(matches!(
            app.detail.as_ref().unwrap().state,
            DetailState::Failed(_)
        ));
        assert_eq!(select(&mut app).units(), 1);
    }

    #[test]
    fn dropping_the_repo_closes_its_pane() {
        let mut app = loaded(&[("one", Status::Open)]);
        let _ = select(&mut app);
        app.epoch = 2;
        app.send(Message::Discovered {
            epoch: 2,
            repos: vec![],
        });
        assert!(app.detail.is_none());
    }

    fn key() -> (PathBuf, String) {
        (info("a").root, "a".repeat(64))
    }

    fn write(op: WriteOp) -> Message {
        Message::Write {
            root: info("a").root,
            id: "a".repeat(64),
            op,
        }
    }

    fn written(app: &App, op: WriteOp, edit: impl FnOnce(&mut Issue)) -> Message {
        let mut issue = app.repos[0].issues[0].clone();
        edit(&mut issue);
        Message::WriteDone {
            root: info("a").root,
            id: issue.id.clone(),
            op,
            result: Ok(Box::new(IssueDetail {
                issue,
                body: "body".into(),
                comments: vec![],
            })),
        }
    }

    #[test]
    fn a_write_marks_the_issue_pending_and_rejects_a_second_one() {
        let mut app = loaded(&[("one", Status::Open)]);
        assert_eq!(app.update(write(WriteOp::Close)).units(), 1);
        assert!(app.pending.contains(&key()));
        assert!(app.is_pending(&info("a").root, &"a".repeat(64)));
        assert_eq!(app.update(write(WriteOp::Close)).units(), 0);
    }

    #[test]
    fn a_failed_write_keeps_the_issue_and_shows_the_error() {
        let mut app = loaded(&[("one", Status::Open)]);
        let before = app.repos[0].issues.clone();
        let _ = select(&mut app);
        let _ = app.update(write(WriteOp::Close));
        app.send(Message::WriteDone {
            root: info("a").root,
            id: "a".repeat(64),
            op: WriteOp::Close,
            result: Err(GitBugError::Locked { pid: Some(7) }.to_string()),
        });
        assert!(app.pending.is_empty());
        assert_eq!(app.repos[0].issues, before);
        assert_eq!(row_titles(&app), ["one"]);
        assert!(app.write_errors[&key()].contains("locked"));
        assert_eq!(app.detail.as_ref().unwrap().summary, before[0]);

        let _ = app.update(write(WriteOp::Close));
        assert!(app.write_errors.is_empty(), "next write clears the error");
    }

    #[test]
    fn a_successful_write_updates_rows_pane_and_cache() {
        let mut app = loaded(&[("one", Status::Open)]);
        let _ = select(&mut app);
        let _ = app.update(write(WriteOp::Close));
        let msg = written(&app, WriteOp::Close, |i| {
            i.status = Status::Closed;
            i.edited = "2026-11-09T00:00:00Z".into();
        });
        app.send(msg);

        assert!(app.pending.is_empty());
        assert_eq!(app.repos[0].issues[0].status, Status::Closed);
        assert!(app.rows.is_empty(), "closed issue leaves the open filter");
        let pane = app.detail.as_ref().unwrap();
        assert_eq!(pane.summary.status, Status::Closed);
        assert!(matches!(pane.state, DetailState::Ready(_)));
        assert!(
            app.detail_cache
                .get(&info("a").root, &"a".repeat(64), "2026-11-09T00:00:00Z")
                .is_some()
        );
        assert_eq!(select(&mut app).units(), 0, "pane is current, no refetch");
    }

    #[test]
    fn empty_comments_are_never_sent_and_sent_ones_clear_the_draft() {
        let mut app = loaded(&[("one", Status::Open)]);
        let _ = select(&mut app);
        let submit = || Message::SubmitComment {
            root: info("a").root,
            id: "a".repeat(64),
        };
        assert_eq!(app.update(submit()).units(), 0);
        assert!(app.write_errors[&key()].contains("empty"));
        assert!(app.pending.is_empty());

        app.drafts
            .insert(key(), text_editor::Content::with_text("## Hi"));
        assert_eq!(app.update(submit()).units(), 1);
        assert!(app.pending.contains(&key()));

        let msg = written(&app, WriteOp::Comment("## Hi".into()), |_| {});
        app.send(msg);
        assert!(app.drafts[&key()].text().trim().is_empty());
    }

    #[test]
    fn identity_sets_initials_and_failure_allows_a_retry() {
        let mut app = booted(&["a"]);
        assert_eq!(app.initials, "?");
        app.send(Message::Identified(Some("Ada Lovelace".into())));
        assert_eq!(app.initials, "AL");
        app.send(Message::Identified(None));
        assert_eq!(app.initials, "AL", "a failed lookup keeps the last name");
        assert!(!app.identity_asked);
    }

    #[test]
    fn discovery_asks_for_the_identity_once() {
        let mut app = App::from_parts(Workspace::default(), Snapshot::default(), None);
        app.discovering = true;
        app.epoch = 1;
        let repos = vec![info("a"), info("b")];
        let first = app.update(Message::Discovered {
            epoch: 1,
            repos: repos.clone(),
        });
        assert_eq!(first.units(), 3, "identity + one fingerprint per repo");
        app.discovering = true;
        app.epoch = 2;
        let second = app.update(Message::Discovered { epoch: 2, repos });
        assert_eq!(second.units(), 2);
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
