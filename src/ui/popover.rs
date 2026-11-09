//! Top-bar popovers: a panel that floats under its trigger button. The scrim from `scrim` closes it on an outside click.

use iced::advanced::widget::{self, Tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, overlay, renderer};
use iced::widget::{
    button, column, container, mouse_area, opaque, row, scrollable, space, stack, text,
    text::Wrapping,
};
use iced::{
    Alignment, Element, Event, Fill, Length, Padding, Point, Rectangle, Renderer, Size, Theme,
    Vector, mouse,
};
use lucide_iced::icon as lu;

use super::components::{MEDIUM, dot, icon, icon_muted};
use super::screen::{status_indicator, status_label};
use crate::app::{App, LoadState, Message, Notification, Popover, ago};
use crate::keymap::{self, Action};
use crate::theme::style;
use gbtracker::workspace::{RootKind, ThemePref};

const GAP: f32 = 8.0;
const EDGE: f32 = 8.0;
const NOTE_CLIP: usize = 200;

/// Which edge of the trigger the panel lines up with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Start,
    End,
}

/// Transparent full-window layer under an open popover; any press on it closes the popover.
pub fn scrim<'a>(app: &App, base: Element<'a, Message>) -> Element<'a, Message> {
    if app.open_popover.is_none() {
        return base;
    }
    let layer = container(space()).width(Fill).height(Fill);
    stack![
        base,
        opaque(mouse_area(layer).on_press(Message::ClosePopover))
    ]
    .into()
}

/// Wraps `trigger` so the panel for `which` floats under it while that popover is open.
pub fn anchored<'a>(
    app: &'a App,
    which: Popover,
    align: Align,
    trigger: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let panel = (app.open_popover == Some(which)).then(|| panel(app, which));
    Element::new(Anchored {
        content: trigger.into(),
        panel,
        align,
    })
}

fn panel<'a>(app: &'a App, which: Popover) -> Element<'a, Message> {
    let (width, body) = match which {
        Popover::Repos => (320.0, repos(app)),
        Popover::Overflow => (300.0, overflow(app)),
        Popover::Notifications => (380.0, notifications(app)),
        Popover::Identity => (300.0, identity(app)),
    };
    container(scrollable(body).style(style::scroll))
        .padding(8)
        .width(width)
        .style(style::popover)
        .into()
}

fn heading<'a>(label: &'a str) -> Element<'a, Message> {
    container(text(label).size(13).style(style::subtle))
        .padding([6, 12])
        .into()
}

fn rule<'a>() -> Element<'a, Message> {
    container(
        container(space())
            .width(Fill)
            .height(1)
            .style(style::divider),
    )
    .padding([6, 4])
    .into()
}

fn item<'a>(
    content: impl Into<Element<'a, Message>>,
    selected: bool,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    button(content)
        .width(Fill)
        .padding([9, 12])
        .style(style::menu_item(selected))
        .on_press_maybe(on_press)
        .into()
}

fn action<'a>(
    glyph: iced::widget::Svg<'static>,
    label: impl text::IntoFragment<'a>,
    hint: Option<String>,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let glyph: Element<'a, Message> = icon(glyph, 16.0).into();
    let mut line = row![glyph, text(label).size(14)]
        .spacing(12)
        .align_y(Alignment::Center);
    if let Some(hint) = hint {
        line = line
            .push(space::horizontal())
            .push(text(hint).size(13).style(style::subtle));
    }
    item(line, false, on_press)
}

fn repos(app: &App) -> Element<'_, Message> {
    let open: usize = app.repos.iter().map(|e| e.open_count()).sum();
    let mut list = column![item(
        row![
            icon(lu::layers(), 16.0),
            text("All repositories").size(14),
            space::horizontal(),
            text!("{open} open").size(13).style(style::subtle),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
        app.filter.repos.is_empty(),
        Some(Message::ChooseRepo(None)),
    )]
    .spacing(2);

    for entry in &app.repos {
        let trailing = match &entry.state {
            LoadState::Error(_) | LoadState::NoGitBug | LoadState::Locked => {
                status_label(&entry.state).0.to_owned()
            }
            _ if entry.fingerprint.is_some() => format!("{} open", entry.open_count()),
            state => status_label(state).0.to_owned(),
        };
        let line = row![
            status_indicator(&entry.state, app.spin),
            text(entry.info.name.as_str())
                .size(14)
                .wrapping(Wrapping::None),
            space::horizontal(),
            text(trailing).size(13).style(style::subtle),
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        list = list.push(item(
            line,
            app.filter.repos.contains(&entry.info.root),
            entry
                .info
                .has_git_bug
                .then(|| Message::ChooseRepo(Some(entry.info.root.clone()))),
        ));
    }
    if app.repos.is_empty() {
        list = list.push(
            container(
                text("No repositories found yet.")
                    .size(14)
                    .style(style::muted),
            )
            .padding([10, 12]),
        );
    }
    list.into()
}

fn overflow(app: &App) -> Element<'_, Message> {
    let mut list = column![
        action(
            lu::refresh_cw(),
            "Rescan",
            keymap::hint(Action::Rescan),
            Some(Message::Rescan)
        ),
        action(
            lu::plus(),
            "Add project",
            None,
            Some(Message::PickFolder(RootKind::Project))
        ),
        action(
            lu::folder_plus(),
            "Add folder to scan",
            None,
            Some(Message::PickFolder(RootKind::Scan))
        ),
        rule(),
        heading("Roadmap Done"),
    ]
    .spacing(2);

    let tracked: Vec<_> = app.repos.iter().filter(|e| e.info.has_git_bug).collect();
    if tracked.is_empty() {
        list = list.push(
            container(
                text("No repositories with git-bug.")
                    .size(14)
                    .style(style::muted),
            )
            .padding([6, 12]),
        );
    }
    for entry in tracked {
        list = list.push(action(
            lu::circle_check(),
            entry.info.name.as_str(),
            None,
            entry
                .fingerprint
                .is_some()
                .then(|| Message::OpenDone(entry.info.root.clone())),
        ));
    }

    list.push(rule())
        .push(action(
            lu::keyboard(),
            "Keyboard shortcuts",
            keymap::hint(Action::Help),
            Some(Message::ToggleHelp),
        ))
        .into()
}

fn notifications(app: &App) -> Element<'_, Message> {
    let clear = button(text("Clear").size(13))
        .padding([4, 8])
        .style(style::ghost)
        .on_press_maybe((!app.notifications.is_empty()).then_some(Message::ClearNotifications));
    let header = container(
        row![
            text("Notifications").size(15).font(MEDIUM),
            space::horizontal(),
            clear
        ]
        .align_y(Alignment::Center),
    )
    .padding(Padding {
        top: 4.0,
        right: 4.0,
        bottom: 8.0,
        left: 12.0,
    });

    let mut list = column![].spacing(2);
    if app.notifications.is_empty() {
        list = list.push(
            container(text("Nothing to report.").size(14).style(style::muted)).padding([6, 12]),
        );
    }
    for note in app.notifications.iter().rev() {
        list = list.push(notification(note));
    }
    column![header, list].into()
}

fn notification(note: &Notification) -> Element<'_, Message> {
    let age = ago(note.at.elapsed().unwrap_or_default());
    let mut body = column![text(note.title.as_str()).size(14)].spacing(2);
    if let Some(detail) = &note.detail {
        body = body.push(text(clip(detail)).size(13).style(style::muted));
    }
    body = body.push(text(age).size(12).style(style::subtle));
    container(
        row![container(dot(note.tone, 10.0)).padding([5, 0]), body]
            .spacing(12)
            .width(Fill),
    )
    .padding([8, 12])
    .width(Length::Fill)
    .into()
}

fn clip(text: &str) -> String {
    if text.chars().count() <= NOTE_CLIP {
        return text.to_owned();
    }
    let head: String = text.chars().take(NOTE_CLIP).collect();
    format!("{head}…")
}

fn identity(app: &App) -> Element<'_, Message> {
    let (name, email) = match &app.identity {
        Some(who) if !who.email.is_empty() => (who.name.as_str(), who.email.as_str()),
        Some(who) => (who.name.as_str(), "No email set"),
        None => ("Unknown identity", "git-bug has not reported one yet"),
    };
    let who = row![
        container(text(app.initials.as_str()).size(15).font(MEDIUM))
            .center(44)
            .style(style::avatar),
        column![
            text(name).size(15).font(MEDIUM),
            text(email).size(13).style(style::muted)
        ]
        .spacing(2),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let themes = [
        (ThemePref::System, lu::monitor(), "System"),
        (ThemePref::Light, lu::sun(), "Light"),
        (ThemePref::Dark, lu::moon(), "Dark"),
    ]
    .into_iter()
    .map(|(pref, glyph, label)| {
        let selected = app.workspace.theme == pref;
        let mut line = row![icon_muted(glyph, 16.0), text(label).size(14)]
            .spacing(12)
            .align_y(Alignment::Center);
        if selected {
            line = line
                .push(space::horizontal())
                .push(lu::check().width(14).height(14).style(style::icon_accent));
        }
        item(line, selected, Some(Message::SetTheme(pref)))
    });

    column![
        container(who).padding([8, 12]),
        rule(),
        heading("Theme"),
        column(themes).spacing(2),
    ]
    .into()
}

struct Anchored<'a> {
    content: Element<'a, Message>,
    panel: Option<Element<'a, Message>>,
    align: Align,
}

impl Widget<Message, Theme, Renderer> for Anchored<'_> {
    fn children(&self) -> Vec<Tree> {
        let mut trees = vec![Tree::new(&self.content)];
        trees.extend(self.panel.as_ref().map(Tree::new));
        trees
    }

    fn diff(&self, tree: &mut Tree) {
        let mut widgets = vec![self.content.as_widget()];
        widgets.extend(self.panel.as_ref().map(|p| p.as_widget()));
        tree.diff_children(&widgets);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let Self {
            content,
            panel,
            align,
        } = self;
        let (first, rest) = tree.children.split_at_mut(1);
        let inner =
            content
                .as_widget_mut()
                .overlay(&mut first[0], layout, renderer, viewport, translation);
        let floating = panel.as_mut().zip(rest.first_mut()).map(|(body, tree)| {
            overlay::Element::new(Box::new(Floating {
                anchor: Rectangle::new(layout.position() + translation, layout.bounds().size()),
                align: *align,
                body,
                tree,
            }))
        });
        let children: Vec<_> = inner.into_iter().chain(floating).collect();
        (!children.is_empty()).then(|| overlay::Group::with_children(children).overlay())
    }
}

struct Floating<'a, 'b> {
    anchor: Rectangle,
    align: Align,
    body: &'b mut Element<'a, Message>,
    tree: &'b mut Tree,
}

fn place(anchor: Rectangle, size: Size, bounds: Size, align: Align) -> Point {
    let x = match align {
        Align::Start => anchor.x,
        Align::End => anchor.x + anchor.width - size.width,
    };
    let max_x = (bounds.width - size.width - EDGE).max(EDGE);
    Point::new(x.clamp(EDGE, max_x), anchor.y + anchor.height + GAP)
}

impl overlay::Overlay<Message, Theme, Renderer> for Floating<'_, '_> {
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let top = self.anchor.y + self.anchor.height + GAP;
        let room = Size::new(
            bounds.width - 2.0 * EDGE,
            (bounds.height - top - EDGE).max(0.0),
        );
        let node = self.body.as_widget_mut().layout(
            self.tree,
            renderer,
            &layout::Limits::new(Size::ZERO, room),
        );
        let at = place(self.anchor, node.size(), bounds, self.align);
        node.move_to(at)
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        defaults: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.body.as_widget().draw(
            self.tree,
            renderer,
            theme,
            defaults,
            layout,
            cursor,
            &layout.bounds(),
        );
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.body
            .as_widget_mut()
            .operate(self.tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        self.body.as_widget_mut().update(
            self.tree,
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            &layout.bounds(),
        );
        if matches!(event, Event::Mouse(mouse::Event::ButtonPressed(_)))
            && cursor.is_over(layout.bounds())
        {
            shell.capture_event();
        }
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.body.as_widget().mouse_interaction(
            self.tree,
            layout,
            cursor,
            &layout.bounds(),
            renderer,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Size = Size::new(1000.0, 700.0);
    const PANEL: Size = Size::new(300.0, 200.0);

    #[test]
    fn panel_sits_below_the_trigger_on_the_chosen_edge() {
        let trigger = Rectangle::new(Point::new(400.0, 14.0), Size::new(120.0, 40.0));
        let start = place(trigger, PANEL, WINDOW, Align::Start);
        assert_eq!(start, Point::new(400.0, 14.0 + 40.0 + GAP));
        let end = place(trigger, PANEL, WINDOW, Align::End);
        assert_eq!(end.x, 400.0 + 120.0 - 300.0);
    }

    #[test]
    fn panel_stays_inside_the_window() {
        let right = Rectangle::new(Point::new(960.0, 14.0), Size::new(40.0, 40.0));
        assert_eq!(
            place(right, PANEL, WINDOW, Align::Start).x,
            WINDOW.width - PANEL.width - EDGE
        );
        let left = Rectangle::new(Point::new(0.0, 14.0), Size::new(40.0, 40.0));
        assert_eq!(place(left, PANEL, WINDOW, Align::End).x, EDGE);
    }
}
