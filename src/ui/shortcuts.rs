//! Keyboard shortcut help overlay, rendered from the same table that resolves key presses.

use iced::widget::{
    center, column, container, mouse_area, opaque, row, scrollable, space, stack, text,
};
use iced::{Alignment, Element, Fill};
use lucide_iced::icon as lu;

use super::components::{MEDIUM, chip, icon_square_button};
use crate::app::{App, Message};
use crate::keymap::{self, Group};
use crate::theme::style;

const WIDTH: f32 = 520.0;

pub fn overlay<'a>(app: &'a App, base: Element<'a, Message>) -> Element<'a, Message> {
    if !app.help {
        return base;
    }
    let scrim = container(center(opaque(card())))
        .width(Fill)
        .height(Fill)
        .style(style::scrim);
    stack![
        base,
        opaque(mouse_area(scrim).on_press(Message::ToggleHelp))
    ]
    .into()
}

fn card<'a>() -> Element<'a, Message> {
    let header = row![
        text("Keyboard shortcuts").size(20).font(MEDIUM),
        space::horizontal(),
        icon_square_button(lu::x(), 14.0, 32.0, "Close", Some(Message::ToggleHelp)),
    ]
    .align_y(Alignment::Center);

    let groups = column(Group::ALL.iter().map(|&group| {
        let rows = column(keymap::help_rows(group).into_iter().map(|(label, chords)| {
            let keys = row(chords.into_iter().map(chip)).spacing(6);
            row![text(label).size(14), space::horizontal(), keys]
                .align_y(Alignment::Center)
                .into()
        }))
        .spacing(10);
        column![text(group.title()).size(13).style(style::subtle), rows]
            .spacing(12)
            .into()
    }))
    .spacing(24);

    container(
        column![
            header,
            scrollable(container(groups).padding([0, 12])).style(style::scroll)
        ]
        .spacing(20),
    )
    .padding(24)
    .width(WIDTH)
    .max_height(640)
    .style(style::card)
    .into()
}
