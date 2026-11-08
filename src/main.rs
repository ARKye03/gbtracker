mod mock;
mod theme;
mod ui;

use iced::widget::Svg;
use iced::{Element, Theme};
use lucide_iced::icon as lu;

use theme::Mode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    Overview,
    Bugs,
    #[default]
    Epics,
    Labels,
    Identities,
    Bridges,
    Analytics,
}

impl Section {
    pub const ALL: [Section; 7] = [
        Section::Overview,
        Section::Bugs,
        Section::Epics,
        Section::Labels,
        Section::Identities,
        Section::Bridges,
        Section::Analytics,
    ];

    pub fn meta(self) -> (Svg<'static>, &'static str) {
        match self {
            Section::Overview => (lu::house(), "Overview"),
            Section::Bugs => (lu::bug(), "Bugs"),
            Section::Epics => (lu::milestone(), "Epics"),
            Section::Labels => (lu::tag(), "Labels"),
            Section::Identities => (lu::users(), "Identities"),
            Section::Bridges => (lu::cable(), "Bridges"),
            Section::Analytics => (lu::chart_line(), "Analytics"),
        }
    }
}

#[derive(Default)]
pub struct App {
    mode: Mode,
    section: Section,
}

impl App {
    fn boot() -> Self {
        let mode = match std::env::var("GBTRACKER_THEME").as_deref() {
            Ok("dark") => Mode::Dark,
            _ => Mode::Light,
        };
        Self {
            mode,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    ToggleTheme,
    Navigate(Section),
    Noop,
}

impl App {
    fn update(&mut self, message: Message) {
        match message {
            Message::ToggleTheme => self.mode = self.mode.toggle(),
            Message::Navigate(s) => self.section = s,
            Message::Noop => {}
        }
    }

    fn view(&self) -> Element<'_, Message> {
        ui::screen::view(self)
    }

    fn theme(&self) -> Theme {
        self.mode.theme()
    }
}

fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title("gbtracker")
        .theme(App::theme)
        .window_size((1280.0, 860.0))
        .run()
}
