use iced::widget::{button, column, container, row, scrollable, space, text};
use iced::{Alignment, Element, Fill, Padding};
use lucide_iced::icon as lu;

use super::components::{
    MEDIUM, chip, detail_row, dot, icon, icon_muted, icon_pill, labeled, primary_button,
    secondary_button, skeleton, spinner_dot, split_progress, top_pill, vdivider,
};
use crate::Section;
use crate::app::{App, LoadState, Message, RepoEntry, ago};
use crate::mock::{self, Epic};
use crate::theme::{Mode, Tone, style};
use gbtracker::workspace::{Root, RootKind};

pub fn view(app: &App) -> Element<'_, Message> {
    let page = match app.section {
        Section::Repositories => repositories(app),
        other => content(other),
    };
    let body = row![sidebar(app), page].spacing(32).height(Fill);

    container(
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
    .style(style::app)
    .into()
}

fn top_bar(app: &App) -> Element<'_, Message> {
    let theme_icon = match app.mode {
        Mode::Light => lu::moon(),
        Mode::Dark => lu::sun(),
    };

    let left = row![
        icon_pill(lu::panel_left(), Message::Noop),
        top_pill(labeled(lu::search(), "Search"), false, Message::Noop),
        top_pill(labeled(lu::plus(), "New Bug"), false, Message::Noop),
        vdivider(),
        top_pill(
            labeled(lu::folder_git_2(), "Repositories"),
            app.section == Section::Repositories,
            Message::Navigate(Section::Repositories)
        ),
        top_pill(
            row![
                icon(lu::git_branch(), 16.0),
                text(workspace_summary(app)).size(15)
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            false,
            Message::Noop
        ),
        icon_pill(lu::ellipsis(), Message::Noop),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let sync = sync_pill(app);

    let avatar = container(text(mock::IDENTITY_INITIALS).size(13).font(MEDIUM))
        .center(32)
        .style(style::avatar);

    let right = row![
        sync,
        icon_pill(theme_icon, Message::ToggleTheme),
        icon_pill(lu::message_square(), Message::Noop),
        icon_pill(lu::bell(), Message::Noop),
        container(avatar).center(40),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    row![left, space::horizontal(), right]
        .align_y(Alignment::Center)
        .into()
}

fn workspace_summary(app: &App) -> String {
    format!("{} repositories", app.repos.len())
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
    let active = app.section;
    let items = Section::ALL.iter().map(|&s| {
        let (svg, label) = s.meta();
        button(labeled(svg, label))
            .width(Fill)
            .padding([12, 16])
            .style(if s == active {
                style::nav_item_active
            } else {
                style::nav_item
            })
            .on_press(Message::Navigate(s))
            .into()
    });

    column![
        column![
            text("Workspace").size(21),
            text!(
                "{} projects · {} repositories",
                app.workspace.roots.len(),
                app.repos.len()
            )
            .size(13)
            .style(style::subtle),
        ]
        .spacing(4)
        .padding([0, 0]),
        column(items).spacing(6),
    ]
    .spacing(36)
    .width(276)
    .into()
}

fn content(section: Section) -> Element<'static, Message> {
    let (_, title) = section.meta();

    let header = row![
        text(title).size(21),
        button(
            row![icon_muted(lu::plus(), 15.0), text("Add Filters").size(15)]
                .spacing(6)
                .align_y(Alignment::Center)
        )
        .padding([8, 12])
        .style(style::outline)
        .on_press(Message::Noop),
        button(
            row![icon_muted(lu::search(), 17.0), text("Search").size(15)]
                .spacing(8)
                .align_y(Alignment::Center)
        )
        .padding([8, 6])
        .style(style::ghost)
        .on_press(Message::Noop),
        space::horizontal(),
        secondary_button("Export", Message::Noop),
        primary_button("New Epic", Message::Noop),
    ]
    .spacing(18)
    .align_y(Alignment::Center);

    let body: Element<'_, Message> = match section {
        Section::Epics => column(mock::EPICS.iter().map(epic_card))
            .spacing(16)
            .padding(Padding {
                bottom: 24.0,
                right: 10.0,
                ..Padding::ZERO
            })
            .into(),
        _ => empty_state(title),
    };

    column![header, scrollable(body).height(Fill).style(style::scroll),]
        .spacing(28)
        .width(Fill)
        .into()
}

fn epic_card(epic: &'static Epic) -> Element<'static, Message> {
    let heading = row![
        column![
            text!("{} bugs", epic.total).size(29),
            text(epic.title).size(18),
        ]
        .spacing(8),
        space::horizontal(),
        button(container(icon(lu::ellipsis(), 16.0)).center(Fill))
            .width(36)
            .height(36)
            .padding(0)
            .style(style::icon_square)
            .on_press(Message::Noop),
    ];

    let details = column![
        detail_row("Closed", format!("{} of {}", epic.closed, epic.total)),
        detail_row("Repository", epic.repo),
        detail_row("Opened", epic.opened),
        detail_row("Last Activity", epic.last_activity),
    ]
    .spacing(18);

    let labels = row(epic.labels.iter().map(|l| chip(l))).spacing(8);

    container(
        column![
            heading,
            split_progress(epic.ratio()),
            details,
            row![
                labels,
                space::horizontal(),
                text(epic.id).size(13).style(style::subtle)
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(28),
    )
    .padding(28)
    .width(Fill)
    .style(style::card)
    .into()
}

fn empty_state(title: &'static str) -> Element<'static, Message> {
    container(
        column![
            text(title).size(18),
            text("Mock screen — not wired yet.")
                .size(14)
                .style(style::muted),
        ]
        .spacing(6)
        .align_x(Alignment::Center),
    )
    .padding(48)
    .center_x(Fill)
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

    let mut body = column![].spacing(16).padding(Padding {
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
            body = body.extend((0..3).map(|_| skeleton_card()));
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
        button(container(icon(lu::x(), 14.0)).center(Fill))
            .width(32)
            .height(32)
            .padding(0)
            .style(style::icon_square)
            .on_press(Message::RemoveRoot(index)),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

fn status_label(state: &LoadState) -> (&'static str, Option<Tone>) {
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

fn status_badge(state: &LoadState, spin: usize) -> Element<'static, Message> {
    let (label, tone) = status_label(state);
    let indicator: Element<'static, Message> = match (state.is_busy(), tone) {
        (true, _) => spinner_dot(spin),
        (false, Some(tone)) => dot(tone, 10.0),
        (false, None) => container(space())
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

fn repo_card(entry: &RepoEntry, spin: usize) -> Element<'_, Message> {
    let heading = row![
        column![
            text(&entry.info.name).size(18),
            text(entry.info.root.display().to_string())
                .size(13)
                .style(style::subtle),
        ]
        .spacing(4),
        space::horizontal(),
        status_badge(&entry.state, spin),
    ]
    .align_y(Alignment::Center);

    let detail: Element<'_, Message> = match &entry.state {
        LoadState::NoGitBug => text("git-bug is not initialised in this repository.")
            .size(14)
            .style(style::muted)
            .into(),
        LoadState::Error(msg) => text(msg)
            .size(14)
            .style(style::tone_text(Tone::Danger, false))
            .into(),
        LoadState::Loading if entry.fingerprint.is_none() => {
            column![skeleton(220.0, 14.0), skeleton(140.0, 14.0)]
                .spacing(10)
                .into()
        }
        state => {
            let mut rows = column![
                detail_row("Open", entry.open_count().to_string()),
                detail_row("Closed", entry.closed_count().to_string()),
            ]
            .spacing(12);
            if *state == LoadState::Locked {
                rows = rows.push(
                    text("Locked by another git-bug process; retrying.")
                        .size(13)
                        .style(style::tone_text(Tone::Warning, false)),
                );
            }
            rows.into()
        }
    };

    container(column![heading, detail].spacing(20))
        .padding(28)
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
