use iced::widget::{
    button, column, container, markdown, row, scrollable, space, text, text_editor,
};
use iced::{Alignment, Element, Fill, Padding};
use lucide_iced::icon as lu;

use super::components::{
    MEDIUM, chip, choice_button, detail_row, icon, primary_button_maybe, secondary_button,
    skeleton, spinner_dot,
};
use super::screen::{kind_label, layer_label, state_badge};
use crate::app::{App, DetailPane, DetailState, Message};
use crate::theme::{Tone, style};
use gbtracker::epics;
use gbtracker::gitbug::{Issue, Kind, State, StateTarget, Status, WriteOp};

const WIDTH: f32 = 480.0;

pub fn view(app: &App) -> Option<Element<'_, Message>> {
    let pane = app.detail.as_ref()?;
    let issue = match &pane.state {
        DetailState::Ready(loaded) => &loaded.detail.issue,
        _ => &pane.summary,
    };

    let header = row![
        chip(kind_label(issue.kind.as_ref())),
        text(issue.human_id.as_str()).size(13).style(style::subtle),
        space::horizontal(),
        button(container(icon(lu::x(), 14.0)).center(Fill))
            .width(32)
            .height(32)
            .padding(0)
            .style(style::icon_square)
            .on_press(Message::CloseDetail),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let mut content = column![
        text(issue.title.as_str()).size(20).font(MEDIUM),
        state_badge(issue),
        summary(app, pane, issue),
        labels(issue),
        actions(app, pane, issue),
        write_status(app, pane, issue),
    ]
    .spacing(20)
    .padding(Padding {
        right: 12.0,
        bottom: 24.0,
        ..Padding::ZERO
    });
    if let Some(relations) = relations(app, pane, issue) {
        content = content.push(relations);
    }
    content = content.push(body(app, pane));

    let card = container(
        column![
            header,
            scrollable(content).height(Fill).style(style::scroll)
        ]
        .spacing(18),
    )
    .padding(24)
    .width(WIDTH)
    .height(Fill)
    .style(style::card);

    Some(
        container(card)
            .padding(Padding {
                bottom: 24.0,
                ..Padding::ZERO
            })
            .into(),
    )
}

fn summary<'a>(app: &'a App, pane: &'a DetailPane, issue: &'a Issue) -> Element<'a, Message> {
    let repo = app
        .repos
        .iter()
        .find(|e| e.info.root == pane.root)
        .map_or_else(|| pane.root.display().to_string(), |e| e.info.name.clone());
    column![
        text(issue.id.as_str()).size(12).style(style::subtle),
        column![
            detail_row("Repository", repo),
            detail_row("Author", issue.author.as_str()),
            detail_row("Created", epics::format_date(&issue.created)),
            detail_row("Edited", epics::format_date(&issue.edited)),
            detail_row(
                "Comments",
                issue.comment_count.saturating_sub(1).to_string()
            ),
        ]
        .spacing(10),
    ]
    .spacing(14)
    .into()
}

fn labels(issue: &Issue) -> Element<'_, Message> {
    row(issue
        .labels
        .iter()
        .filter(|l| super::is_user_label(l))
        .map(|l| chip(l.as_str())))
    .spacing(8)
    .wrap()
    .vertical_spacing(8)
    .into()
}

fn actions<'a>(app: &'a App, pane: &'a DetailPane, issue: &'a Issue) -> Element<'a, Message> {
    let idle = !app.is_pending(&pane.root, &issue.id);
    let write = |op: WriteOp| {
        idle.then(|| Message::Write {
            root: pane.root.clone(),
            id: issue.id.clone(),
            op,
        })
    };
    let copy = row![
        secondary_button("Copy id", Message::Copy(issue.id.clone())),
        secondary_button(
            "Copy /roadmap:impl",
            Message::Copy(format!("/roadmap:impl {}", issue.id))
        ),
    ]
    .spacing(8)
    .wrap()
    .vertical_spacing(8);

    let changes = if issue.status == Status::Closed {
        row![choice_button("Reopen", false, write(WriteOp::Reopen))]
    } else {
        let state = |target, current: bool, label| {
            choice_button(label, current, write(WriteOp::State(target)))
        };
        row![
            state(
                StateTarget::InProgress,
                issue.state == Some(State::InProgress),
                "In progress"
            ),
            state(
                StateTarget::Blocked,
                issue.state == Some(State::Blocked),
                "Blocked"
            ),
            state(StateTarget::Clear, false, "Clear state"),
            choice_button("Close", false, write(WriteOp::Close)),
        ]
    }
    .spacing(8)
    .wrap()
    .vertical_spacing(8);

    column![copy, changes].spacing(8).into()
}

fn write_status<'a>(app: &'a App, pane: &DetailPane, issue: &Issue) -> Element<'a, Message> {
    let key = (pane.root.clone(), issue.id.clone());
    if app.pending.contains(&key) {
        return row![
            spinner_dot(app.spin),
            text("Saving…").size(13).style(style::subtle)
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into();
    }
    match app.write_errors.get(&key) {
        Some(msg) => text(msg.as_str())
            .size(13)
            .style(style::tone_text(Tone::Danger, false))
            .into(),
        None => space().into(),
    }
}

fn comment_box<'a>(app: &'a App, pane: &'a DetailPane) -> Option<Element<'a, Message>> {
    let key = (pane.root.clone(), pane.summary.id.clone());
    let draft = app.drafts.get(&key)?;
    let idle = !app.pending.contains(&key);
    let empty = draft.text().trim().is_empty();
    let (root, id) = key;
    let mut editor = text_editor(draft)
        .placeholder("Write a comment (markdown)…")
        .height(110)
        .padding(10);
    if idle {
        let (root, id) = (root.clone(), id.clone());
        editor = editor.on_action(move |action| Message::EditComment {
            root: root.clone(),
            id: id.clone(),
            action,
        });
    }
    let submit = (idle && !empty).then_some(Message::SubmitComment { root, id });
    Some(section(
        "Add a comment",
        column![editor, primary_button_maybe("Comment", submit)]
            .spacing(10)
            .into(),
    ))
}

fn link<'a>(pane: &DetailPane, issue: &'a Issue, lead: String) -> Element<'a, Message> {
    button(
        row![
            text(lead).size(13).style(style::subtle).width(32),
            text(issue.human_id.as_str()).size(13).style(style::subtle),
            text(issue.title.as_str()).size(14).width(Fill),
            state_badge(issue),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .width(Fill)
    .padding([6, 8])
    .style(style::list_item)
    .on_press(Message::SelectIssue {
        root: pane.root.clone(),
        id: issue.id.clone(),
    })
    .into()
}

fn relations<'a>(
    app: &'a App,
    pane: &'a DetailPane,
    issue: &'a Issue,
) -> Option<Element<'a, Message>> {
    let entry = app.repos.iter().find(|e| e.info.root == pane.root)?;
    if issue.kind == Some(Kind::Epic) {
        let kids = epics::children_of(&entry.issues, issue);
        let list: Element<'a, Message> = if kids.is_empty() {
            text("No tasks yet.").size(14).style(style::muted).into()
        } else {
            column(
                kids.into_iter()
                    .map(|c| link(pane, &entry.issues[c], layer_label(&entry.issues[c]))),
            )
            .spacing(4)
            .into()
        };
        return Some(section("Tasks", list));
    }
    let parent = epics::parent_of(&entry.issues, issue)?;
    Some(section("Epic", link(pane, parent, "Epic".to_owned())))
}

fn section<'a>(title: &'a str, content: Element<'a, Message>) -> Element<'a, Message> {
    column![text(title).size(15).font(MEDIUM), content]
        .spacing(10)
        .into()
}

fn rendered<'a>(
    content: &'a markdown::Content,
    settings: markdown::Settings,
) -> Element<'a, Message> {
    markdown::view(content.items(), settings).map(|_| Message::Noop)
}

fn body<'a>(app: &'a App, pane: &'a DetailPane) -> Element<'a, Message> {
    match &pane.state {
        DetailState::Loading => column![
            skeleton(Fill, 14.0),
            skeleton(Fill, 14.0),
            skeleton(Fill, 14.0),
            skeleton(220.0, 14.0),
        ]
        .spacing(12)
        .into(),
        DetailState::Failed(msg) => column![
            text(msg.as_str())
                .size(14)
                .style(style::tone_text(Tone::Danger, false)),
            secondary_button(
                "Retry",
                Message::SelectIssue {
                    root: pane.root.clone(),
                    id: pane.summary.id.clone(),
                }
            ),
        ]
        .spacing(12)
        .into(),
        DetailState::Ready(loaded) => {
            let settings = markdown::Settings::with_text_size(14, app.theme());
            let mut out = column![rendered(&loaded.body, settings)].spacing(24);
            if !loaded.comments.is_empty() {
                let comments = loaded.detail.comments.iter().zip(&loaded.comments).map(
                    |(comment, content)| {
                        container(
                            column![
                                row![
                                    text(comment.author.as_str()).size(13).font(MEDIUM),
                                    text(comment.human_id.as_str())
                                        .size(12)
                                        .style(style::subtle),
                                ]
                                .spacing(10),
                                rendered(content, settings),
                            ]
                            .spacing(10),
                        )
                        .padding(14)
                        .width(Fill)
                        .style(style::chip)
                        .into()
                    },
                );
                out = out.push(section("Comments", column(comments).spacing(12).into()));
            }
            if let Some(editor) = comment_box(app, pane) {
                out = out.push(editor);
            }
            out.into()
        }
    }
}
