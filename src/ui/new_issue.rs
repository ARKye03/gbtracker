//! New issue dialog: repo, kind, title, body and parent epic. Locked while the create runs.

use std::fmt;
use std::path::PathBuf;

use iced::widget::{
    button, center, column, container, mouse_area, opaque, pick_list, row, space, stack, text,
    text_editor, text_input,
};
use iced::{Alignment, Element, Fill};
use lucide_iced::icon as lu;

use super::components::{
    MEDIUM, choice_button, icon_square_button, primary_button_maybe, spinner_dot, toggle_chip,
};
use crate::app::{App, Message, NEW_ISSUE_TITLE_ID, NewIssueForm};
use crate::theme::{Tone, style};
use gbtracker::gitbug::NewKind;

const WIDTH: f32 = 560.0;

#[derive(Clone, PartialEq)]
struct RepoChoice {
    root: PathBuf,
    name: String,
}

impl fmt::Display for RepoChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

#[derive(Clone, PartialEq)]
struct EpicChoice {
    id: Option<String>,
    label: String,
}

impl fmt::Display for EpicChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

fn kind_title(kind: NewKind) -> &'static str {
    match kind {
        NewKind::Bug => "Bug",
        NewKind::Task => "Task",
        NewKind::Epic => "Epic",
    }
}

pub fn overlay<'a>(app: &'a App, base: Element<'a, Message>) -> Element<'a, Message> {
    let Some(form) = &app.new_issue else {
        return base;
    };
    let scrim = container(center(opaque(card(app, form))))
        .width(Fill)
        .height(Fill)
        .style(style::scrim);
    stack![
        base,
        opaque(mouse_area(scrim).on_press(Message::DismissNewIssue))
    ]
    .into()
}

fn field<'a>(label: &'a str, input: Element<'a, Message>) -> Element<'a, Message> {
    column![text(label).size(13).style(style::subtle), input]
        .spacing(6)
        .into()
}

fn card<'a>(app: &'a App, form: &'a NewIssueForm) -> Element<'a, Message> {
    let header = row![
        text("New issue").size(20).font(MEDIUM),
        space::horizontal(),
        icon_square_button(
            lu::x(),
            14.0,
            32.0,
            "Close",
            (!form.submitting).then_some(Message::DismissNewIssue)
        ),
    ]
    .align_y(Alignment::Center);

    let repos: Vec<RepoChoice> = app
        .repos
        .iter()
        .filter(|e| e.info.has_git_bug)
        .map(|e| RepoChoice {
            root: e.info.root.clone(),
            name: e.info.name.clone(),
        })
        .collect();
    let repo_selected = repos.iter().find(|c| c.root == form.root).cloned();
    let repo = pick_list(repos, repo_selected, |c: RepoChoice| {
        Message::NewIssueRepo(c.root)
    })
    .text_size(14)
    .padding([7, 12])
    .width(Fill);

    let kinds = row(NewKind::ALL.iter().map(|&kind| {
        toggle_chip(
            kind_title(kind),
            form.kind == kind,
            false,
            Message::NewIssueKind(kind),
        )
    }))
    .spacing(6);

    let mut title = text_input("Title", &form.title)
        .id(NEW_ISSUE_TITLE_ID)
        .size(14)
        .padding([8, 12]);
    if !form.submitting {
        title = title
            .on_input(Message::NewIssueTitle)
            .on_submit(Message::SubmitNewIssue);
    }

    let mut body = text_editor(&form.body)
        .placeholder("Describe it (markdown)…")
        .height(160)
        .padding(10);
    if !form.submitting {
        body = body.on_action(Message::NewIssueBody);
    }

    let mut fields = column![
        field("Repository", repo.into()),
        field("Kind", kinds.into()),
        field("Title", title.into()),
        field("Description", body.into()),
    ]
    .spacing(14);

    if form.kind != NewKind::Epic {
        let none = EpicChoice {
            id: None,
            label: "No parent epic".to_owned(),
        };
        let mut choices = vec![none.clone()];
        choices.extend(app.open_epics(&form.root).into_iter().map(|i| EpicChoice {
            id: Some(i.id.clone()),
            label: format!("{}  {}", i.human_id, i.title),
        }));
        let selected = choices
            .iter()
            .find(|c| c.id == form.epic)
            .cloned()
            .unwrap_or(none);
        let epic = pick_list(choices, Some(selected), |c: EpicChoice| {
            Message::NewIssueEpic(c.id)
        })
        .text_size(14)
        .padding([7, 12])
        .width(Fill);
        fields = fields.push(field("Parent epic", epic.into()));
    }

    let create: Element<'a, Message> = if form.submitting {
        button(
            row![
                spinner_dot(app.spin),
                text("Creating…").size(15).font(MEDIUM)
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding([11, 18])
        .style(style::primary)
        .into()
    } else {
        primary_button_maybe(
            "Create",
            (!form.title.trim().is_empty()).then_some(Message::SubmitNewIssue),
        )
    };

    let mut footer = row![].spacing(8).align_y(Alignment::Center);
    if let Some(error) = &form.error {
        footer = footer.push(
            text(error.as_str())
                .size(13)
                .style(style::tone_text(Tone::Danger, false))
                .width(Fill),
        );
    } else {
        footer = footer.push(space::horizontal());
    }
    footer = footer
        .push(choice_button(
            "Cancel",
            false,
            (!form.submitting).then_some(Message::DismissNewIssue),
        ))
        .push(create);

    container(column![header, fields, footer].spacing(20))
        .padding(24)
        .width(WIDTH)
        .style(style::card)
        .into()
}
