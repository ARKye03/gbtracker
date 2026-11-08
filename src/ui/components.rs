//! Reusable building blocks matching docs/proposal.jpg. Real screens should compose these, not restyle widgets ad hoc.

use iced::widget::{Svg, button, column, container, progress_bar, row, space, text};
use iced::{Alignment, Element, Fill, Font, Length, font};

use crate::theme::{Tone, style};

pub const MEDIUM: Font = Font {
    weight: font::Weight::Medium,
    ..Font::DEFAULT
};

pub fn icon<'a>(svg: Svg<'a>, size: f32) -> Svg<'a> {
    svg.width(size).height(size).style(style::icon)
}

pub fn icon_muted<'a>(svg: Svg<'a>, size: f32) -> Svg<'a> {
    svg.width(size).height(size).style(style::icon_muted)
}

/// Icon + label laid out for buttons/nav items.
pub fn labeled<'a, M: 'a>(svg: Svg<'a>, label: &'a str) -> Element<'a, M> {
    row![icon(svg, 16.0), text(label).size(15)]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
}

pub fn top_pill<'a, M: Clone + 'a>(
    content: impl Into<Element<'a, M>>,
    active: bool,
    on_press: M,
) -> Element<'a, M> {
    button(container(content).center_y(Fill))
        .height(40)
        .padding([0, 14])
        .style(if active {
            style::pill_active
        } else {
            style::pill
        })
        .on_press(on_press)
        .into()
}

pub fn icon_pill<'a, M: Clone + 'a>(svg: Svg<'a>, on_press: M) -> Element<'a, M> {
    button(container(icon(svg, 17.0)).center(Fill))
        .width(40)
        .height(40)
        .padding(0)
        .style(style::pill)
        .on_press(on_press)
        .into()
}

pub fn primary_button<'a, M: Clone + 'a>(label: &'a str, on_press: M) -> Element<'a, M> {
    button(text(label).size(15).font(MEDIUM))
        .padding([11, 18])
        .style(style::primary)
        .on_press(on_press)
        .into()
}

pub fn secondary_button<'a, M: Clone + 'a>(label: &'a str, on_press: M) -> Element<'a, M> {
    button(text(label).size(15))
        .padding([11, 18])
        .style(style::secondary)
        .on_press(on_press)
        .into()
}

pub fn chip<'a, M: 'a>(label: impl text::IntoFragment<'a>) -> Element<'a, M> {
    container(text(label).size(13))
        .padding([5, 8])
        .style(style::chip)
        .into()
}

/// Pressable chip for filters; the active one is raised like the active tab.
pub fn toggle_chip<'a, M: Clone + 'a>(label: &'a str, active: bool, on_press: M) -> Element<'a, M> {
    button(text(label).size(13))
        .padding([6, 12])
        .style(if active {
            style::pill_active
        } else {
            style::pill
        })
        .on_press(on_press)
        .into()
}

pub fn dot<'a, M: 'a>(tone: Tone, size: f32) -> Element<'a, M> {
    container(space())
        .width(size)
        .height(size)
        .style(style::status_dot(tone))
        .into()
}

/// Three dots with one lit; advance `phase` on a timer to animate.
pub fn spinner_dot<'a, M: 'a>(phase: usize) -> Element<'a, M> {
    row((0..3).map(|i| {
        container(space())
            .width(5)
            .height(5)
            .style(style::accent_dot(i == phase % 3))
            .into()
    }))
    .spacing(3)
    .align_y(Alignment::Center)
    .into()
}

/// Rounded placeholder block for content that has not loaded yet.
pub fn skeleton<'a, M: 'a>(width: impl Into<Length>, height: f32) -> Element<'a, M> {
    container(space())
        .width(width)
        .height(height)
        .style(style::skeleton)
        .into()
}

/// Label on the left, value on the right (card detail rows).
pub fn detail_row<'a, M: 'a>(label: &'a str, value: impl text::IntoFragment<'a>) -> Element<'a, M> {
    row![
        text(label).size(14).style(style::muted),
        space::horizontal(),
        text(value).size(14),
    ]
    .into()
}

/// Thin bar with done% on the left and remaining% on the right, colored by tone.
pub fn split_progress<'a, M: 'a>(ratio: f32) -> Element<'a, M> {
    let tone = Tone::for_ratio(ratio);
    let done = (ratio * 100.0).round() as u32;
    column![
        progress_bar(0.0..=1.0, ratio)
            .girth(7)
            .length(Fill)
            .style(style::progress(tone)),
        row![
            text!("{done}%")
                .size(13)
                .font(MEDIUM)
                .style(style::tone_text(tone, false)),
            space::horizontal(),
            text!("{}%", 100 - done)
                .size(13)
                .style(style::tone_text(tone, true)),
        ],
    ]
    .spacing(10)
    .into()
}

pub fn vdivider<'a, M: 'a>() -> Element<'a, M> {
    container(space())
        .width(1)
        .height(Length::Fixed(24.0))
        .style(style::divider)
        .into()
}
