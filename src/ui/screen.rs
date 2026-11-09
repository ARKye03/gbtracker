use std::fmt;
use std::path::{Path, PathBuf};

use iced::widget::{
    button, column, container, lazy, pick_list, row, scrollable, space, stack, text,
    text::Wrapping, text_input,
};
use iced::{Alignment, Element, Fill, Padding};
use lucide_iced::icon as lu;

use super::components::{
    MEDIUM, card_grid, chip, choice_button, detail_row, dot, icon, icon_muted, icon_pill,
    icon_pill_hint, icon_square_button, icon_square_button_hint, labeled, primary_button,
    secondary_button, skeleton, spinner_dot, split_progress, toggle_chip, top_pill, vdivider,
};
use super::popover::{self, Align};
use super::tooltip::tip;
use super::{detail, done, new_issue, shortcuts};
use crate::Section;
use crate::app::{App, LIST_ID, LoadState, Message, Popover, RepoEntry, Row, SEARCH_ID, ago};
use crate::keymap::{self, Action};
use crate::theme::{Tone, style};
use gbtracker::epics::{self, EpicSummary};
use gbtracker::filter::{Filter, StateFilter, StatusFilter};
use gbtracker::gitbug::{Issue, Kind, NewKind, State, Status, WriteOp};
use gbtracker::workspace::{Root, RootKind, ThemePref};

const SIDEBAR_WIDTH: f32 = 200.0;
const RAIL_WIDTH: f32 = 56.0;
const NAV_BAR_WIDTH: f32 = 3.0;
const NAV_GAP: f32 = 4.0;
const NAV_ICON: f32 = 20.0;
const NAV_PAD: f32 = 16.0;

/// Left padding that keeps the nav icon at `NAV_PAD` when expanded and centered in the button at the rail width.
fn nav_icon_pad(width: f32) -> f32 {
    let button = RAIL_WIDTH - NAV_BAR_WIDTH - NAV_GAP;
    let centered = (button - NAV_ICON) / 2.0;
    let collapse = ((SIDEBAR_WIDTH - width) / (SIDEBAR_WIDTH - RAIL_WIDTH)).clamp(0.0, 1.0);
    NAV_PAD + (centered - NAV_PAD) * collapse
}

pub fn view(app: &App) -> Element<'_, Message> {
    let page = match app.section {
        Section::Repositories => repositories(app),
        Section::Issues => issues(app),
        Section::Epics => epics_page(app),
        Section::Overview => overview(app),
    };
    let mut body = row![sidebar(app), page].spacing(20).height(Fill);
    if matches!(app.section, Section::Issues | Section::Epics)
        && let Some(pane) = detail::view(app)
    {
        body = body.push(pane);
    }

    let page = container(
        column![
            top_bar(app),
            container(body)
                .padding(Padding {
                    top: 28.0,
                    right: 24.0,
                    bottom: 0.0,
                    left: 28.0,
                })
                .width(Fill)
                .height(Fill)
                .style(style::panel),
        ]
        .spacing(14),
    )
    .padding(Padding {
        top: 14.0,
        right: 14.0,
        bottom: 0.0,
        left: 14.0,
    })
    .width(Fill)
    .height(Fill)
    .style(style::app);

    let page = popover::scrim(
        app,
        new_issue::overlay(app, done::overlay(app, page.into())),
    );
    shortcuts::overlay(app, page)
}

fn top_bar(app: &App) -> Element<'_, Message> {
    let (theme_icon, theme_label) = match app.workspace.theme {
        ThemePref::System => (lu::monitor(), "Theme: System"),
        ThemePref::Light => (lu::sun(), "Theme: Light"),
        ThemePref::Dark => (lu::moon(), "Theme: Dark"),
    };

    let left = row![
        icon_pill_hint(
            lu::panel_left(),
            "Toggle sidebar",
            keymap::hint(Action::ToggleSidebar),
            Message::ToggleSidebar
        ),
        tip(
            top_pill(labeled(lu::search(), "Search"), false, Message::FocusSearch),
            "Search"
        )
        .shortcut_maybe(keymap::hint(Action::FocusSearch)),
        tip(
            top_pill(
                labeled(lu::plus(), "New Bug"),
                false,
                Message::OpenNewIssue(NewKind::Bug)
            ),
            "New issue"
        )
        .shortcut_maybe(keymap::hint(Action::NewIssue)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let sync = sync_pill(app);

    let bell = icon_pill(
        lu::bell(),
        "Notifications",
        Message::TogglePopover(Popover::Notifications),
    );
    let bell: Element<'_, Message> = if app.unread > 0 {
        stack![
            bell,
            container(unread_badge(app.unread))
                .align_top(Fill)
                .align_right(Fill)
        ]
        .into()
    } else {
        bell
    };

    let right = row![
        sync,
        icon_pill_hint(
            lu::keyboard(),
            "Keyboard shortcuts",
            keymap::hint(Action::Help),
            Message::ToggleHelp
        ),
        icon_pill_hint(
            theme_icon,
            theme_label,
            keymap::hint(Action::CycleTheme),
            Message::ToggleTheme
        ),
        popover::anchored(app, Popover::Notifications, Align::End, bell),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    row![left, space::horizontal(), right]
        .align_y(Alignment::Center)
        .into()
}

fn unread_badge(count: usize) -> Element<'static, Message> {
    let label = if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    };
    container(text(label).size(10).font(MEDIUM))
        .padding([1, 5])
        .style(style::badge)
        .into()
}

fn sync_pill(app: &App) -> Element<'_, Message> {
    let (label, indicator): (String, Element<'_, Message>) = match app.batch {
        Some(b) if b.visible => (
            format!("Loading {}/{}…", b.done, b.total),
            spinner_dot(app.spin),
        ),
        _ if app.discovering => ("Scanning…".to_owned(), spinner_dot(app.spin)),
        _ if app.failed_count() > 0 => (
            format!("{} failed", app.failed_count()),
            dot(Tone::Danger, 18.0),
        ),
        _ => match app.last_synced {
            Some(t) => (
                format!("Synced {}", ago(t.elapsed().unwrap_or_default())),
                dot(Tone::Success, 18.0),
            ),
            None => (
                "Not synced".to_owned(),
                container(space())
                    .width(18)
                    .height(18)
                    .style(style::neutral_dot)
                    .into(),
            ),
        },
    };

    container(
        row![text(label).size(13).style(style::subtle), indicator]
            .spacing(10)
            .align_y(Alignment::Center),
    )
    .height(40)
    .padding([0, 12])
    .center_y(40)
    .style(style::status_pill)
    .into()
}

fn sidebar(app: &App) -> Element<'_, Message> {
    let width = app.sidebar.interpolate(SIDEBAR_WIDTH, RAIL_WIDTH, app.now);
    // Squared so labels are gone before the narrowing rail clips them.
    let alpha = app.sidebar.interpolate(1.0_f32, 0.0, app.now).powi(2);
    let active = app.section;
    let collapsed = app.sidebar.value();
    let pad = nav_icon_pad(width);
    let items = Section::ALL.iter().map(|&s| {
        let (svg, label) = s.meta();
        let is_active = s == active;
        let nav = button(
            row![
                icon(svg, NAV_ICON),
                text(label)
                    .size(15)
                    .wrapping(Wrapping::None)
                    .style(style::faded(alpha))
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .width(Fill)
        .padding(Padding {
            top: 8.0,
            right: pad,
            bottom: 8.0,
            left: pad,
        })
        .style(if is_active {
            style::nav_item_active
        } else {
            style::nav_item
        })
        .on_press(Message::Navigate(s));
        row![
            container(space())
                .width(NAV_BAR_WIDTH)
                .height(22)
                .style(style::nav_bar(is_active)),
            tip(nav, label)
                .shortcut_maybe(keymap::hint(Action::Goto(s)))
                .enabled(collapsed)
        ]
        .spacing(NAV_GAP)
        .align_y(Alignment::Center)
        .into()
    });

    container(column(items).spacing(2))
        .width(width)
        .clip(true)
        .into()
}

fn epics_page(app: &App) -> Element<'_, Message> {
    let count = if app.epics.len() == 1 {
        "1 epic".to_owned()
    } else {
        format!("{} epics", app.epics.len())
    };
    let status = row([
        ("Open", StatusFilter::Open),
        ("Closed", StatusFilter::Closed),
        ("All", StatusFilter::All),
    ]
    .into_iter()
    .map(|(label, value)| {
        toggle_chip(
            label,
            app.epic_filter.status == value,
            false,
            Message::SetEpicStatus(value),
        )
    }))
    .spacing(6);

    let header = row![
        text("Epics").size(21),
        text(count).size(14).style(style::muted),
        space::horizontal(),
        status,
        repo_picker(app, &app.epic_filter, Message::PickEpicRepo),
    ]
    .spacing(18)
    .align_y(Alignment::Center);

    let mut list = column![].spacing(16).padding(Padding {
        bottom: 24.0,
        right: 10.0,
        ..Padding::ZERO
    });
    list = list.extend(app.epics.iter().map(|s| epic_card(app, s)));
    if app.loading_repos() > 0 || app.discovering {
        list = list.extend((0..2).map(|_| skeleton_card()));
    } else if app.epics.is_empty() {
        list = list.push(
            container(
                text(if app.repos.iter().all(|e| e.issues.is_empty()) {
                    "No epics yet."
                } else {
                    "No epics match these filters."
                })
                .size(14)
                .style(style::muted),
            )
            .padding(48)
            .center_x(Fill)
            .style(style::card),
        );
    }

    column![header, scrollable(list).height(Fill).style(style::scroll)]
        .spacing(28)
        .width(Fill)
        .into()
}

fn epic_card<'a>(app: &'a App, summary: &EpicSummary) -> Element<'a, Message> {
    let entry = &app.repos[summary.repo];
    let epic = &entry.issues[summary.epic];
    let expanded = app.expanded.contains(&entry.info.root, &epic.id);

    let headline = if summary.total == 1 {
        "1 task".to_owned()
    } else {
        format!("{} tasks", summary.total)
    };
    let chevron = if expanded {
        lu::chevron_up()
    } else {
        lu::chevron_down()
    };
    let heading = row![
        button(column![text(headline).size(29), text(epic.title.as_str()).size(18)].spacing(8))
            .padding(6)
            .style(style::list_item)
            .on_press(select(&entry.info.root, epic)),
        space::horizontal(),
        icon_square_button(
            chevron,
            16.0,
            36.0,
            if expanded { "Collapse" } else { "Expand" },
            Some(Message::ToggleEpic(
                entry.info.root.clone(),
                epic.id.clone()
            ))
        ),
    ];

    let details = column![
        detail_row("Closed", format!("{} of {}", summary.closed, summary.total)),
        detail_row("Repository", entry.info.root.display().to_string()),
        detail_row("Opened", epics::format_date(&epic.created)),
        detail_row("Last Activity", epics::format_date(&summary.last_activity)),
        detail_row(
            "In progress / Blocked",
            format!("{} / {}", summary.in_progress, summary.blocked)
        ),
    ]
    .spacing(18);

    let labels = row(epic
        .labels
        .iter()
        .filter(|l| super::is_user_label(l))
        .map(|l| chip(l.as_str())))
    .spacing(8)
    .wrap()
    .vertical_spacing(8);

    let mut card = column![
        heading,
        split_progress(summary.ratio()),
        details,
        row![
            labels,
            space::horizontal(),
            text(epic.human_id.as_str()).size(13).style(style::subtle)
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    ]
    .spacing(28);

    if expanded {
        let children: Vec<Element<'a, Message>> = if summary.children.is_empty() {
            vec![text("No tasks yet.").size(14).style(style::muted).into()]
        } else {
            summary
                .children
                .iter()
                .map(|&c| child_row(&entry.info.root, &entry.issues[c]))
                .collect()
        };
        card = card.push(column(children).spacing(12));
    }

    container(card)
        .padding(28)
        .width(Fill)
        .style(style::card)
        .into()
}

fn select(root: &Path, issue: &Issue) -> Message {
    Message::SelectIssue {
        root: root.to_owned(),
        id: issue.id.clone(),
    }
}

pub(super) fn layer_label(issue: &Issue) -> String {
    match issue.layer {
        Some(n) => format!("L{n}"),
        None => "—".to_owned(),
    }
}

fn child_row<'a>(root: &Path, issue: &'a Issue) -> Element<'a, Message> {
    button(
        row![
            text(layer_label(issue))
                .size(13)
                .style(style::subtle)
                .width(32),
            text(issue.human_id.as_str()).size(13).style(style::subtle),
            text(issue.title.as_str()).size(14),
            space::horizontal(),
            state_badge(issue),
        ]
        .spacing(14)
        .align_y(Alignment::Center),
    )
    .width(Fill)
    .padding([6, 8])
    .style(style::list_item)
    .on_press(select(root, issue))
    .into()
}

fn overview(app: &App) -> Element<'_, Message> {
    let tracked: Vec<&RepoEntry> = app.repos.iter().filter(|e| e.info.has_git_bug).collect();
    let header = row![
        text("Overview").size(21),
        text!("{} repositories", tracked.len())
            .size(14)
            .style(style::muted),
    ]
    .spacing(18)
    .align_y(Alignment::Center);

    let body: Element<'_, Message> = if tracked.is_empty() {
        if app.discovering {
            column((0..3).map(|_| skeleton_card())).spacing(16).into()
        } else {
            container(
                column![
                    text("No repositories with git-bug yet.")
                        .size(14)
                        .style(style::muted),
                    row![
                        secondary_button("Add folder to scan", Message::PickFolder(RootKind::Scan)),
                        primary_button("Add project", Message::PickFolder(RootKind::Project)),
                    ]
                    .spacing(12),
                ]
                .spacing(18)
                .align_x(Alignment::Center),
            )
            .padding(48)
            .center_x(Fill)
            .style(style::card)
            .into()
        }
    } else {
        let spin = app.spin;
        card_grid(tracked, 380.0, 16.0, move |e| overview_card(e, spin))
    };

    column![
        header,
        scrollable(container(body).padding(Padding {
            bottom: 24.0,
            right: 10.0,
            ..Padding::ZERO
        }))
        .height(Fill)
        .style(style::scroll)
    ]
    .spacing(28)
    .width(Fill)
    .into()
}

fn overview_card(entry: &RepoEntry, spin: usize) -> Element<'_, Message> {
    let counts = epics::count_open(&entry.issues);
    #[expect(
        clippy::cast_precision_loss,
        reason = "issue counts are far below f32's exact integer range; the ratio only drives a progress bar"
    )]
    let ratio = if entry.issues.is_empty() {
        0.0
    } else {
        entry.closed_count() as f32 / entry.issues.len() as f32
    };

    let heading = row![
        column![
            text!("{} open", entry.open_count()).size(29),
            text(entry.info.name.as_str()).size(18),
        ]
        .spacing(8),
        space::horizontal(),
        status_badge(&entry.state, spin),
    ];

    let details = column![
        detail_row("Open epics", counts.epics.to_string()),
        detail_row("Open tasks", counts.tasks.to_string()),
        detail_row("Open bugs", counts.bugs.to_string()),
        detail_row("In progress", counts.in_progress.to_string()),
        detail_row("Blocked", counts.blocked.to_string()),
    ]
    .spacing(14);

    container(
        column![
            heading,
            split_progress(ratio),
            details,
            text(entry.info.root.display().to_string())
                .size(13)
                .style(style::subtle),
        ]
        .spacing(24),
    )
    .padding(28)
    .width(Fill)
    .style(style::card)
    .into()
}

fn repositories(app: &App) -> Element<'_, Message> {
    let header = row![
        text("Repositories").size(21),
        space::horizontal(),
        secondary_button("Rescan", Message::Rescan),
        secondary_button("Add folder to scan", Message::PickFolder(RootKind::Scan)),
        primary_button("Add project", Message::PickFolder(RootKind::Project)),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let mut body = column![].spacing(10).padding(Padding {
        bottom: 24.0,
        right: 10.0,
        ..Padding::ZERO
    });
    if let Some(notice) = &app.notice {
        body = body.push(
            container(text(notice).size(14))
                .padding([12, 16])
                .width(Fill)
                .style(style::banner(Tone::Danger)),
        );
    }
    body = body.push(roots_card(app));
    if app.repos.is_empty() {
        if app.discovering {
            body = body.extend((0..3).map(|_| skeleton_repo_row()));
        }
    } else {
        body = body.extend(app.repos.iter().map(|r| repo_card(r, app.spin)));
    }

    column![header, scrollable(body).height(Fill).style(style::scroll)]
        .spacing(28)
        .width(Fill)
        .into()
}

fn roots_card(app: &App) -> Element<'_, Message> {
    let rows: Vec<Element<'_, Message>> = if app.workspace.roots.is_empty() {
        vec![
            text("No projects yet. Add a project or a folder to scan.")
                .size(14)
                .style(style::muted)
                .into(),
        ]
    } else {
        app.workspace
            .roots
            .iter()
            .enumerate()
            .map(|(i, r)| root_row(i, r))
            .collect()
    };
    container(column![text("Roots").size(18), column(rows).spacing(10)].spacing(18))
        .padding(28)
        .width(Fill)
        .style(style::card)
        .into()
}

fn root_row(index: usize, root: &Root) -> Element<'_, Message> {
    let (svg, kind) = match root.kind {
        RootKind::Project => (lu::folder_git_2(), "project"),
        RootKind::Scan => (lu::folder(), "scan"),
    };
    row![
        icon_muted(svg, 16.0),
        text(root.path.display().to_string()).size(14),
        chip(kind),
        space::horizontal(),
        icon_square_button(
            lu::x(),
            14.0,
            32.0,
            "Remove folder",
            Some(Message::RemoveRoot(index))
        ),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

pub(super) fn status_label(state: &LoadState) -> (&'static str, Option<Tone>) {
    match state {
        LoadState::Cached => ("Cached", Some(Tone::Warning)),
        LoadState::Loading => ("Loading", None),
        LoadState::Refreshing => ("Refreshing", None),
        LoadState::Ready(_) => ("Ready", Some(Tone::Success)),
        LoadState::NoGitBug => ("No git-bug", None),
        LoadState::Locked => ("Locked", Some(Tone::Warning)),
        LoadState::Error(_) => ("Error", Some(Tone::Danger)),
    }
}

pub(super) fn status_indicator(state: &LoadState, spin: usize) -> Element<'static, Message> {
    match (state.is_busy(), status_label(state).1) {
        (true, _) => spinner_dot(spin),
        (false, Some(tone)) => dot(tone, 10.0),
        (false, None) => container(space())
            .width(10)
            .height(10)
            .style(style::neutral_dot)
            .into(),
    }
}

fn status_badge(state: &LoadState, spin: usize) -> Element<'static, Message> {
    let (label, _) = status_label(state);
    row![
        status_indicator(state, spin),
        text(label).size(13).style(style::subtle)
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn repo_card(entry: &RepoEntry, spin: usize) -> Element<'_, Message> {
    let mut heading = row![
        column![
            text(&entry.info.name).size(16),
            text(entry.info.root.display().to_string())
                .size(13)
                .style(style::subtle),
        ]
        .spacing(2),
        space::horizontal(),
    ]
    .spacing(16)
    .align_y(Alignment::Center);
    if entry.fingerprint.is_some()
        && !matches!(entry.state, LoadState::Error(_) | LoadState::NoGitBug)
    {
        heading = heading.push(
            text(format!("{} open", entry.open_count()))
                .size(13)
                .style(style::subtle),
        );
    }
    if entry.info.has_git_bug {
        heading = heading.push(choice_button(
            "Check Done",
            false,
            entry
                .fingerprint
                .is_some()
                .then(|| Message::OpenDone(entry.info.root.clone())),
        ));
    }
    heading = heading.push(status_badge(&entry.state, spin));

    let message: Option<Element<'_, Message>> = match &entry.state {
        LoadState::NoGitBug => Some(
            text("git-bug is not initialised in this repository.")
                .size(13)
                .style(style::muted)
                .into(),
        ),
        LoadState::Error(msg) => Some(
            text(msg)
                .size(13)
                .style(style::tone_text(Tone::Danger, false))
                .into(),
        ),
        LoadState::Locked => Some(
            text("Locked by another git-bug process; retrying.")
                .size(13)
                .style(style::tone_text(Tone::Warning, false))
                .into(),
        ),
        _ => None,
    };

    let mut content = column![heading].spacing(8);
    if let Some(message) = message {
        content = content.push(message);
    }
    container(content)
        .padding([14, 20])
        .width(Fill)
        .style(style::card)
        .into()
}

fn skeleton_repo_row() -> Element<'static, Message> {
    container(
        row![
            column![skeleton(160.0, 16.0), skeleton(240.0, 12.0)].spacing(6),
            space::horizontal(),
            skeleton(60.0, 14.0),
        ]
        .spacing(16)
        .align_y(Alignment::Center),
    )
    .padding([14, 20])
    .width(Fill)
    .style(style::card)
    .into()
}

fn skeleton_card() -> Element<'static, Message> {
    container(
        column![
            skeleton(180.0, 20.0),
            skeleton(260.0, 12.0),
            skeleton(Fill, 12.0),
        ]
        .spacing(14),
    )
    .padding(28)
    .width(Fill)
    .style(style::card)
    .into()
}

#[derive(Clone, PartialEq)]
struct RepoChoice {
    root: Option<PathBuf>,
    name: String,
}

impl fmt::Display for RepoChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

fn issues(app: &App) -> Element<'_, Message> {
    let total = app.total_issues();
    let count = if app.rows.len() == total {
        format!("{total} issues")
    } else {
        format!("{} of {total} issues", app.rows.len())
    };
    let filtered = !app.filter.is_default() || app.active_work || !app.search.is_empty();

    let mut header = row![
        text("Issues").size(21),
        text(count).size(14).style(style::muted),
        space::horizontal(),
    ]
    .spacing(18)
    .align_y(Alignment::Center);
    if filtered {
        header = header.push(secondary_button("Clear filters", Message::ClearFilters));
    }
    header = header.push(if app.active_work {
        primary_button("Active work", Message::ToggleActiveWork)
    } else {
        secondary_button("Active work", Message::ToggleActiveWork)
    });

    column![
        header,
        filter_bar(app),
        scrollable(issue_list(app))
            .id(LIST_ID)
            .height(Fill)
            .style(style::scroll)
    ]
    .spacing(20)
    .width(Fill)
    .into()
}

// Rows are `lazy`, whose size hint is always `Shrink`; without an explicit width the column
// collapses and the `Fill` cards inside get none.
pub fn issue_list(app: &App) -> Element<'_, Message> {
    let total = app.total_issues();
    let mut list = column![].spacing(8).width(Fill).padding(Padding {
        bottom: 24.0,
        right: 10.0,
        ..Padding::ZERO
    });
    list = list.extend(app.rows.iter().take(app.shown).map(|r| issue_row(app, r)));
    if app.rows.len() > app.shown {
        list = list.push(
            button(text!("Show {} more", app.rows.len() - app.shown).size(14))
                .padding([10, 16])
                .style(style::secondary)
                .on_press(Message::ShowMore),
        );
    }
    if app.loading_repos() > 0 || app.discovering {
        list = list.extend((0..3).map(|_| skeleton_row()));
    } else if app.rows.is_empty() {
        list = list.push(
            container(
                text(if total == 0 {
                    "No issues yet."
                } else {
                    "No issues match these filters."
                })
                .size(14)
                .style(style::muted),
            )
            .padding(48)
            .center_x(Fill)
            .style(style::card),
        );
    }

    list.into()
}

fn filter_bar(app: &App) -> Element<'_, Message> {
    let filter = &app.filter;

    let kinds = row([
        ("Epic", Kind::Epic),
        ("Task", Kind::Task),
        ("Bug", Kind::Bug),
    ]
    .into_iter()
    .map(|(label, kind)| {
        let active = filter.kinds.contains(&kind);
        toggle_chip(label, active, true, Message::ToggleKind(kind))
    }))
    .spacing(6);

    let status = row([
        ("Open", StatusFilter::Open),
        ("Closed", StatusFilter::Closed),
        ("All", StatusFilter::All),
    ]
    .into_iter()
    .map(|(label, value)| {
        toggle_chip(
            label,
            filter.status == value,
            false,
            Message::SetStatus(value),
        )
    }))
    .spacing(6);

    let state = row([
        ("Any state", StateFilter::Any),
        ("In progress", StateFilter::InProgress),
        ("Blocked", StateFilter::Blocked),
    ]
    .into_iter()
    .map(|(label, value)| {
        toggle_chip(
            label,
            filter.state == value,
            false,
            Message::SetState(value),
        )
    }))
    .spacing(6);

    let repo = repo_picker(app, filter, Message::PickRepo);

    let search = text_input("Search title or id", &app.search)
        .id(SEARCH_ID)
        .on_input(Message::SearchInput)
        .on_submit(Message::OpenCursor)
        .size(14)
        .padding([8, 12])
        .width(260);

    row![kinds, vdivider(), status, vdivider(), state, repo, search]
        .spacing(14)
        .align_y(Alignment::Center)
        .wrap()
        .vertical_spacing(10)
        .into()
}

fn repo_picker<'a>(
    app: &'a App,
    filter: &Filter,
    on_pick: fn(Option<PathBuf>) -> Message,
) -> Element<'a, Message> {
    let mut choices = vec![RepoChoice {
        root: None,
        name: "All repositories".to_owned(),
    }];
    choices.extend(app.repos.iter().map(|e| RepoChoice {
        root: Some(e.info.root.clone()),
        name: e.info.name.clone(),
    }));
    let selected = filter
        .repos
        .iter()
        .next()
        .and_then(|root| choices.iter().find(|c| c.root.as_ref() == Some(root)))
        .or_else(|| choices.first())
        .cloned();
    pick_list(choices, selected, move |c: RepoChoice| on_pick(c.root))
        .text_size(14)
        .padding([7, 12])
        .into()
}

fn issue_row<'a>(app: &'a App, row: &Row) -> Element<'a, Message> {
    let entry = &app.repos[row.repo];
    let issue = &entry.issues[row.issue];
    let depth = row.depth;
    let selected = app
        .cursor
        .as_ref()
        .is_some_and(|(root, id)| *root == entry.info.root && *id == issue.id);
    let pending = app.is_pending(&entry.info.root, &issue.id);
    let error = app.write_errors.get(&entry.info.root, &issue.id);
    let key = (
        &entry.info.root,
        &entry.info.name,
        &issue.id,
        &issue.edited,
        issue.status == Status::Closed,
        depth,
        selected,
        pending.then_some(app.spin),
        error,
    );
    lazy(key, move |_| {
        issue_card(
            &entry.info.root,
            &entry.info.name,
            issue,
            depth,
            selected,
            pending.then_some(app.spin),
            error.map(String::as_str),
        )
    })
    .into()
}

pub(super) fn kind_label(kind: Option<&Kind>) -> String {
    match kind {
        Some(Kind::Epic) => "epic".to_owned(),
        Some(Kind::Task) => "task".to_owned(),
        Some(Kind::Bug) => "bug".to_owned(),
        Some(Kind::Other(other)) => other.clone(),
        None => "issue".to_owned(),
    }
}

pub(super) fn state_badge(issue: &Issue) -> Element<'static, Message> {
    let (label, tone) = match (&issue.status, &issue.state) {
        (Status::Closed, _) => ("Closed", None),
        (_, Some(State::InProgress)) => ("In progress", Some(Tone::Success)),
        (_, Some(State::Blocked)) => ("Blocked", Some(Tone::Danger)),
        (_, Some(State::Done)) => ("Done", None),
        _ => ("Open", None),
    };
    let indicator: Element<'static, Message> = match tone {
        Some(tone) => dot(tone, 10.0),
        None => container(space())
            .width(10)
            .height(10)
            .style(style::neutral_dot)
            .into(),
    };
    row![indicator, text(label).size(13).style(style::subtle)]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
}

fn issue_card(
    root: &Path,
    repo: &str,
    issue: &Issue,
    depth: usize,
    selected: bool,
    pending: Option<usize>,
    error: Option<&str>,
) -> Element<'static, Message> {
    let labels = row(issue
        .labels
        .iter()
        .filter(|l| super::is_user_label(l))
        .map(|l| chip(l.clone())))
    .spacing(6);

    let meta = row![
        text(issue.human_id.clone()).size(13).style(style::subtle),
        text(repo.to_owned()).size(13).style(style::subtle),
        labels,
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let mut details = column![text(issue.title.clone()).size(15).font(MEDIUM), meta].spacing(6);
    if let Some(error) = error {
        details = details.push(
            text(error.to_owned())
                .size(13)
                .style(style::tone_text(Tone::Danger, false)),
        );
    }

    let card = button(
        row![
            chip(kind_label(issue.kind.as_ref())),
            details,
            space::horizontal(),
            state_badge(issue),
        ]
        .spacing(14)
        .align_y(Alignment::Center),
    )
    .padding([14, 20])
    .width(Fill)
    .style(style::card_button(selected))
    .on_press(select(root, issue));

    let (op, glyph, hint) = if issue.status == Status::Closed {
        (WriteOp::Reopen, lu::rotate_ccw(), "Reopen issue")
    } else {
        (WriteOp::Close, lu::circle_check(), "Close issue")
    };
    let toggle: Element<'static, Message> = match pending {
        Some(phase) => container(spinner_dot(phase)).center(36).into(),
        None => icon_square_button_hint(
            glyph,
            16.0,
            36.0,
            hint,
            selected
                .then(|| keymap::hint(Action::ToggleStatus))
                .flatten(),
            Some(Message::Write {
                root: root.to_owned(),
                id: issue.id.clone(),
                op,
            }),
        ),
    };

    #[expect(
        clippy::cast_precision_loss,
        reason = "nesting depth is tiny; exact in f32"
    )]
    let indent = depth as f32 * 32.0;
    container(row![card, toggle].spacing(8).align_y(Alignment::Center))
        .width(Fill)
        .padding(Padding {
            left: indent,
            ..Padding::ZERO
        })
        .into()
}

fn skeleton_row() -> Element<'static, Message> {
    container(
        row![
            skeleton(48.0, 22.0),
            skeleton(Fill, 14.0),
            skeleton(80.0, 14.0)
        ]
        .spacing(14)
        .align_y(Alignment::Center),
    )
    .padding([18, 20])
    .width(Fill)
    .style(style::card)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rail_icon_is_centered_in_the_nav_button() {
        let button = RAIL_WIDTH - NAV_BAR_WIDTH - NAV_GAP;
        let icon_center = nav_icon_pad(RAIL_WIDTH) + NAV_ICON / 2.0;
        assert!((icon_center - button / 2.0).abs() <= 0.5);
    }

    #[test]
    fn expanded_icon_keeps_the_regular_padding() {
        assert_eq!(nav_icon_pad(SIDEBAR_WIDTH), NAV_PAD);
    }
}
