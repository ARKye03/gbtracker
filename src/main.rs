mod app;
mod mock;
mod theme;
mod ui;

use iced::widget::Svg;
use lucide_iced::icon as lu;

use app::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Repositories,
    Overview,
    Issues,
    Epics,
    Labels,
    Identities,
    Bridges,
    Analytics,
}

impl Section {
    pub const ALL: [Section; 8] = [
        Section::Repositories,
        Section::Overview,
        Section::Issues,
        Section::Epics,
        Section::Labels,
        Section::Identities,
        Section::Bridges,
        Section::Analytics,
    ];

    pub fn meta(self) -> (Svg<'static>, &'static str) {
        match self {
            Section::Repositories => (lu::folder_git_2(), "Repositories"),
            Section::Overview => (lu::house(), "Overview"),
            Section::Issues => (lu::bug(), "Issues"),
            Section::Epics => (lu::milestone(), "Epics"),
            Section::Labels => (lu::tag(), "Labels"),
            Section::Identities => (lu::users(), "Identities"),
            Section::Bridges => (lu::cable(), "Bridges"),
            Section::Analytics => (lu::chart_line(), "Analytics"),
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
