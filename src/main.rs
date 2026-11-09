mod app;
mod keymap;
mod theme;
mod ui;

use iced::widget::Svg;
use lucide_iced::icon as lu;

use app::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Overview,
    Issues,
    Epics,
    Repositories,
}

impl Section {
    pub const ALL: [Section; 4] = [
        Section::Overview,
        Section::Issues,
        Section::Epics,
        Section::Repositories,
    ];

    pub fn meta(self) -> (Svg<'static>, &'static str) {
        match self {
            Section::Overview => (lu::house(), "Overview"),
            Section::Issues => (lu::bug(), "Issues"),
            Section::Epics => (lu::milestone(), "Epics"),
            Section::Repositories => (lu::folder_git_2(), "Repositories"),
        }
    }
}

fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title("gbtracker")
        .theme(App::theme)
        .subscription(App::subscription)
        .window_size((1280.0, 860.0))
        .run()
}
