pub mod style;

use iced::theme::Palette;
use iced::{Color, Theme, color};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Light,
    Dark,
}

impl Mode {
    pub fn toggle(self) -> Self {
        match self {
            Mode::Light => Mode::Dark,
            Mode::Dark => Mode::Light,
        }
    }

    pub fn theme(self) -> Theme {
        let t = Tokens::for_mode(self);
        let name = match self {
            Mode::Light => "gbtracker-light",
            Mode::Dark => "gbtracker-dark",
        };
        Theme::custom(
            name,
            Palette {
                background: t.app_bg,
                text: t.text,
                primary: t.accent,
                success: t.success,
                warning: t.warning,
                danger: t.danger,
            },
        )
    }
}

/// Progress/status tone used by bars, percentages and status dots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Success,
    Warning,
    Danger,
}

impl Tone {
    pub fn for_ratio(ratio: f32) -> Self {
        if ratio >= 0.5 {
            Tone::Success
        } else if ratio >= 0.2 {
            Tone::Warning
        } else {
            Tone::Danger
        }
    }
}

/// Design tokens. Every style function reads colors from here, never hardcodes them.
#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    /// Window background (top bar sits directly on it).
    pub app_bg: Color,
    /// Rounded content panel below the top bar.
    pub panel: Color,
    /// Cards, active tab, active nav item.
    pub surface: Color,
    /// Neutral fills: secondary buttons, icon buttons, tab pills.
    pub fill: Color,
    pub fill_hover: Color,
    pub chip: Color,
    pub border: Color,
    pub text: Color,
    pub text_muted: Color,
    pub text_subtle: Color,
    pub accent: Color,
    pub accent_hover: Color,
    /// Accent at ~15% over `panel`: background of selected chips and pills.
    pub accent_soft: Color,
    pub on_accent: Color,
    pub success: Color,
    pub success_track: Color,
    pub success_soft: Color,
    pub warning: Color,
    pub warning_track: Color,
    pub warning_soft: Color,
    pub danger: Color,
    pub danger_track: Color,
    pub danger_soft: Color,
    pub shadow: Color,
}

impl Tokens {
    pub fn of(theme: &Theme) -> Self {
        if theme.extended_palette().is_dark {
            DARK
        } else {
            LIGHT
        }
    }

    pub fn for_mode(mode: Mode) -> Self {
        match mode {
            Mode::Light => LIGHT,
            Mode::Dark => DARK,
        }
    }

    pub fn tone(&self, tone: Tone) -> (Color, Color, Color) {
        match tone {
            Tone::Success => (self.success, self.success_soft, self.success_track),
            Tone::Warning => (self.warning, self.warning_soft, self.warning_track),
            Tone::Danger => (self.danger, self.danger_soft, self.danger_track),
        }
    }
}

pub const LIGHT: Tokens = Tokens {
    app_bg: color!(0xf4f3f3),
    panel: color!(0xeeeded),
    surface: color!(0xffffff),
    fill: color!(0xe7e6e6),
    fill_hover: color!(0xdedddd),
    chip: color!(0xf3f3f3),
    border: color!(0xc9c8c8),
    text: color!(0x1d1d1f),
    text_muted: color!(0x77767a),
    text_subtle: color!(0xa4a3a6),
    accent: color!(0x8839ef),
    accent_hover: color!(0x7428d6),
    accent_soft: color!(0xdfd2ed),
    on_accent: color!(0xffffff),
    success: color!(0x0f9f6e),
    success_track: color!(0xe3f3ec),
    success_soft: color!(0x6cc9a6),
    warning: color!(0xe8b500),
    warning_track: color!(0xfcefd6),
    warning_soft: color!(0xf3c45a),
    danger: color!(0xd9483b),
    danger_track: color!(0xfbe3e0),
    danger_soft: color!(0xe9958d),
    shadow: Color::from_rgba8(0, 0, 0, 0.06),
};

pub const DARK: Tokens = Tokens {
    app_bg: color!(0x111113),
    panel: color!(0x18181b),
    surface: color!(0x212125),
    fill: color!(0x2a2a2f),
    fill_hover: color!(0x34343a),
    chip: color!(0x2c2c31),
    border: color!(0x46464d),
    text: color!(0xededef),
    text_muted: color!(0x9a9aa2),
    text_subtle: color!(0x6b6b73),
    accent: color!(0xcba6f7),
    accent_hover: color!(0xd9bdfa),
    accent_soft: color!(0x332d3c),
    on_accent: color!(0x1e1e2e),
    success: color!(0x22c08a),
    success_track: color!(0x173a2e),
    success_soft: color!(0x4f9e81),
    warning: color!(0xf2c230),
    warning_track: color!(0x3d331a),
    warning_soft: color!(0xb3933a),
    danger: color!(0xf06a5d),
    danger_track: color!(0x43231f),
    danger_soft: color!(0xb3625a),
    shadow: Color::from_rgba8(0, 0, 0, 0.35),
};

#[cfg(test)]
mod tests {
    use super::*;

    fn linear(c: f32) -> f32 {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    fn luminance(c: Color) -> f32 {
        0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
    }

    fn contrast(a: Color, b: Color) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    // Unselected chips are transparent, so they read as the panel they sit on.
    #[test]
    fn selected_chip_is_distinct_from_unselected() {
        for (name, t) in [("light", LIGHT), ("dark", DARK)] {
            let unselected = t.panel;
            let best = contrast(t.accent_soft, unselected).max(contrast(t.accent, unselected));
            assert!(
                best >= 3.0,
                "{name}: selected vs unselected contrast {best:.2}"
            );
            let label = contrast(t.accent, t.accent_soft);
            assert!(label >= 3.0, "{name}: selected label contrast {label:.2}");
        }
    }
}
