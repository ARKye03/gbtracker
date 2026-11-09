//! Reusable building blocks matching docs/proposal.jpg. Real screens should compose these, not restyle widgets ad hoc.

use iced::widget::{Svg, button, column, container, progress_bar, responsive, row, space, text};
use iced::{Alignment, Element, Fill, Font, Length, Shrink, font};
use lucide_iced::icon as lu;

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
            style::chip_selected
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
    primary_button_maybe(label, Some(on_press))
}

pub fn primary_button_maybe<'a, M: Clone + 'a>(
    label: &'a str,
    on_press: Option<M>,
) -> Element<'a, M> {
    button(text(label).size(15).font(MEDIUM))
        .padding([11, 18])
        .style(style::primary)
        .on_press_maybe(on_press)
        .into()
}

pub fn secondary_button<'a, M: Clone + 'a>(label: &'a str, on_press: M) -> Element<'a, M> {
    choice_button(label, false, Some(on_press))
}

/// Primary when `active`, secondary otherwise; `None` renders it disabled.
pub fn choice_button<'a, M: Clone + 'a>(
    label: &'a str,
    active: bool,
    on_press: Option<M>,
) -> Element<'a, M> {
    button(text(label).size(15))
        .padding([11, 18])
        .style(if active {
            style::primary
        } else {
            style::secondary
        })
        .on_press_maybe(on_press)
        .into()
}

pub fn chip<'a, M: 'a>(label: impl text::IntoFragment<'a>) -> Element<'a, M> {
    container(text(label).size(13))
        .padding([5, 8])
        .style(style::chip)
        .into()
}

/// Pressable filter chip. Selected chips get an accent fill and border; in `multi` groups they also show a check.
pub fn toggle_chip<'a, M: Clone + 'a>(
    label: &'a str,
    selected: bool,
    multi: bool,
    on_press: M,
) -> Element<'a, M> {
    let label = text(label).size(13);
    let content: Element<'a, M> = if selected && multi {
        row![
            lu::check().width(13).height(13).style(style::icon_accent),
            label
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    } else {
        label.into()
    };
    button(content)
        .padding([6, 12])
        .style(if selected {
            style::chip_selected
        } else {
            style::chip_unselected
        })
        .on_press(on_press)
        .into()
}

/// Responsive grid: as many `min_width` columns as fit, cards share the row evenly and the last row is padded to stay aligned.
pub fn card_grid<'a, T, M>(
    items: Vec<T>,
    min_width: f32,
    gap: f32,
    render: impl Fn(T) -> Element<'a, M> + 'a,
) -> Element<'a, M>
where
    T: Clone + 'a,
    M: 'a,
{
    responsive(move |size| {
        let cols = grid_columns(size.width, min_width, gap);
        column(items.chunks(cols).map(|chunk| {
            let cells = chunk.iter().cloned().map(&render);
            let padding = (0..cols - chunk.len()).map(|_| space::horizontal().into());
            row(cells.chain(padding)).spacing(gap).width(Fill).into()
        }))
        .spacing(gap)
        .width(Fill)
        .into()
    })
    .height(Shrink)
    .into()
}

fn grid_columns(width: f32, min_width: f32, gap: f32) -> usize {
    (((width + gap) / (min_width + gap)).floor() as usize).max(1)
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

#[cfg(test)]
mod tests {
    use super::grid_columns;

    #[test]
    fn columns_scale_with_width() {
        assert_eq!(grid_columns(100.0, 380.0, 16.0), 1);
        assert_eq!(grid_columns(1280.0, 380.0, 16.0), 3);
        assert_eq!(grid_columns(2560.0, 380.0, 16.0), 6);
    }
}
