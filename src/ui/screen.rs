use iced::widget::{button, column, container, row, scrollable, space, text};
use iced::{Alignment, Element, Fill, Padding};
use lucide_iced::icon as lu;

use super::components::{
    MEDIUM, chip, detail_row, dot, icon, icon_muted, icon_pill, labeled, primary_button,
    secondary_button, split_progress, top_pill, vdivider,
};
use crate::mock::{self, Epic};
use crate::theme::{Mode, Tone, style};
use crate::{App, Message, Section};

pub fn view(app: &App) -> Element<'_, Message> {
    let body = row![sidebar(app.section), content(app.section)]
        .spacing(32)
        .height(Fill);

    container(
        column![
            top_bar(app.mode),
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

fn top_bar(mode: Mode) -> Element<'static, Message> {
    let theme_icon = match mode {
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
            false,
            Message::Noop
        ),
        top_pill(
            labeled(lu::git_branch(), mock::WORKSPACE),
            true,
            Message::Noop
        ),
        icon_pill(lu::ellipsis(), Message::Noop),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let sync = container(
        row![
            text("Synced 2m ago").size(13).style(style::subtle),
            dot(Tone::Success, 18.0),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .height(40)
    .padding([0, 12])
    .center_y(40)
    .style(style::status_pill);

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

fn sidebar(active: Section) -> Element<'static, Message> {
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
            text(mock::WORKSPACE).size(21),
            text(mock::WORKSPACE_PATH).size(13).style(style::subtle),
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
