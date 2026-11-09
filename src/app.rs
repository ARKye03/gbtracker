//! Application state and update loop. `update` and `view` never block: every git, git-bug and fs call runs in a `Task`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gbtracker::cache::{self, Cache, Snapshot};
use gbtracker::detail::{DetailCache, LoadedDetail};
use gbtracker::done::{self, EpicCandidate, Report};
use gbtracker::epics::{self, EpicSummary};
use gbtracker::filter::{self, Filter, StateFilter, StatusFilter};
use gbtracker::gitbug::{
    self, GitBugError, Identity, Issue, IssueDetail, Kind, NewIssue, NewKind, Status, WriteOp,
};
use gbtracker::workspace::{self, RepoInfo, Root, RootKind, ThemePref, Workspace};
use gbtracker::writes::WriteQueue;
use iced::animation::{Animation, Easing};
use iced::keyboard::{self, Key, Modifiers};
use iced::theme::Mode as SystemMode;
use iced::time::Instant;
use iced::widget::operation::{self, RelativeOffset};
use iced::widget::text_editor;
use iced::{Element, Subscription, Task, Theme, clipboard, event, time, window};
use tokio::sync::Semaphore;

use crate::keymap::{self, Action, Context};
use crate::theme::{Mode, Tone};
use crate::{Section, ui};

const MAX_NOTIFICATIONS: usize = 100;
const MAX_CONCURRENT: usize = 4;
const TRACK_EVERY: Duration = Duration::from_secs(5);
const SPIN_EVERY: Duration = Duration::from_millis(250);
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);
pub const PAGE_SIZE: usize = 200;
pub const SEARCH_ID: &str = "issue-search";
pub const LIST_ID: &str = "issue-list";
pub const NEW_ISSUE_TITLE_ID: &str = "new-issue-title";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DonePhase {
    Loading,
    Failed,
    Choose,
    Closing,
    Closed,
}

/// The Roadmap Done sheet for one repo. Bugs close first; epics follow once every close has settled.
#[derive(Debug, Clone)]
pub struct DoneSheet {
    pub root: PathBuf,
    pub name: String,
    pub phase: DonePhase,
    pub error: Option<String>,
    pub report: Option<Report>,
    pub picked: HashSet<String>,
    /// Outcome per closed row, ticked in as each close completes.
    pub results: HashMap<String, Result<(), String>>,
    seq: u64,
    waiting: HashSet<String>,
    epic_plan: Vec<EpicCandidate>,
    epics_started: bool,
}

impl DoneSheet {
    fn loading(root: PathBuf, name: String, seq: u64) -> Self {
        DoneSheet {
            root,
            name,
            phase: DonePhase::Loading,
            error: None,
            report: None,
            picked: HashSet::new(),
            results: HashMap::new(),
            seq,
            waiting: HashSet::new(),
            epic_plan: Vec::new(),
            epics_started: false,
        }
    }

    /// A run in progress stays open, so its epics are still closed once the children settle.
    pub fn can_dismiss(&self) -> bool {
        self.phase != DonePhase::Closing
    }

    pub fn is_waiting(&self, id: &str) -> bool {
        self.waiting.contains(id)
    }

    /// Rows that will be closed with the current ticks.
    pub fn selected_count(&self) -> usize {
        self.report.as_ref().map_or(0, |r| {
            let (bugs, epics) = r.chosen(&self.picked);
            bugs.len() + epics.len()
        })
    }

    fn blocker(&self, epic: &EpicCandidate) -> Option<String> {
        epic.needs
            .iter()
            .find(|c| !matches!(self.results.get(&c.id), Some(Ok(()))))
            .map(|c| c.human_id.clone())
    }
}

/// The New issue dialog. Locked while a create is in flight so the submitted text and repo stay what was sent.
pub struct NewIssueForm {
    pub root: PathBuf,
    pub kind: NewKind,
    pub title: String,
    pub body: text_editor::Content,
    /// Full id of the parent epic.
    pub epic: Option<String>,
    pub submitting: bool,
    pub error: Option<String>,
}

impl NewIssueForm {
    fn new(root: PathBuf, kind: NewKind) -> Self {
        NewIssueForm {
            root,
            kind,
            title: String::new(),
            body: text_editor::Content::new(),
            epic: None,
            submitting: false,
            error: None,
        }
    }
}

/// The top-bar popover that is open; at most one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Popover {
    Repos,
    Overflow,
    Notifications,
    Identity,
}

#[derive(Debug, Clone)]
pub struct Notification {
    pub tone: Tone,
    pub title: String,
    pub detail: Option<String>,
    pub at: SystemTime,
}

pub struct App {
    pub system: SystemMode,
    theme_override: Option<Mode>,
    pub sidebar: Animation<bool>,
    /// Latest frame time; animations are projected at this instant so `view` stays pure.
    pub now: Instant,
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
    /// Keyboard-selected issue; follows clicks, and drives `Enter` and `x`.
    pub cursor: Option<(PathBuf, String)>,
    pub help: bool,
    detail_cache: DetailCache,
    /// Issues with a write in flight, by repo root and id; their actions stay disabled.
    pub pending: HashSet<(PathBuf, String)>,
    /// Last failed write per issue; cleared when the next one starts.
    pub write_errors: HashMap<(PathBuf, String), String>,
    /// Unsent comment text per issue, kept across pane switches.
    pub drafts: HashMap<(PathBuf, String), text_editor::Content>,
    writes: WriteQueue,
    pub done: Option<DoneSheet>,
    done_seq: u64,
    pub new_issue: Option<NewIssueForm>,
    pub initials: String,
    pub identity: Option<Identity>,
    identity_asked: bool,
    pub open_popover: Option<Popover>,
    /// Oldest first, capped at `MAX_NOTIFICATIONS`.
    pub notifications: Vec<Notification>,
    /// Notifications that arrived while the bell list was closed.
    pub unread: usize,
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
    SystemTheme(SystemMode),
    ToggleSidebar,
    Frame(Instant),
    Navigate(Section),
    Noop,
    KeyPressed {
        key: Key,
        modifiers: Modifiers,
        typing: bool,
    },
    FocusSearch,
    OpenCursor,
    ToggleHelp,
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
    OpenDone(PathBuf),
    DoneFound {
        seq: u64,
        result: Result<Box<Report>, String>,
    },
    ToggleDone(String),
    PickDone(bool),
    RunDone,
    DismissDone,
    OpenNewIssue(NewKind),
    DismissNewIssue,
    NewIssueRepo(PathBuf),
    NewIssueKind(NewKind),
    NewIssueTitle(String),
    NewIssueBody(text_editor::Action),
    NewIssueEpic(Option<String>),
    SubmitNewIssue,
    IssueCreated {
        root: PathBuf,
        result: Result<Box<IssueDetail>, String>,
    },
    Identified(Option<Identity>),
    TogglePopover(Popover),
    ClosePopover,
    ChooseRepo(Option<PathBuf>),
    SetTheme(ThemePref),
    ClearNotifications,
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
            app.set_notice(format!("{e} — changes will not be saved"));
        }
        if let Ok(theme) = std::env::var("GBTRACKER_THEME") {
            app.theme_override = Some(if theme == "dark" {
                Mode::Dark
            } else {
                Mode::Light
            });
        }
        let task = Task::batch([
            app.start_discovery(),
            iced::system::theme().map(Message::SystemTheme),
        ]);
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
            system: SystemMode::None,
            theme_override: None,
            sidebar: Animation::new(workspace.sidebar_collapsed)
                .quick()
                .easing(Easing::EaseInOut),
            now: Instant::now(),
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
            cursor: None,
            help: false,
            detail_cache: DetailCache::default(),
            pending: HashSet::new(),
            write_errors: HashMap::new(),
            drafts: HashMap::new(),
            writes: WriteQueue::default(),
            done: None,
            done_seq: 0,
            new_issue: None,
            initials: "?".to_owned(),
            identity: None,
            identity_asked: false,
            open_popover: None,
            notifications: Vec::new(),
            unread: 0,
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
        if let Some(at) = self.cursor_index() {
            while at >= self.shown {
                self.shown += PAGE_SIZE;
            }
        }
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
        if matches!(
            message,
            Message::Navigate(_)
                | Message::FocusSearch
                | Message::ToggleHelp
                | Message::OpenDone(_)
                | Message::Rescan
                | Message::PickFolder(_)
                | Message::ChooseRepo(_)
                | Message::OpenNewIssue(_)
        ) {
            self.open_popover = None;
        }
        match message {
            Message::ToggleTheme => {
                let next = self.workspace.theme.next();
                self.update(Message::SetTheme(next))
            }
            Message::SetTheme(pref) => {
                self.theme_override = None;
                self.workspace.theme = pref;
                self.persist_workspace()
            }
            Message::TogglePopover(popover) => {
                if self.open_popover == Some(popover) {
                    self.open_popover = None;
                } else {
                    self.open_popover = Some(popover);
                    if popover == Popover::Notifications {
                        self.unread = 0;
                    }
                }
                Task::none()
            }
            Message::ClosePopover => {
                self.open_popover = None;
                Task::none()
            }
            Message::ChooseRepo(root) => {
                self.pick_repo(root);
                self.section = Section::Issues;
                Task::none()
            }
            Message::ClearNotifications => {
                self.notifications.clear();
                self.unread = 0;
                Task::none()
            }
            Message::SystemTheme(mode) => {
                self.system = mode;
                Task::none()
            }
            Message::ToggleSidebar => {
                let collapsed = !self.workspace.sidebar_collapsed;
                self.workspace.sidebar_collapsed = collapsed;
                self.now = Instant::now();
                self.sidebar.go_mut(collapsed, self.now);
                self.persist_workspace()
            }
            Message::Frame(at) => {
                self.now = at;
                Task::none()
            }
            Message::Navigate(section) => {
                self.section = section;
                Task::none()
            }
            Message::Noop => Task::none(),
            Message::KeyPressed {
                key,
                modifiers,
                typing,
            } => {
                let ctx = Context {
                    typing,
                    section: self.section,
                    modal_open: self.help || self.done.is_some() || self.new_issue.is_some(),
                };
                match keymap::resolve(&key, modifiers, ctx) {
                    Some(action) => self.perform(action),
                    None => Task::none(),
                }
            }
            Message::FocusSearch => {
                self.section = Section::Issues;
                operation::focus(SEARCH_ID)
            }
            Message::OpenCursor => {
                self.filter.set_text(&self.search);
                self.apply_filter();
                self.cursor = None;
                self.open_cursor()
            }
            Message::ToggleHelp => {
                self.help = !self.help;
                Task::none()
            }
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
                self.pick_repo(root);
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
                self.cursor = Some((root.clone(), id.clone()));
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
                if self.help {
                    self.help = false;
                    return Task::none();
                }
                if self.open_popover.take().is_some() {
                    return Task::none();
                }
                if let Some(form) = &self.new_issue {
                    if !form.submitting {
                        self.new_issue = None;
                    }
                    return Task::none();
                }
                match &self.done {
                    Some(sheet) if sheet.can_dismiss() => self.done = None,
                    Some(_) => {}
                    None => self.detail = None,
                }
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
            Message::OpenDone(root) => self.open_done(root),
            Message::DoneFound { seq, result } => {
                self.on_done_found(seq, result);
                Task::none()
            }
            Message::ToggleDone(id) => {
                if let Some(sheet) = self.done.as_mut().filter(|s| s.phase == DonePhase::Choose)
                    && !sheet.picked.remove(&id)
                {
                    sheet.picked.insert(id);
                }
                Task::none()
            }
            Message::PickDone(all) => {
                if let Some(sheet) = self.done.as_mut().filter(|s| s.phase == DonePhase::Choose) {
                    sheet.picked = match (&sheet.report, all) {
                        (Some(report), true) => report.all_ids(),
                        _ => HashSet::new(),
                    };
                }
                Task::none()
            }
            Message::RunDone => self.run_done(),
            Message::DismissDone => {
                if self.done.as_ref().is_some_and(DoneSheet::can_dismiss) {
                    self.done = None;
                }
                Task::none()
            }
            Message::OpenNewIssue(kind) => self.open_new_issue(kind),
            Message::DismissNewIssue => {
                if self.new_issue.as_ref().is_some_and(|f| !f.submitting) {
                    self.new_issue = None;
                }
                Task::none()
            }
            Message::NewIssueRepo(root) => {
                if let Some(form) = self.editable_form()
                    && form.root != root
                {
                    form.root = root;
                    form.epic = None;
                }
                Task::none()
            }
            Message::NewIssueKind(kind) => {
                if let Some(form) = self.editable_form() {
                    form.kind = kind;
                }
                Task::none()
            }
            Message::NewIssueTitle(title) => {
                if let Some(form) = self.editable_form() {
                    form.title = title;
                }
                Task::none()
            }
            Message::NewIssueBody(action) => {
                if let Some(form) = self.editable_form() {
                    form.body.perform(action);
                }
                Task::none()
            }
            Message::NewIssueEpic(epic) => {
                if let Some(form) = self.editable_form() {
                    form.epic = epic;
                }
                Task::none()
            }
            Message::SubmitNewIssue => self.submit_new_issue(),
            Message::IssueCreated { root, result } => self.on_created(root, result),
            Message::Identified(identity) => {
                match identity {
                    Some(identity) => {
                        self.initials = gitbug::initials(&identity.name);
                        self.identity = Some(identity);
                    }
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
                    self.set_notice(format!("save failed: {e}"));
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

    fn pick_repo(&mut self, root: Option<PathBuf>) {
        self.filter.repos.clear();
        self.filter.repos.extend(root);
        self.apply_filter();
    }

    fn notify(&mut self, tone: Tone, title: impl Into<String>, detail: Option<String>) {
        self.notifications.push(Notification {
            tone,
            title: title.into(),
            detail,
            at: SystemTime::now(),
        });
        if self.notifications.len() > MAX_NOTIFICATIONS {
            self.notifications.remove(0);
        }
        if self.open_popover != Some(Popover::Notifications) {
            self.unread += 1;
        }
        self.unread = self.unread.min(self.notifications.len());
    }

    fn set_notice(&mut self, notice: String) {
        self.notify(Tone::Warning, notice.clone(), None);
        self.notice = Some(notice);
    }

    fn issue_ref(&self, root: &Path, id: &str) -> (String, Option<String>) {
        let found = self
            .repos
            .iter()
            .find(|e| e.info.root == root)
            .and_then(|e| e.issues.iter().find(|i| i.id == id));
        match found {
            Some(issue) => (issue.human_id.clone(), Some(issue.title.clone())),
            None => (id.chars().take(7).collect(), None),
        }
    }

    fn notify_write(&mut self, root: &Path, id: &str, op: &WriteOp, outcome: &Result<(), String>) {
        let (hid, title) = self.issue_ref(root, id);
        match outcome {
            Ok(()) => {
                let verb = match op {
                    WriteOp::Close => "Closed",
                    WriteOp::Reopen => "Reopened",
                    WriteOp::State(_) => "Changed state of",
                    WriteOp::Comment(_) => "Commented on",
                };
                self.notify(Tone::Success, format!("{verb} {hid}"), title);
            }
            Err(msg) => {
                let verb = match op {
                    WriteOp::Close => "close",
                    WriteOp::Reopen => "reopen",
                    WriteOp::State(_) => "change the state of",
                    WriteOp::Comment(_) => "comment on",
                };
                self.notify(
                    Tone::Danger,
                    format!("Could not {verb} {hid}"),
                    Some(msg.clone()),
                );
            }
        }
    }

    fn perform(&mut self, action: Action) -> Task<Message> {
        match action {
            Action::Goto(section) => self.update(Message::Navigate(section)),
            Action::ToggleSidebar => self.update(Message::ToggleSidebar),
            Action::FocusSearch => self.update(Message::FocusSearch),
            Action::CycleTheme => self.update(Message::ToggleTheme),
            Action::Rescan => self.update(Message::Rescan),
            Action::NewIssue => self.update(Message::OpenNewIssue(NewKind::Bug)),
            Action::Next => self.move_cursor(1),
            Action::Previous => self.move_cursor(-1),
            Action::Open => self.open_cursor(),
            Action::ToggleStatus => self.toggle_cursor_status(),
            Action::Help => self.update(Message::ToggleHelp),
            Action::Escape => self.update(Message::CloseDetail),
        }
    }

    fn cursor_index(&self) -> Option<usize> {
        let (root, id) = self.cursor.as_ref()?;
        self.rows.iter().position(|r| {
            let entry = &self.repos[r.repo];
            entry.info.root == *root && entry.issues[r.issue].id == *id
        })
    }

    fn row_target(&self, index: usize) -> (PathBuf, Issue) {
        let row = &self.rows[index];
        let entry = &self.repos[row.repo];
        (entry.info.root.clone(), entry.issues[row.issue].clone())
    }

    /// Moves the keyboard cursor; an open detail pane follows it.
    fn move_cursor(&mut self, step: isize) -> Task<Message> {
        if self.rows.is_empty() {
            return Task::none();
        }
        let last = self.rows.len() - 1;
        let next = match self.cursor_index() {
            Some(at) => at.saturating_add_signed(step).min(last),
            None => 0,
        };
        let scroll = self.scroll_to(next);
        let (root, issue) = self.row_target(next);
        self.cursor = Some((root.clone(), issue.id.clone()));
        if self.detail.is_some() {
            Task::batch([scroll, self.open_detail(root, issue)])
        } else {
            scroll
        }
    }

    fn scroll_to(&mut self, index: usize) -> Task<Message> {
        while index >= self.shown {
            self.shown += PAGE_SIZE;
        }
        // Rows are uniform, so a proportional offset keeps the row fully in view.
        let visible = self.rows.len().min(self.shown);
        let y = if visible > 1 {
            index as f32 / (visible - 1) as f32
        } else {
            0.0
        };
        operation::snap_to(LIST_ID, RelativeOffset { x: 0.0, y })
    }

    fn editable_form(&mut self) -> Option<&mut NewIssueForm> {
        self.new_issue.as_mut().filter(|f| !f.submitting)
    }

    /// Defaults to the repo the list is filtered to, else the first one git-bug can write to.
    fn open_new_issue(&mut self, kind: NewKind) -> Task<Message> {
        if self.new_issue.is_some() {
            return Task::none();
        }
        let usable = |e: &&RepoEntry| e.info.has_git_bug;
        let filtered = self
            .filter
            .repos
            .iter()
            .find(|r| self.repos.iter().filter(usable).any(|e| e.info.root == **r))
            .cloned();
        let root = filtered.or_else(|| self.repos.iter().find(usable).map(|e| e.info.root.clone()));
        let Some(root) = root else {
            self.notify(
                Tone::Warning,
                "No repository to file an issue in",
                Some("Add a folder that holds a git-bug repository first.".to_owned()),
            );
            return Task::none();
        };
        self.new_issue = Some(NewIssueForm::new(root, kind));
        operation::focus(NEW_ISSUE_TITLE_ID)
    }

    /// Open epics of a repo, the candidates for a new issue's parent.
    pub fn open_epics(&self, root: &Path) -> Vec<&Issue> {
        self.repos
            .iter()
            .find(|e| e.info.root == root)
            .map(|e| {
                e.issues
                    .iter()
                    .filter(|i| i.kind == Some(Kind::Epic) && i.status == Status::Open)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Queues behind earlier writes to the repo; the form stays up, locked, until the result arrives.
    fn submit_new_issue(&mut self) -> Task<Message> {
        let Some(form) = self.new_issue.as_mut().filter(|f| !f.submitting) else {
            return Task::none();
        };
        let new = NewIssue {
            kind: form.kind,
            title: form.title.trim().to_owned(),
            body: form.body.text().trim_end().to_owned(),
            epic: form.epic.clone(),
        };
        if new.title.is_empty() {
            form.error = Some(GitBugError::EmptyTitle.to_string());
            return Task::none();
        }
        form.submitting = true;
        form.error = None;
        let root = form.root.clone();
        let ticket = self.writes.enter(&root);
        Task::perform(
            async move {
                ticket.turn().await;
                let result = gitbug::create(&root, &new)
                    .await
                    .map(Box::new)
                    .map_err(|e| e.to_string());
                (root, result)
            },
            |(root, result)| Message::IssueCreated { root, result },
        )
    }

    /// The issue is added to the list at once; the stale fingerprint makes the follow-up check reload the repo for real.
    fn on_created(
        &mut self,
        root: PathBuf,
        result: Result<Box<IssueDetail>, String>,
    ) -> Task<Message> {
        let detail = match result {
            Ok(detail) => *detail,
            Err(msg) => {
                if let Some(form) = self.new_issue.as_mut().filter(|f| f.root == root) {
                    form.submitting = false;
                    form.error = Some(msg);
                }
                return Task::none();
            }
        };
        if self.new_issue.as_ref().is_some_and(|f| f.root == root) {
            self.new_issue = None;
        }
        let issue = detail.issue.clone();
        let id = issue.id.clone();
        self.notify(
            Tone::Success,
            format!("Created {}", issue.human_id),
            Some(issue.title.clone()),
        );
        self.detail_cache.insert(
            root.clone(),
            id.clone(),
            issue.edited.clone(),
            Rc::new(LoadedDetail::new(detail)),
        );
        let Some(persist) = self.insert_issue(&root, issue) else {
            return Task::none();
        };
        self.section = Section::Issues;
        let key = (root.clone(), id.clone());
        if !self.rows.iter().any(|r| {
            let entry = &self.repos[r.repo];
            entry.info.root == key.0 && entry.issues[r.issue].id == key.1
        }) {
            self.filter = Filter::default();
            self.search.clear();
            self.search_seq += 1;
            self.roadmap = false;
            self.apply_filter();
        }
        let select = self.update(Message::SelectIssue { root, id });
        let scroll = match self.cursor_index() {
            Some(at) => self.scroll_to(at),
            None => Task::none(),
        };
        Task::batch([persist, select, scroll, self.check_tracked()])
    }

    /// Adds or replaces one issue, keeping the layer order a fresh load would give. `None` when the repo is gone.
    fn insert_issue(&mut self, root: &Path, issue: Issue) -> Option<Task<Message>> {
        let entry = self.repos.iter_mut().find(|e| e.info.root == root)?;
        let mut issues = std::mem::take(&mut entry.issues);
        match issues.iter().position(|i| i.id == issue.id) {
            Some(pos) => issues[pos] = issue,
            None => issues.push(issue),
        }
        issues.sort_by_key(|i| i.layer.unwrap_or(u32::MAX));
        entry.set_issues(issues);
        if matches!(entry.state, LoadState::Ready(_)) {
            entry.state = LoadState::Ready(entry.issues.len());
        }
        let persist = entry
            .fingerprint
            .clone()
            .map(|fp| (entry.info.clone(), fp, entry.issues.clone()));
        self.refilter();
        Some(match persist {
            Some((info, fp, issues)) => self.persist_issues(info, fp, issues),
            None => Task::none(),
        })
    }

    fn open_cursor(&mut self) -> Task<Message> {
        if self.rows.is_empty() {
            return Task::none();
        }
        let at = self.cursor_index().unwrap_or(0);
        let (root, issue) = self.row_target(at);
        self.cursor = Some((root.clone(), issue.id.clone()));
        self.open_detail(root, issue)
    }

    fn toggle_cursor_status(&mut self) -> Task<Message> {
        let Some(at) = self.cursor_index() else {
            return Task::none();
        };
        let (root, issue) = self.row_target(at);
        let op = match issue.status {
            Status::Closed => WriteOp::Reopen,
            _ => WriteOp::Close,
        };
        self.start_write(root, issue.id, op)
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

    fn on_written(
        &mut self,
        root: PathBuf,
        id: String,
        op: WriteOp,
        result: Result<Box<IssueDetail>, String>,
    ) -> Task<Message> {
        let outcome = result.as_ref().map(|_| ()).map_err(Clone::clone);
        let closed = matches!(op, WriteOp::Close);
        // The Roadmap Done summary stands in for the per-issue closes it ran.
        let from_done = closed
            && self
                .done
                .as_ref()
                .is_some_and(|s| s.root == root && s.is_waiting(&id));
        if !from_done {
            self.notify_write(&root, &id, &op, &outcome);
        }
        let applied = self.apply_written(root.clone(), id.clone(), op, result);
        if closed {
            Task::batch([applied, self.done_progress(&root, &id, outcome)])
        } else {
            applied
        }
    }

    /// A failed write keeps the local copy (a partly applied one is picked up by the next fingerprint check); a successful one replaces the issue everywhere it is held.
    fn apply_written(
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

    /// Opens at once in a loading state; candidates come from the issues already loaded.
    fn open_done(&mut self, root: PathBuf) -> Task<Message> {
        let Some(entry) = self
            .repos
            .iter()
            .find(|e| e.info.root == root && e.fingerprint.is_some())
        else {
            return Task::none();
        };
        self.done_seq += 1;
        let seq = self.done_seq;
        let (name, issues) = (entry.info.name.clone(), entry.issues.clone());
        self.done = Some(DoneSheet::loading(root.clone(), name, seq));
        Task::perform(
            async move {
                done::find(&root, &issues, None)
                    .await
                    .map(Box::new)
                    .map_err(|e| e.to_string())
            },
            move |result| Message::DoneFound { seq, result },
        )
    }

    fn on_done_found(&mut self, seq: u64, result: Result<Box<Report>, String>) {
        let Some(sheet) = self
            .done
            .as_mut()
            .filter(|s| s.seq == seq && s.phase == DonePhase::Loading)
        else {
            return;
        };
        match result {
            Ok(report) => {
                sheet.picked = report.all_ids();
                sheet.report = Some(*report);
                sheet.phase = DonePhase::Choose;
            }
            Err(msg) => {
                sheet.error = Some(msg.clone());
                sheet.phase = DonePhase::Failed;
                let name = sheet.name.clone();
                self.notify(
                    Tone::Danger,
                    format!("Roadmap Done failed for {name}"),
                    Some(msg),
                );
            }
        }
    }

    fn complete_done(&mut self) {
        let Some(sheet) = self.done.as_mut() else {
            return;
        };
        sheet.phase = DonePhase::Closed;
        let ok = sheet.results.values().filter(|r| r.is_ok()).count();
        let failed = sheet.results.len() - ok;
        let title = format!("Roadmap Done: {}", sheet.name);
        let (tone, detail) = if failed == 0 {
            (Tone::Success, format!("{ok} closed"))
        } else {
            (Tone::Danger, format!("{ok} closed, {failed} failed"))
        };
        self.notify(tone, title, Some(detail));
    }

    fn run_done(&mut self) -> Task<Message> {
        let Some(sheet) = self.done.as_mut().filter(|s| s.phase == DonePhase::Choose) else {
            return Task::none();
        };
        let Some(report) = &sheet.report else {
            return Task::none();
        };
        let (bugs, epics) = report.chosen(&sheet.picked);
        if bugs.is_empty() && epics.is_empty() {
            return Task::none();
        }
        let bug_ids: Vec<String> = bugs.iter().map(|b| b.id.clone()).collect();
        let epic_plan: Vec<EpicCandidate> = epics.into_iter().cloned().collect();
        let root = sheet.root.clone();
        sheet.phase = DonePhase::Closing;
        sheet.epic_plan = epic_plan;
        let mut tasks: Vec<Task<Message>> = bug_ids
            .iter()
            .map(|id| self.done_close(&root, id))
            .collect();
        tasks.push(self.done_settle());
        Task::batch(tasks)
    }

    /// An issue with another write in flight fails its row at once instead of waiting for a result that never comes.
    fn done_close(&mut self, root: &Path, id: &str) -> Task<Message> {
        let busy = self.is_pending(root, id);
        let Some(sheet) = self.done.as_mut() else {
            return Task::none();
        };
        if busy {
            sheet.results.insert(
                id.to_owned(),
                Err("another write on this issue is in flight".to_owned()),
            );
            return Task::none();
        }
        sheet.waiting.insert(id.to_owned());
        self.start_write(root.to_owned(), id.to_owned(), WriteOp::Close)
    }

    fn done_progress(
        &mut self,
        root: &Path,
        id: &str,
        outcome: Result<(), String>,
    ) -> Task<Message> {
        let Some(sheet) = self.done.as_mut().filter(|s| s.root == root) else {
            return Task::none();
        };
        if !sheet.waiting.remove(id) {
            return Task::none();
        }
        sheet.results.insert(id.to_owned(), outcome);
        self.done_settle()
    }

    /// Once every bug close has finished, closes the epics whose children all closed; the rest fail without a write.
    fn done_settle(&mut self) -> Task<Message> {
        let Some(sheet) = self.done.as_mut() else {
            return Task::none();
        };
        if sheet.phase != DonePhase::Closing || !sheet.waiting.is_empty() {
            return Task::none();
        }
        if sheet.epics_started {
            self.complete_done();
            return Task::none();
        }
        sheet.epics_started = true;
        let root = sheet.root.clone();
        let mut tasks = Vec::new();
        for epic in std::mem::take(&mut sheet.epic_plan) {
            let blocker = self.done.as_ref().and_then(|s| s.blocker(&epic));
            match blocker {
                Some(hid) => {
                    if let Some(s) = self.done.as_mut() {
                        s.results
                            .insert(epic.id.clone(), Err(format!("child {hid} did not close")));
                    }
                }
                None => tasks.push(self.done_close(&root, &epic.id)),
            }
        }
        if self.done.as_ref().is_some_and(|s| s.waiting.is_empty()) {
            self.complete_done();
        }
        Task::batch(tasks)
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
        if self
            .done
            .as_ref()
            .is_some_and(|d| !known.contains(&&d.root))
        {
            self.done = None;
        }
        if self
            .new_issue
            .as_ref()
            .is_some_and(|f| !known.contains(&&f.root))
        {
            self.new_issue = None;
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
                    async move { gitbug::current_identity(&root).await.ok() },
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
                let name = entry.info.name.clone();
                entry.state = LoadState::Error(msg.clone());
                self.notify(Tone::Danger, format!("{name} failed to load"), Some(msg));
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
                let name = entry.info.name.clone();
                entry.state = LoadState::Error(msg.clone());
                self.notify(Tone::Danger, format!("{name} failed to load"), Some(msg));
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

    pub fn mode(&self) -> Mode {
        if let Some(mode) = self.theme_override {
            return mode;
        }
        match self.workspace.theme {
            ThemePref::Light => Mode::Light,
            ThemePref::Dark => Mode::Dark,
            ThemePref::System => match self.system {
                SystemMode::Dark => Mode::Dark,
                _ => Mode::Light,
            },
        }
    }

    pub fn theme(&self) -> Theme {
        self.mode().theme()
    }

    pub fn needs_frames(&self) -> bool {
        self.sidebar.is_animating(self.now)
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            iced::system::theme_changes().map(Message::SystemTheme),
            time::every(TRACK_EVERY).map(|_| Message::Tick),
            event::listen_with(|event, status, _| match event {
                event::Event::Window(window::Event::Focused) => Some(Message::Refocused),
                event::Event::Keyboard(keyboard::Event::KeyPressed {
                    key,
                    modified_key,
                    modifiers,
                    ..
                }) => Some(Message::KeyPressed {
                    key: if modifiers.command() {
                        key
                    } else {
                        modified_key
                    },
                    modifiers,
                    typing: status == event::Status::Captured,
                }),
                _ => None,
            }),
        ];
        let sheet_busy = self
            .done
            .as_ref()
            .is_some_and(|d| matches!(d.phase, DonePhase::Loading | DonePhase::Closing));
        let creating = self.new_issue.as_ref().is_some_and(|f| f.submitting);
        if self.is_busy() || !self.pending.is_empty() || sheet_busy || creating {
            subs.push(time::every(SPIN_EVERY).map(|_| Message::Spin));
        }
        if self.needs_frames() {
            subs.push(window::frames().map(Message::Frame));
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
    use iced::keyboard::key::Named;

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
    fn theme_toggle_cycles_system_light_dark() {
        let mut app = App::from_parts(Workspace::default(), Snapshot::default(), None);
        app.config_locked = true;
        assert_eq!(app.workspace.theme, ThemePref::System);
        app.send(Message::ToggleTheme);
        assert_eq!(app.workspace.theme, ThemePref::Light);
        app.send(Message::ToggleTheme);
        assert_eq!(app.workspace.theme, ThemePref::Dark);
        app.send(Message::ToggleTheme);
        assert_eq!(app.workspace.theme, ThemePref::System);
    }

    #[test]
    fn workspace_dark_selects_initial_mode() {
        let ws = Workspace {
            theme: ThemePref::Dark,
            ..Workspace::default()
        };
        let app = App::from_parts(ws, Snapshot::default(), None);
        assert_eq!(app.mode(), Mode::Dark);
    }

    #[test]
    fn system_pref_follows_os_changes() {
        let mut app = App::from_parts(Workspace::default(), Snapshot::default(), None);
        app.config_locked = true;
        assert_eq!(app.mode(), Mode::Light);
        app.send(Message::SystemTheme(SystemMode::Dark));
        assert_eq!(app.mode(), Mode::Dark);
        app.send(Message::SystemTheme(SystemMode::Light));
        assert_eq!(app.mode(), Mode::Light);
        app.send(Message::SystemTheme(SystemMode::Dark));
        app.send(Message::ToggleTheme);
        assert_eq!(app.mode(), Mode::Light);
    }

    #[test]
    fn sidebar_toggle_animates_then_stops_asking_for_frames() {
        let mut app = App::from_parts(Workspace::default(), Snapshot::default(), None);
        app.config_locked = true;
        assert!(!app.needs_frames());
        app.send(Message::ToggleSidebar);
        assert!(app.workspace.sidebar_collapsed);
        assert!(app.needs_frames());
        app.send(Message::Frame(Instant::now() + Duration::from_secs(1)));
        assert!(!app.needs_frames());
        assert_eq!(app.sidebar.interpolate(276.0, 64.0, app.now), 64.0);
    }

    #[test]
    fn collapsed_workspace_boots_collapsed_without_animating() {
        let ws = Workspace {
            sidebar_collapsed: true,
            ..Workspace::default()
        };
        let app = App::from_parts(ws, Snapshot::default(), None);
        assert!(!app.needs_frames());
        assert_eq!(app.sidebar.interpolate(276.0, 64.0, app.now), 64.0);
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
    fn issue_list_fills_width_despite_lazy_rows() {
        use iced::Length;

        let app = loaded(&[("one", Status::Open), ("two", Status::Open)]);
        assert_eq!(app.rows.len(), 2);
        let list = ui::screen::issue_list(&app);
        assert_eq!(list.as_widget().size().width, Length::Fill);
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
        app.send(Message::Identified(Some(ada())));
        assert_eq!(app.initials, "AL");
        assert_eq!(app.identity, Some(ada()));
        app.send(Message::Identified(None));
        assert_eq!(app.initials, "AL", "a failed lookup keeps the last name");
        assert_eq!(app.identity, Some(ada()));
        assert!(!app.identity_asked);
    }

    fn ada() -> Identity {
        Identity {
            name: "Ada Lovelace".into(),
            email: "ada@example.com".into(),
        }
    }

    fn fail_close(app: &mut App) {
        let _ = app.update(write(WriteOp::Close));
        app.send(Message::WriteDone {
            root: info("a").root,
            id: "a".repeat(64),
            op: WriteOp::Close,
            result: Err("boom".into()),
        });
    }

    #[test]
    fn a_failed_write_pushes_a_notification_and_counts_it_unread() {
        let mut app = loaded(&[("one", Status::Open)]);
        assert!(app.notifications.is_empty());
        assert_eq!(app.unread, 0);
        fail_close(&mut app);
        assert_eq!(app.unread, 1);
        let note = app.notifications.last().unwrap();
        assert_eq!(note.tone, Tone::Danger);
        assert!(note.title.contains("close"));
        assert_eq!(note.detail.as_deref(), Some("boom"));
    }

    #[test]
    fn a_successful_write_notifies_with_the_issue_title() {
        let mut app = loaded(&[("one", Status::Open)]);
        let _ = app.update(write(WriteOp::Close));
        let msg = written(&app, WriteOp::Close, |i| i.status = Status::Closed);
        app.send(msg);
        let note = app.notifications.last().unwrap();
        assert_eq!(note.tone, Tone::Success);
        assert!(note.title.starts_with("Closed"));
        assert_eq!(note.detail.as_deref(), Some("one"));
        assert_eq!(app.unread, 1);
    }

    #[test]
    fn opening_the_bell_resets_unread_and_arrivals_while_open_stay_read() {
        let mut app = loaded(&[("one", Status::Open)]);
        fail_close(&mut app);
        fail_close(&mut app);
        assert_eq!(app.unread, 2);

        app.send(Message::TogglePopover(Popover::Notifications));
        assert_eq!(app.open_popover, Some(Popover::Notifications));
        assert_eq!(app.unread, 0);
        fail_close(&mut app);
        assert_eq!(app.unread, 0);
        assert_eq!(app.notifications.len(), 3);

        app.send(Message::TogglePopover(Popover::Notifications));
        assert_eq!(app.open_popover, None);
        fail_close(&mut app);
        assert_eq!(app.unread, 1);

        app.send(Message::ClearNotifications);
        assert!(app.notifications.is_empty());
        assert_eq!(app.unread, 0);
    }

    #[test]
    fn notifications_are_capped_and_unread_never_exceeds_them() {
        let mut app = App::from_parts(Workspace::default(), Snapshot::default(), None);
        for n in 0..MAX_NOTIFICATIONS + 5 {
            app.notify(Tone::Success, format!("n{n}"), None);
        }
        assert_eq!(app.notifications.len(), MAX_NOTIFICATIONS);
        assert_eq!(app.notifications[0].title, "n5");
        assert_eq!(app.unread, MAX_NOTIFICATIONS);
    }

    #[test]
    fn failed_repo_loads_and_config_notices_are_announced() {
        let mut app = booted(&["a", "b"]);
        app.send(Message::Fingerprinted {
            epoch: 1,
            root: info("a").root,
            result: Err("gone".into()),
        });
        app.send(Message::Fingerprinted {
            epoch: 1,
            root: info("b").root,
            result: Ok("new".into()),
        });
        app.send(Message::Loaded {
            epoch: 1,
            root: info("b").root,
            fingerprint: "new".into(),
            result: Err(LoadFailure::Failed("corrupt".into())),
        });
        let titles: Vec<_> = app.notifications.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, ["a failed to load", "b failed to load"]);

        app.send(Message::Loaded {
            epoch: 1,
            root: info("b").root,
            fingerprint: "new".into(),
            result: Err(LoadFailure::Locked),
        });
        assert_eq!(
            app.notifications.len(),
            2,
            "a lock is retried, not reported"
        );

        app.send(Message::Persisted(Err("disk full".into())));
        assert_eq!(app.notice.as_deref(), Some("save failed: disk full"));
        assert_eq!(app.notifications.last().unwrap().tone, Tone::Warning);
    }

    #[test]
    fn roadmap_done_reports_once_instead_of_per_close() {
        let mut app = sheet_open();
        let _ = app.update(Message::RunDone);
        app.send(closed('b', Ok(())));
        app.send(closed('c', Err("locked")));
        let notes: Vec<_> = app
            .notifications
            .iter()
            .map(|n| (n.tone, n.title.as_str(), n.detail.as_deref()))
            .collect();
        assert_eq!(
            notes,
            [(Tone::Danger, "Roadmap Done: a", Some("1 closed, 2 failed"))]
        );
    }

    #[test]
    fn roadmap_done_that_cannot_read_the_base_is_announced() {
        let mut app = loaded(&[("one", Status::Open)]);
        let _ = app.update(Message::OpenDone(info("a").root));
        let seq = sheet(&app).seq;
        app.send(Message::DoneFound {
            seq,
            result: Err("no base branch".into()),
        });
        let note = app.notifications.last().unwrap();
        assert_eq!(note.tone, Tone::Danger);
        assert_eq!(note.detail.as_deref(), Some("no base branch"));
    }

    #[test]
    fn choosing_a_repo_filters_the_issues_page_and_closes_the_switcher() {
        let mut app = booted(&["a", "b"]);
        app.send(Message::TogglePopover(Popover::Repos));
        assert_eq!(app.section, Section::Repositories);

        app.send(Message::ChooseRepo(Some(info("b").root)));
        assert_eq!(app.section, Section::Issues);
        assert_eq!(app.filter.repos.len(), 1);
        assert!(app.filter.repos.contains(&info("b").root));
        assert_eq!(app.open_popover, None);

        app.section = Section::Overview;
        app.send(Message::ChooseRepo(None));
        assert_eq!(app.section, Section::Issues);
        assert!(app.filter.repos.is_empty());
    }

    #[test]
    fn every_popover_builds_a_view() {
        let mut app = loaded(&[("one", Status::Open)]);
        app.identity = Some(ada());
        fail_close(&mut app);
        for popover in [
            Popover::Repos,
            Popover::Overflow,
            Popover::Notifications,
            Popover::Identity,
        ] {
            app.open_popover = Some(popover);
            let _ = app.view();
        }
    }

    #[test]
    fn only_one_popover_is_open_and_escape_closes_it_before_the_pane() {
        let mut app = numbered(2);
        enter(&mut app);
        app.send(Message::TogglePopover(Popover::Overflow));
        app.send(Message::TogglePopover(Popover::Identity));
        assert_eq!(app.open_popover, Some(Popover::Identity));
        escape(&mut app, false);
        assert_eq!(app.open_popover, None);
        assert!(app.detail.is_some());
        escape(&mut app, false);
        assert!(app.detail.is_none());
    }

    #[test]
    fn menu_actions_close_the_popover() {
        let mut app = booted(&["a"]);
        app.config_locked = true;
        for message in [
            Message::Rescan,
            Message::ToggleHelp,
            Message::Navigate(Section::Epics),
            Message::OpenDone(info("a").root),
        ] {
            app.send(Message::TogglePopover(Popover::Overflow));
            assert_eq!(app.open_popover, Some(Popover::Overflow));
            app.send(message);
            assert_eq!(app.open_popover, None);
        }
        app.send(Message::TogglePopover(Popover::Identity));
        app.send(Message::SetTheme(ThemePref::Dark));
        assert_eq!(app.workspace.theme, ThemePref::Dark);
        assert_eq!(
            app.open_popover,
            Some(Popover::Identity),
            "stays for more picks"
        );
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

    fn done_id(c: char) -> String {
        c.to_string().repeat(64)
    }

    fn candidate(c: char) -> done::Candidate {
        done::Candidate {
            id: done_id(c),
            human_id: c.to_string().repeat(7),
            title: format!("bug {c}"),
            kind: Some(Kind::Task),
            evidence: vec![],
        }
    }

    fn done_report() -> Report {
        let needs = ['b', 'c']
            .iter()
            .map(|&c| done::NeededChild {
                id: done_id(c),
                human_id: c.to_string().repeat(7),
            })
            .collect();
        Report {
            base: "main".into(),
            refs: vec!["main".into()],
            open_count: 3,
            bugs: vec![candidate('b'), candidate('c')],
            epics: vec![EpicCandidate {
                id: done_id('e'),
                human_id: "eeeeeee".into(),
                title: "epic".into(),
                children: 2,
                needs,
            }],
        }
    }

    fn sheet_open() -> App {
        let mut app = loaded(&[("one", Status::Open)]);
        assert_eq!(app.update(Message::OpenDone(info("a").root)).units(), 1);
        let seq = app.done.as_ref().unwrap().seq;
        assert_eq!(app.done.as_ref().unwrap().phase, DonePhase::Loading);
        app.send(Message::DoneFound {
            seq,
            result: Ok(Box::new(done_report())),
        });
        app
    }

    fn closed(c: char, result: Result<(), &str>) -> Message {
        let mut issue = Issue {
            id: done_id(c),
            ..issue(Status::Closed)
        };
        issue.human_id = c.to_string().repeat(7);
        Message::WriteDone {
            root: info("a").root,
            id: issue.id.clone(),
            op: WriteOp::Close,
            result: result.map_err(str::to_owned).map(|()| {
                Box::new(IssueDetail {
                    issue,
                    body: "body".into(),
                    comments: vec![],
                })
            }),
        }
    }

    fn sheet(app: &App) -> &DoneSheet {
        app.done.as_ref().unwrap()
    }

    #[test]
    fn the_sheet_opens_loading_then_ticks_every_candidate() {
        let app = sheet_open();
        assert_eq!(sheet(&app).phase, DonePhase::Choose);
        assert_eq!(sheet(&app).picked.len(), 3);
        assert_eq!(sheet(&app).selected_count(), 3);
    }

    #[test]
    fn the_sheet_needs_loaded_issues_and_ignores_stale_results() {
        let mut app = booted(&["a"]);
        app.repos[0].fingerprint = None;
        assert_eq!(app.update(Message::OpenDone(info("a").root)).units(), 0);
        assert!(app.done.is_none());

        let mut app = loaded(&[("one", Status::Open)]);
        let _ = app.update(Message::OpenDone(info("a").root));
        let seq = sheet(&app).seq;
        app.send(Message::DoneFound {
            seq: seq + 1,
            result: Ok(Box::new(done_report())),
        });
        assert_eq!(sheet(&app).phase, DonePhase::Loading);
        app.send(Message::DoneFound {
            seq,
            result: Err("no base branch".into()),
        });
        assert_eq!(sheet(&app).phase, DonePhase::Failed);
        assert!(sheet(&app).error.as_ref().unwrap().contains("base"));
    }

    #[test]
    fn bugs_close_first_and_the_epic_follows_its_children() {
        let mut app = sheet_open();
        assert_eq!(app.update(Message::RunDone).units(), 2);
        assert_eq!(sheet(&app).phase, DonePhase::Closing);
        assert!(sheet(&app).is_waiting(&done_id('b')));
        assert!(!sheet(&app).is_waiting(&done_id('e')));

        assert_eq!(app.update(closed('b', Ok(()))).units(), 0);
        assert_eq!(sheet(&app).results[&done_id('b')], Ok(()));
        assert_eq!(sheet(&app).phase, DonePhase::Closing);

        assert_eq!(app.update(closed('c', Ok(()))).units(), 1);
        assert!(sheet(&app).is_waiting(&done_id('e')));
        assert_eq!(sheet(&app).phase, DonePhase::Closing);

        app.send(closed('e', Ok(())));
        assert_eq!(sheet(&app).phase, DonePhase::Closed);
        assert!(sheet(&app).results.values().all(Result::is_ok));
        assert!(app.pending.is_empty());
    }

    #[test]
    fn a_failed_child_keeps_the_epic_open_without_writing() {
        let mut app = sheet_open();
        let _ = app.update(Message::RunDone);
        app.send(closed('b', Ok(())));
        assert_eq!(app.update(closed('c', Err("locked"))).units(), 0);

        let sheet = sheet(&app);
        assert_eq!(sheet.phase, DonePhase::Closed);
        assert!(sheet.results[&done_id('c')].is_err());
        assert_eq!(
            sheet.results[&done_id('e')],
            Err("child ccccccc did not close".to_owned())
        );
        assert!(app.pending.is_empty());
    }

    #[test]
    fn unticking_a_child_disables_its_epic() {
        let mut app = sheet_open();
        app.send(Message::ToggleDone(done_id('c')));
        assert_eq!(sheet(&app).selected_count(), 1, "only bug b is chosen");
        assert_eq!(app.update(Message::RunDone).units(), 1);
        app.send(closed('b', Ok(())));
        assert_eq!(sheet(&app).phase, DonePhase::Closed);
        assert!(!sheet(&app).results.contains_key(&done_id('e')));

        let mut app = sheet_open();
        app.send(Message::PickDone(false));
        assert_eq!(app.update(Message::RunDone).units(), 0, "nothing ticked");
        assert_eq!(sheet(&app).phase, DonePhase::Choose);
        app.send(Message::PickDone(true));
        assert_eq!(sheet(&app).selected_count(), 3);
    }

    #[test]
    fn a_row_with_another_write_in_flight_fails_at_once() {
        let mut app = sheet_open();
        app.pending.insert((info("a").root, done_id('b')));
        assert_eq!(app.update(Message::RunDone).units(), 1);
        assert!(sheet(&app).results[&done_id('b')].is_err());
        assert_eq!(app.update(closed('c', Ok(()))).units(), 0);
        assert_eq!(sheet(&app).phase, DonePhase::Closed);
        assert!(sheet(&app).results[&done_id('e')].is_err());
    }

    #[test]
    fn escape_closes_the_sheet_before_the_pane_but_never_mid_run() {
        let mut app = sheet_open();
        let _ = select(&mut app);
        app.send(Message::CloseDetail);
        assert!(app.done.is_none());
        assert!(app.detail.is_some());

        let mut app = sheet_open();
        let _ = app.update(Message::RunDone);
        app.send(Message::DismissDone);
        app.send(Message::CloseDetail);
        assert_eq!(
            sheet(&app).phase,
            DonePhase::Closing,
            "a run cannot be dismissed"
        );
        app.send(closed('b', Ok(())));
        assert_eq!(app.update(closed('c', Ok(()))).units(), 1);
        app.send(closed('e', Ok(())));
        app.send(Message::DismissDone);
        assert!(app.done.is_none());
    }

    #[test]
    fn dropping_the_repo_closes_its_sheet() {
        let mut app = sheet_open();
        app.epoch = 2;
        app.send(Message::Discovered {
            epoch: 2,
            repos: vec![],
        });
        assert!(app.done.is_none());
    }

    fn press(app: &mut App, key: Key, modifiers: Modifiers, typing: bool) -> Task<Message> {
        app.update(Message::KeyPressed {
            key,
            modifiers,
            typing,
        })
    }

    fn tap_task(app: &mut App, key: &str) -> Task<Message> {
        press(app, Key::Character(key.into()), Modifiers::NONE, false)
    }

    fn cmd_task(app: &mut App, key: &str) -> Task<Message> {
        press(app, Key::Character(key.into()), Modifiers::COMMAND, false)
    }

    fn tap(app: &mut App, key: &str) {
        let _ = tap_task(app, key);
    }

    fn cmd(app: &mut App, key: &str) {
        let _ = cmd_task(app, key);
    }

    fn enter(app: &mut App) {
        let _ = press(app, Key::Named(Named::Enter), Modifiers::NONE, false);
    }

    fn escape(app: &mut App, typing: bool) {
        let _ = press(app, Key::Named(Named::Escape), Modifiers::NONE, typing);
    }

    fn numbered(count: usize) -> App {
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
            result: Ok((0..count)
                .map(|n| Issue {
                    id: format!("{n:064}"),
                    ..titled(&format!("issue {n}"), Status::Open)
                })
                .collect()),
        });
        app.section = Section::Issues;
        app
    }

    fn cursor_title(app: &App) -> Option<String> {
        app.cursor_index()
            .map(|i| app.row_target(i).1.title.clone())
    }

    fn detail_title(app: &App) -> Option<&str> {
        app.detail.as_ref().map(|p| p.summary.title.as_str())
    }

    #[test]
    fn command_digits_navigate_and_b_toggles_the_sidebar() {
        let mut app = App::from_parts(Workspace::default(), Snapshot::default(), None);
        app.config_locked = true;
        cmd(&mut app, "3");
        assert_eq!(app.section, Section::ALL[2]);
        cmd(&mut app, "8");
        assert_eq!(app.section, Section::ALL[7]);
        cmd(&mut app, "b");
        assert!(app.workspace.sidebar_collapsed);
    }

    #[test]
    fn command_shift_l_cycles_the_theme() {
        let mut app = App::from_parts(Workspace::default(), Snapshot::default(), None);
        app.config_locked = true;
        let mods = Modifiers::COMMAND | Modifiers::SHIFT;
        let _ = press(&mut app, Key::Character("l".into()), mods, false);
        assert_eq!(app.workspace.theme, ThemePref::Light);
    }

    #[test]
    fn command_k_and_f_land_on_issues_search() {
        for key in ["k", "f"] {
            let mut app = App::from_parts(Workspace::default(), Snapshot::default(), None);
            assert_eq!(app.section, Section::Repositories);
            assert_eq!(cmd_task(&mut app, key).units(), 1, "focus operation");
            assert_eq!(app.section, Section::Issues);
        }
        let mut app = App::from_parts(Workspace::default(), Snapshot::default(), None);
        assert_eq!(app.update(Message::FocusSearch).units(), 1);
        assert_eq!(app.section, Section::Issues);
    }

    #[test]
    fn command_r_rescans() {
        let mut app = App::from_parts(Workspace::default(), Snapshot::default(), None);
        assert!(!app.discovering);
        cmd(&mut app, "r");
        assert!(app.discovering);
    }

    #[test]
    fn j_and_k_move_the_cursor_and_clamp() {
        let mut app = numbered(3);
        assert_eq!(cursor_title(&app), None);
        tap(&mut app, "j");
        assert_eq!(cursor_title(&app).as_deref(), Some("issue 0"));
        tap(&mut app, "j");
        tap(&mut app, "j");
        tap(&mut app, "j");
        assert_eq!(cursor_title(&app).as_deref(), Some("issue 2"));
        tap(&mut app, "k");
        assert_eq!(cursor_title(&app).as_deref(), Some("issue 1"));
        for _ in 0..2 {
            let _ = press(&mut app, Key::Named(Named::ArrowUp), Modifiers::NONE, false);
        }
        assert_eq!(cursor_title(&app).as_deref(), Some("issue 0"));
        assert!(app.detail.is_none(), "moving alone does not open anything");
    }

    #[test]
    fn single_keys_do_nothing_while_typing_or_off_the_issues_page() {
        let mut app = numbered(3);
        let _ = press(&mut app, Key::Character("j".into()), Modifiers::NONE, true);
        assert_eq!(app.cursor, None);
        app.section = Section::Epics;
        tap(&mut app, "j");
        assert_eq!(app.cursor, None);
    }

    #[test]
    fn enter_opens_the_cursor_and_an_open_pane_follows_moves() {
        let mut app = numbered(3);
        tap(&mut app, "j");
        tap(&mut app, "j");
        assert!(app.detail.is_none());
        enter(&mut app);
        assert_eq!(detail_title(&app), Some("issue 1"));
        tap(&mut app, "j");
        assert_eq!(detail_title(&app), Some("issue 2"));
        assert_eq!(cursor_title(&app).as_deref(), Some("issue 2"));
    }

    #[test]
    fn submitting_the_search_opens_the_first_result() {
        let mut app = numbered(3);
        app.send(Message::OpenCursor);
        assert_eq!(detail_title(&app), Some("issue 0"));
    }

    #[test]
    fn submitting_the_search_ignores_a_stale_cursor() {
        let mut app = numbered(3);
        tap(&mut app, "j");
        tap(&mut app, "j");
        tap(&mut app, "j");
        app.search = "issue".into();
        app.send(Message::OpenCursor);
        assert_eq!(detail_title(&app), Some("issue 0"));
    }

    #[test]
    fn x_closes_the_cursor_issue() {
        let mut app = numbered(2);
        assert_eq!(tap_task(&mut app, "x").units(), 0, "no cursor, no write");
        tap(&mut app, "j");
        tap(&mut app, "j");
        assert_eq!(tap_task(&mut app, "x").units(), 1);
        assert!(app.is_pending(&info("a").root, &format!("{:064}", 1)));
        assert!(!app.is_pending(&info("a").root, &format!("{:064}", 0)));
    }

    #[test]
    fn question_mark_toggles_help_and_escape_closes_it_before_the_pane() {
        let mut app = numbered(2);
        tap(&mut app, "j");
        enter(&mut app);
        tap(&mut app, "?");
        assert!(app.help);
        tap(&mut app, "j");
        assert_eq!(
            cursor_title(&app).as_deref(),
            Some("issue 0"),
            "help is modal"
        );
        escape(&mut app, false);
        assert!(!app.help);
        assert!(app.detail.is_some());
        escape(&mut app, false);
        assert!(app.detail.is_none());
        cmd(&mut app, "/");
        assert!(app.help);
        cmd(&mut app, "/");
        assert!(!app.help);
    }

    #[test]
    fn escape_while_typing_leaves_the_pane_alone() {
        let mut app = numbered(1);
        enter(&mut app);
        assert!(app.detail.is_some());
        escape(&mut app, true);
        assert!(app.detail.is_some());
    }

    fn form(app: &App) -> &NewIssueForm {
        app.new_issue.as_ref().expect("dialog is open")
    }

    fn created_issue(kind: Kind) -> Issue {
        Issue {
            id: "f".repeat(64),
            human_id: "fffffff".into(),
            kind: Some(kind),
            ..titled("new one", Status::Open)
        }
    }

    fn created(root: PathBuf, issue: Issue) -> Message {
        Message::IssueCreated {
            root,
            result: Ok(Box::new(IssueDetail {
                issue,
                body: "body".into(),
                comments: vec![],
            })),
        }
    }

    fn fill(app: &mut App, title: &str) {
        app.send(Message::NewIssueTitle(title.into()));
    }

    #[test]
    fn the_dialog_defaults_to_the_filtered_repo_else_the_first_one() {
        let mut app = booted(&["a", "b"]);
        app.send(Message::OpenNewIssue(NewKind::Bug));
        assert_eq!(form(&app).root, info("a").root);
        assert_eq!(form(&app).kind, NewKind::Bug);

        let mut app = booted(&["a", "b"]);
        app.send(Message::PickRepo(Some(info("b").root)));
        app.send(Message::OpenNewIssue(NewKind::Task));
        assert_eq!(form(&app).root, info("b").root);
        assert_eq!(form(&app).kind, NewKind::Task);
    }

    #[test]
    fn without_a_writable_repo_the_dialog_says_so_instead_of_opening() {
        let mut app = App::from_parts(Workspace::default(), Snapshot::default(), None);
        app.send(Message::OpenNewIssue(NewKind::Bug));
        assert!(app.new_issue.is_none());
        assert_eq!(app.notifications.len(), 1);
    }

    #[test]
    fn command_n_opens_the_dialog_and_it_swallows_other_shortcuts() {
        let mut app = numbered(1);
        cmd(&mut app, "n");
        assert!(app.new_issue.is_some());
        cmd(&mut app, "2");
        assert_eq!(app.section, Section::Issues);
        tap(&mut app, "j");
        assert!(app.cursor.is_none());
        escape(&mut app, false);
        assert!(app.new_issue.is_none());
    }

    #[test]
    fn an_empty_title_never_reaches_git_bug() {
        let mut app = booted(&["a"]);
        app.send(Message::OpenNewIssue(NewKind::Bug));
        fill(&mut app, "   ");
        assert_eq!(app.update(Message::SubmitNewIssue).units(), 0);
        assert!(!form(&app).submitting);
        assert_eq!(form(&app).error.as_deref(), Some("title is empty"));
    }

    #[test]
    fn submitting_locks_the_form_and_sends_exactly_one_create() {
        let mut app = booted(&["a"]);
        app.send(Message::OpenNewIssue(NewKind::Bug));
        fill(&mut app, "boom");
        assert_eq!(app.update(Message::SubmitNewIssue).units(), 1);
        assert!(form(&app).submitting);

        assert_eq!(app.update(Message::SubmitNewIssue).units(), 0);
        fill(&mut app, "changed");
        app.send(Message::NewIssueKind(NewKind::Epic));
        assert_eq!(form(&app).title, "boom");
        assert_eq!(form(&app).kind, NewKind::Bug);

        app.send(Message::DismissNewIssue);
        escape(&mut app, false);
        assert!(app.new_issue.is_some(), "never closes mid-create");
    }

    #[test]
    fn a_failed_create_keeps_the_form_and_shows_the_error() {
        let mut app = booted(&["a"]);
        app.send(Message::OpenNewIssue(NewKind::Bug));
        fill(&mut app, "boom");
        let _ = app.update(Message::SubmitNewIssue);
        app.send(Message::IssueCreated {
            root: info("a").root,
            result: Err("git-bug bug new failed: nope".into()),
        });
        let f = form(&app);
        assert!(!f.submitting);
        assert_eq!(f.title, "boom");
        assert_eq!(f.error.as_deref(), Some("git-bug bug new failed: nope"));
        assert!(app.notifications.is_empty());
    }

    #[test]
    fn a_created_issue_is_listed_selected_and_announced() {
        let mut app = numbered(2);
        app.send(Message::OpenNewIssue(NewKind::Task));
        fill(&mut app, "new one");
        let _ = app.update(Message::SubmitNewIssue);
        app.send(created(info("a").root, created_issue(Kind::Task)));

        assert!(app.new_issue.is_none());
        assert_eq!(app.repos[0].issues.len(), 3);
        assert_eq!(app.repos[0].state, LoadState::Ready(3));
        assert_eq!(cursor_title(&app).as_deref(), Some("new one"));
        assert_eq!(detail_title(&app), Some("new one"));
        assert_eq!(app.section, Section::Issues);
        let last = app.notifications.last().unwrap();
        assert_eq!(last.title, "Created fffffff");
        assert_eq!(last.detail.as_deref(), Some("new one"));
        assert!(
            app.batch.is_some(),
            "the repo is re-checked for a real load"
        );
    }

    #[test]
    fn a_created_issue_hidden_by_the_filters_clears_them() {
        let mut app = numbered(2);
        app.send(Message::ToggleKind(Kind::Epic));
        assert!(app.rows.is_empty());
        app.send(Message::OpenNewIssue(NewKind::Task));
        fill(&mut app, "new one");
        let _ = app.update(Message::SubmitNewIssue);
        app.send(created(info("a").root, created_issue(Kind::Task)));
        assert_eq!(app.filter, Filter::default());
        assert_eq!(cursor_title(&app).as_deref(), Some("new one"));
    }

    #[test]
    fn open_epics_list_only_open_epics_of_that_repo() {
        let mut app = epic_app();
        assert_eq!(app.open_epics(&info("a").root).len(), 1);
        let root = info("a").root;
        let closed = Issue {
            id: "c".repeat(64),
            kind: Some(Kind::Epic),
            ..titled("old epic", Status::Closed)
        };
        let _ = app.insert_issue(&root, closed);
        assert_eq!(app.open_epics(&root).len(), 1);
        assert!(app.open_epics(Path::new("/r/none")).is_empty());
    }

    #[test]
    fn changing_the_repo_drops_the_chosen_epic() {
        let mut app = booted(&["a", "b"]);
        app.send(Message::OpenNewIssue(NewKind::Task));
        app.send(Message::NewIssueEpic(Some("e".repeat(64))));
        app.send(Message::NewIssueRepo(info("a").root));
        assert!(form(&app).epic.is_some(), "same repo keeps it");
        app.send(Message::NewIssueRepo(info("b").root));
        assert_eq!(form(&app).root, info("b").root);
        assert!(form(&app).epic.is_none());
    }

    #[test]
    fn dropping_the_repo_closes_its_dialog() {
        let mut app = booted(&["a", "b"]);
        app.send(Message::PickRepo(Some(info("b").root)));
        app.send(Message::OpenNewIssue(NewKind::Bug));
        app.epoch = 2;
        app.send(Message::Discovered {
            epoch: 2,
            repos: vec![info("a")],
        });
        assert!(app.new_issue.is_none());
    }

    #[test]
    fn the_dialog_view_builds_in_every_shape() {
        let mut app = epic_app();
        for kind in NewKind::ALL {
            app.new_issue = None;
            app.send(Message::OpenNewIssue(kind));
            let _ = app.view();
            fill(&mut app, "x");
            let _ = app.update(Message::SubmitNewIssue);
            let _ = app.view();
        }
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
