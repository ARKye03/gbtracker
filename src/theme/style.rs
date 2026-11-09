//! Widget style functions. Pass them to `.style(...)`; they read [`Tokens`] from the active theme.

use iced::widget::{button, container, progress_bar, scrollable, svg, text};
use iced::{Background, Border, Color, Shadow, Theme, Vector, border};

use super::{Tokens, Tone};

pub mod radius {
    pub const SM: f32 = 6.0;
    pub const MD: f32 = 8.0;
    pub const LG: f32 = 12.0;
    pub const XL: f32 = 18.0;
    pub const PILL: f32 = 999.0;
}

fn soft_shadow(t: &Tokens) -> Shadow {
    Shadow {
        color: t.shadow,
        offset: Vector::new(0.0, 1.0),
        blur_radius: 6.0,
    }
}

fn filled(bg: Color, fg: Color, r: f32) -> button::Style {
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: fg,
        border: border::rounded(r),
        ..button::Style::default()
    }
}

fn dimmed(style: button::Style, status: button::Status) -> button::Style {
    if status != button::Status::Disabled {
        return style;
    }
    button::Style {
        background: style.background.map(|b| b.scale_alpha(0.5)),
        text_color: style.text_color.scale_alpha(0.5),
        ..style
    }
}

// --- containers ---

pub fn app(theme: &Theme) -> container::Style {
    let t = Tokens::of(theme);
    container::Style {
        background: Some(t.app_bg.into()),
        text_color: Some(t.text),
        ..container::Style::default()
    }
}

pub fn panel(theme: &Theme) -> container::Style {
    let t = Tokens::of(theme);
    container::Style {
        background: Some(t.panel.into()),
        border: border::rounded(border::top(radius::XL)),
        ..container::Style::default()
    }
}

pub fn card(theme: &Theme) -> container::Style {
    let t = Tokens::of(theme);
    container::Style {
        background: Some(t.surface.into()),
        border: border::rounded(radius::XL),
        shadow: soft_shadow(&t),
        ..container::Style::default()
    }
}

pub fn chip(theme: &Theme) -> container::Style {
    let t = Tokens::of(theme);
    container::Style {
        background: Some(t.chip.into()),
        text_color: Some(t.text),
        border: border::rounded(radius::SM),
        ..container::Style::default()
    }
}

/// Floating tooltip surface; `alpha` drives its fade in and out.
pub fn tooltip(alpha: f32) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let t = Tokens::of(theme);
        container::Style {
            background: Some(t.surface.scale_alpha(alpha).into()),
            text_color: Some(t.text.scale_alpha(alpha)),
            border: Border {
                color: t.border.scale_alpha(alpha),
                width: 1.0,
                radius: radius::MD.into(),
            },
            shadow: Shadow {
                color: t.shadow.scale_alpha(alpha),
                offset: Vector::new(0.0, 2.0),
                blur_radius: 10.0,
            },
            ..container::Style::default()
        }
    }
}

/// Dims the page behind a modal sheet.
pub fn scrim(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.45).into()),
        ..container::Style::default()
    }
}

/// Floating menu surface for top-bar popovers.
pub fn popover(theme: &Theme) -> container::Style {
    let t = Tokens::of(theme);
    container::Style {
        background: Some(t.surface.into()),
        text_color: Some(t.text),
        border: Border {
            color: t.border,
            width: 1.0,
            radius: radius::LG.into(),
        },
        shadow: Shadow {
            color: t.shadow,
            offset: Vector::new(0.0, 6.0),
            blur_radius: 20.0,
        },
        ..container::Style::default()
    }
}

/// Unread count on the bell.
pub fn badge(theme: &Theme) -> container::Style {
    let t = Tokens::of(theme);
    container::Style {
        background: Some(t.danger.into()),
        text_color: Some(Color::WHITE),
        border: border::rounded(radius::PILL),
        ..container::Style::default()
    }
}

pub fn divider(theme: &Theme) -> container::Style {
    let t = Tokens::of(theme);
    container::Style {
        background: Some(t.fill.into()),
        ..container::Style::default()
    }
}

pub fn status_dot(tone: Tone) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let (c, _, _) = Tokens::of(theme).tone(tone);
        container::Style {
            background: Some(c.into()),
            border: border::rounded(radius::PILL),
            ..container::Style::default()
        }
    }
}

pub fn neutral_dot(theme: &Theme) -> container::Style {
    let t = Tokens::of(theme);
    container::Style {
        background: Some(t.text_subtle.into()),
        border: border::rounded(radius::PILL),
        ..container::Style::default()
    }
}

pub fn accent_dot(active: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let t = Tokens::of(theme);
        let c = if active { t.accent } else { t.text_subtle };
        container::Style {
            background: Some(c.into()),
            border: border::rounded(radius::PILL),
            ..container::Style::default()
        }
    }
}

/// Placeholder block shown while content is loading.
pub fn skeleton(theme: &Theme) -> container::Style {
    let t = Tokens::of(theme);
    container::Style {
        background: Some(t.fill.into()),
        border: border::rounded(radius::SM),
        ..container::Style::default()
    }
}

pub fn banner(tone: Tone) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let t = Tokens::of(theme);
        let (c, _, track) = t.tone(tone);
        container::Style {
            background: Some(track.into()),
            text_color: Some(c),
            border: Border {
                color: c,
                width: 1.0,
                radius: radius::MD.into(),
            },
            ..container::Style::default()
        }
    }
}

// --- buttons ---

pub fn primary(theme: &Theme, status: button::Status) -> button::Style {
    let t = Tokens::of(theme);
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => t.accent_hover,
        _ => t.accent,
    };
    dimmed(filled(bg, t.on_accent, radius::MD), status)
}

pub fn secondary(theme: &Theme, status: button::Status) -> button::Style {
    let t = Tokens::of(theme);
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => t.fill_hover,
        _ => t.fill,
    };
    dimmed(filled(bg, t.text, radius::MD), status)
}

/// Top-bar pill (Search, New Bug, inactive tabs, icon buttons).
pub fn pill(theme: &Theme, status: button::Status) -> button::Style {
    let t = Tokens::of(theme);
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => t.fill_hover,
        _ => t.fill.scale_alpha(0.6),
    };
    dimmed(filled(bg, t.text, radius::LG), status)
}

/// Selected filter chip / active top pill: tinted fill, accent border and text.
pub fn chip_selected(theme: &Theme, _status: button::Status) -> button::Style {
    let t = Tokens::of(theme);
    button::Style {
        border: Border {
            color: t.accent,
            width: 1.0,
            radius: radius::LG.into(),
        },
        ..filled(t.accent_soft, t.accent, radius::LG)
    }
}

pub fn chip_unselected(theme: &Theme, status: button::Status) -> button::Style {
    let t = Tokens::of(theme);
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => t.fill.scale_alpha(0.5),
        _ => Color::TRANSPARENT,
    };
    button::Style {
        border: Border {
            color: t.border,
            width: 1.0,
            radius: radius::LG.into(),
        },
        ..filled(bg, t.text_muted, radius::LG)
    }
}

pub fn nav_item(theme: &Theme, status: button::Status) -> button::Style {
    let t = Tokens::of(theme);
    match status {
        button::Status::Hovered | button::Status::Pressed => {
            filled(t.surface.scale_alpha(0.5), t.text, radius::LG)
        }
        _ => button::Style {
            text_color: t.text,
            ..button::Style::default()
        },
    }
}

pub fn nav_item_active(theme: &Theme, _status: button::Status) -> button::Style {
    let t = Tokens::of(theme);
    button::Style {
        shadow: soft_shadow(&t),
        ..filled(t.surface, t.text, radius::LG)
    }
}

/// Left-edge marker for the active sidebar item.
pub fn nav_bar(active: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let t = Tokens::of(theme);
        container::Style {
            background: active.then(|| t.accent.into()),
            border: border::rounded(radius::PILL),
            ..container::Style::default()
        }
    }
}

/// Transparent text-like button (header search, card "more" uses `icon_square`).
pub fn ghost(theme: &Theme, status: button::Status) -> button::Style {
    let t = Tokens::of(theme);
    let fg = match status {
        button::Status::Hovered | button::Status::Pressed => t.text,
        _ => t.text_muted,
    };
    button::Style {
        text_color: fg,
        ..button::Style::default()
    }
}

/// A whole card that can be pressed; the open one gets an accent outline.
pub fn card_button(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let t = Tokens::of(theme);
        let bg = match status {
            button::Status::Hovered | button::Status::Pressed => t.fill.scale_alpha(0.5),
            _ => t.surface,
        };
        let outline = if selected {
            Border {
                color: t.accent,
                width: 1.5,
                radius: radius::XL.into(),
            }
        } else {
            border::rounded(radius::XL)
        };
        button::Style {
            border: outline,
            shadow: soft_shadow(&t),
            ..filled(bg, t.text, radius::XL)
        }
    }
}

/// Transparent row that lights up on hover.
pub fn list_item(theme: &Theme, status: button::Status) -> button::Style {
    let t = Tokens::of(theme);
    match status {
        button::Status::Hovered | button::Status::Pressed => {
            filled(t.fill.scale_alpha(0.5), t.text, radius::MD)
        }
        _ => button::Style {
            text_color: t.text,
            ..button::Style::default()
        },
    }
}

pub fn icon_square(theme: &Theme, status: button::Status) -> button::Style {
    let t = Tokens::of(theme);
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => t.fill,
        _ => t.chip,
    };
    filled(bg, t.text, radius::MD)
}

/// Top-bar status pill (replaces the proposal's "Talk to AI" box with sync status).
pub fn status_pill(theme: &Theme) -> container::Style {
    let t = Tokens::of(theme);
    container::Style {
        background: Some(t.fill.scale_alpha(0.6).into()),
        border: border::rounded(radius::LG),
        ..container::Style::default()
    }
}

// --- progress ---

pub fn progress(tone: Tone) -> impl Fn(&Theme) -> progress_bar::Style {
    move |theme| {
        let (bar, _, track) = Tokens::of(theme).tone(tone);
        progress_bar::Style {
            background: track.into(),
            bar: bar.into(),
            border: border::rounded(radius::PILL),
        }
    }
}

// --- text & icons ---

pub fn muted(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(Tokens::of(theme).text_muted),
    }
}

pub fn subtle(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(Tokens::of(theme).text_subtle),
    }
}

pub fn faded(alpha: f32) -> impl Fn(&Theme) -> text::Style {
    move |theme| text::Style {
        color: Some(Tokens::of(theme).text.scale_alpha(alpha)),
    }
}

pub fn faded_subtle(alpha: f32) -> impl Fn(&Theme) -> text::Style {
    move |theme| text::Style {
        color: Some(Tokens::of(theme).text_subtle.scale_alpha(alpha)),
    }
}

pub fn tone_text(tone: Tone, soft: bool) -> impl Fn(&Theme) -> text::Style {
    move |theme| {
        let (strong, light, _) = Tokens::of(theme).tone(tone);
        text::Style {
            color: Some(if soft { light } else { strong }),
        }
    }
}

pub fn icon(theme: &Theme, _status: svg::Status) -> svg::Style {
    svg::Style {
        color: Some(Tokens::of(theme).text),
    }
}

pub fn icon_accent(theme: &Theme, _status: svg::Status) -> svg::Style {
    svg::Style {
        color: Some(Tokens::of(theme).accent),
    }
}

pub fn icon_muted(theme: &Theme, _status: svg::Status) -> svg::Style {
    svg::Style {
        color: Some(Tokens::of(theme).text_muted),
    }
}

// --- scrollable ---

pub fn scroll(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let t = Tokens::of(theme);
    let mut s = scrollable::default(theme, status);
    for rail in [&mut s.vertical_rail, &mut s.horizontal_rail] {
        rail.background = None;
        rail.border = Border::default();
        rail.scroller.border = border::rounded(radius::PILL);
        rail.scroller.background = t.fill_hover.into();
    }
    s
}
