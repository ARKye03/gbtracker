//! Top-bar popovers: a panel that floats under its trigger button. The scrim from `scrim` closes it on an outside click.

use iced::advanced::widget::{self, Tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, overlay, renderer};
use iced::widget::{
    button, column, container, mouse_area, opaque, row, scrollable, space, stack, text,
};
use iced::{
    Alignment, Element, Event, Fill, Length, Padding, Point, Rectangle, Renderer, Size, Theme,
    Vector, mouse,
};

use super::components::{MEDIUM, dot};
use crate::app::{App, Message, Notification, Popover, ago};
use crate::theme::style;

const GAP: f32 = 8.0;
const EDGE: f32 = 8.0;
const NOTE_CLIP: usize = 200;

/// Which edge of the trigger the panel lines up with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    #[cfg_attr(not(test), allow(dead_code))]
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
        Popover::Notifications => (380.0, notifications(app)),
    };
    container(scrollable(body).style(style::scroll))
        .padding(8)
        .width(width)
        .style(style::popover)
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
        widgets.extend(self.panel.as_ref().map(Element::as_widget));
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
