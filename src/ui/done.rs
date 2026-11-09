//! Roadmap Done review sheet: one checkbox row per candidate with its commits; results tick in as closes finish.

use iced::widget::{
    center, checkbox, column, container, mouse_area, opaque, row, scrollable, space, stack, text,
};
use iced::{Alignment, Element, Fill};
use lucide_iced::icon as lu;

use super::components::{
    MEDIUM, chip, choice_button, icon_square_button, secondary_button, skeleton, spinner_dot,
};
use crate::app::{App, DonePhase, DoneSheet, Message};
use crate::theme::{Tone, style};
use gbtracker::done::{Candidate, EpicCandidate, Report};

const WIDTH: f32 = 640.0;
const WHY: usize = 2;
const SHA_CHARS: usize = 7;

pub fn overlay<'a>(app: &'a App, base: Element<'a, Message>) -> Element<'a, Message> {
    let Some(sheet) = &app.done else {
        return base;
    };
    let scrim = container(center(opaque(card(app, sheet))))
        .width(Fill)
        .height(Fill)
        .style(style::scrim);
    stack![
        base,
        opaque(mouse_area(scrim).on_press(Message::DismissDone))
    ]
    .into()
}

fn card<'a>(app: &'a App, sheet: &'a DoneSheet) -> Element<'a, Message> {
    let header = row![
        column![
            text("Roadmap Done").size(20).font(MEDIUM),
            text(sheet.name.as_str()).size(13).style(style::subtle),
        ]
        .spacing(4),
        space::horizontal(),
        icon_square_button(
            lu::x(),
            14.0,
            32.0,
            "Close",
            sheet.can_dismiss().then_some(Message::DismissDone)
        ),
    ]
    .align_y(Alignment::Center);

    let body: Element<'a, Message> = match (sheet.phase, &sheet.report) {
        (DonePhase::Loading, _) => loading(app),
        (DonePhase::Failed, _) => text(sheet.error.as_deref().unwrap_or("failed"))
            .size(14)
            .style(style::tone_text(Tone::Danger, false))
            .into(),
        (_, Some(report)) if report.is_empty() => text(format!(
            "Nothing to close: none of the {} open issues has a Refs: commit on {}.",
            report.open_count,
            report.refs.join(" + ")
        ))
        .size(14)
        .style(style::muted)
        .into(),
        (_, Some(report)) => review(app, sheet, report),
        _ => space().into(),
    };

    container(column![header, body, footer(sheet)].spacing(20))
        .padding(24)
        .width(WIDTH)
        .max_height(720)
        .style(style::card)
        .into()
}

fn loading(app: &App) -> Element<'_, Message> {
    column![
        row![
            spinner_dot(app.spin),
            text("Reading the base branch…")
                .size(14)
                .style(style::muted)
        ]
        .spacing(10)
        .align_y(Alignment::Center),
        skeleton(Fill, 14.0),
        skeleton(Fill, 14.0),
        skeleton(240.0, 14.0),
    ]
    .spacing(14)
    .into()
}

fn review<'a>(app: &'a App, sheet: &'a DoneSheet, report: &'a Report) -> Element<'a, Message> {
    let total = report.bugs.len() + report.epics.len();
    let head = text(format!(
        "{total} of {} open look merged into {}",
        report.open_count,
        report.refs.join(" + ")
    ))
    .size(14)
    .style(style::muted);

    let rows = report
        .bugs
        .iter()
        .map(|b| bug_row(app, sheet, b))
        .chain(report.epics.iter().map(|e| epic_row(app, sheet, e)));
    column![
        head,
        scrollable(column(rows).spacing(10).padding(iced::Padding {
            right: 12.0,
            ..iced::Padding::ZERO
        }))
        .height(Fill)
        .style(style::scroll)
    ]
    .spacing(14)
    .height(Fill)
    .into()
}

fn tick<'a>(
    sheet: &DoneSheet,
    id: &'a str,
    checked: bool,
    label: String,
) -> iced::widget::Checkbox<'a, Message> {
    let toggle = (sheet.phase == DonePhase::Choose).then(|| id.to_owned());
    checkbox(checked)
        .label(label)
        .size(18)
        .text_size(15)
        .on_toggle_maybe(toggle.map(|id| move |_| Message::ToggleDone(id.clone())))
}

fn outcome<'a>(app: &'a App, sheet: &'a DoneSheet, id: &str) -> Option<Element<'a, Message>> {
    if sheet.is_waiting(id) {
        return Some(
            row![
                spinner_dot(app.spin),
                text("Closing…").size(13).style(style::subtle)
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    Some(match sheet.results.get(id)? {
        Ok(()) => text("Closed")
            .size(13)
            .style(style::tone_text(Tone::Success, false))
            .into(),
        Err(msg) => text(format!("Failed: {msg}"))
            .size(13)
            .style(style::tone_text(Tone::Danger, false))
            .into(),
    })
}

fn row_card<'a>(
    head: Element<'a, Message>,
    why: Vec<String>,
    status: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut lines = column(why.into_iter().map(|line| {
        text(line)
            .size(12)
            .style(style::subtle)
            .wrapping(text::Wrapping::None)
            .into()
    }))
    .spacing(3)
    .padding(iced::Padding {
        left: 28.0,
        ..iced::Padding::ZERO
    });
    if let Some(status) = status {
        lines = lines.push(container(status).padding(iced::Padding {
            top: 3.0,
            ..iced::Padding::ZERO
        }));
    }
    container(column![head, lines].spacing(6))
        .padding([12, 14])
        .width(Fill)
        .style(style::chip)
        .into()
}

fn bug_row<'a>(app: &'a App, sheet: &'a DoneSheet, bug: &'a Candidate) -> Element<'a, Message> {
    let label = format!("{}  {}", bug.human_id, bug.title);
    let mut why: Vec<String> = bug
        .evidence
        .iter()
        .take(WHY)
        .map(|e| {
            format!(
                "{}  {}",
                e.sha.chars().take(SHA_CHARS).collect::<String>(),
                e.subject
            )
        })
        .collect();
    if bug.evidence.len() > WHY {
        why.push(format!("+{} more commits", bug.evidence.len() - WHY));
    }
    let head = row![
        tick(sheet, &bug.id, sheet.picked.contains(&bug.id), label).width(Fill),
        chip(super::screen::kind_label(bug.kind.as_ref())),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    row_card(head.into(), why, outcome(app, sheet, &bug.id))
}

fn epic_row<'a>(
    app: &'a App,
    sheet: &'a DoneSheet,
    epic: &'a EpicCandidate,
) -> Element<'a, Message> {
    let label = format!("{}  {}", epic.human_id, epic.title);
    let allowed = epic.is_allowed(&sheet.picked);
    let closed = epic.children - epic.needs.len();
    let mut why = vec![if epic.needs.is_empty() {
        format!("all {} children closed", epic.children)
    } else {
        format!(
            "{closed}/{} children closed, the rest close above",
            epic.children
        )
    }];
    if !allowed && sheet.phase == DonePhase::Choose {
        why.push("waiting on unticked children".to_owned());
    }
    let mut check = tick(
        sheet,
        &epic.id,
        sheet.picked.contains(&epic.id) && allowed,
        label,
    )
    .width(Fill);
    if !allowed {
        check = check.on_toggle_maybe(None::<fn(bool) -> Message>);
    }
    let head = row![check, chip("epic"),]
        .spacing(10)
        .align_y(Alignment::Center);
    row_card(head.into(), why, outcome(app, sheet, &epic.id))
}

fn footer(sheet: &DoneSheet) -> Element<'_, Message> {
    let choosing =
        sheet.phase == DonePhase::Choose && sheet.report.as_ref().is_some_and(|r| !r.is_empty());
    let mut bar = row![].spacing(8).align_y(Alignment::Center);
    if choosing {
        let n = sheet.selected_count();
        bar = bar
            .push(secondary_button("All", Message::PickDone(true)))
            .push(secondary_button("None", Message::PickDone(false)))
            .push(text(format!("{n} selected")).size(13).style(style::subtle))
            .push(space::horizontal())
            .push(choice_button(
                "Close selected",
                true,
                (n > 0).then_some(Message::RunDone),
            ));
    } else {
        let label = if sheet.phase == DonePhase::Closing {
            "Closing…"
        } else {
            "Done"
        };
        bar = bar.push(space::horizontal()).push(choice_button(
            label,
            sheet.phase != DonePhase::Closing,
            (sheet.phase != DonePhase::Closing).then_some(Message::DismissDone),
        ));
    }
    bar.into()
}
