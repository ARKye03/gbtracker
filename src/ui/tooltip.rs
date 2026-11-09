//! Animated floating tooltip: delayed open, warm group, fade and slide, flip and shift to stay in the viewport.

use std::cell::Cell;
use std::time::Duration;

use iced::advanced::widget::{self, Tree, tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, overlay, renderer};
use iced::animation::{Animation, Easing};
use iced::time::Instant;
use iced::widget::{container, row, text, text::Wrapping};
use iced::{
    Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector, mouse, window,
};

use crate::theme::style;

const OPEN_DELAY: Duration = Duration::from_millis(350);
const WARM_WINDOW: Duration = Duration::from_millis(300);
const FADE_IN: Duration = Duration::from_millis(120);
const FADE_OUT: Duration = Duration::from_millis(80);
const SLIDE: f32 = 4.0;
const GAP: f32 = 8.0;
const EDGE: f32 = 4.0;

thread_local! {
    static LAST_SHOWN: Cell<Option<Instant>> = const { Cell::new(None) };
}

pub fn tip<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    label: impl Into<String>,
) -> Tooltip<'a, M> {
    Tooltip {
        content: content.into(),
        label: label.into(),
        shortcut: None,
        enabled: true,
    }
}

pub struct Tooltip<'a, M> {
    content: Element<'a, M>,
    label: String,
    shortcut: Option<String>,
    enabled: bool,
}

impl<M> Tooltip<'_, M> {
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn shortcut_maybe(mut self, shortcut: Option<String>) -> Self {
        self.shortcut = shortcut;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Pending(Instant),
    Open,
    /// Closed by a press or scroll; stays closed until the cursor leaves.
    Dismissed,
}

fn is_warm(last_shown: Option<Instant>, now: Instant) -> bool {
    last_shown.is_some_and(|at| now.saturating_duration_since(at) < WARM_WINDOW)
}

fn next_phase(phase: Phase, now: Instant, hovering: bool, dismissed: bool, warm: bool) -> Phase {
    if !hovering {
        return Phase::Idle;
    }
    if dismissed || phase == Phase::Dismissed {
        return Phase::Dismissed;
    }
    match phase {
        Phase::Idle if warm => Phase::Open,
        Phase::Idle => Phase::Pending(now),
        Phase::Pending(since) if now.saturating_duration_since(since) >= OPEN_DELAY => Phase::Open,
        other => other,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Below,
    Above,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Placement {
    position: Point,
    side: Side,
}

/// Prefers below the anchor, flips above when it would overflow the viewport, then shifts horizontally to fit.
fn place(anchor: Rectangle, size: Size, viewport: Size, gap: f32) -> Placement {
    let below = anchor.y + anchor.height + gap;
    let above = anchor.y - gap - size.height;
    let (y, side) = if below + size.height <= viewport.height - EDGE || above < EDGE {
        (below, Side::Below)
    } else {
        (above, Side::Above)
    };
    let x = anchor.x + (anchor.width - size.width) / 2.0;
    let max_x = (viewport.width - size.width - EDGE).max(EDGE);
    let max_y = (viewport.height - size.height - EDGE).max(EDGE);
    Placement {
        position: Point::new(x.clamp(EDGE, max_x), y.clamp(EDGE, max_y)),
        side,
    }
}

struct State {
    phase: Phase,
    fade: Animation<bool>,
    /// The renderer holds text paragraphs weakly, so they must live in a tree that outlasts the frame.
    body: Option<Tree>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            fade: Animation::new(false).easing(Easing::EaseOut),
            body: None,
        }
    }
}

impl State {
    fn set_visible(&mut self, visible: bool, now: Instant) {
        let duration = if visible { FADE_IN } else { FADE_OUT };
        self.fade = self.fade.clone().duration(duration).go(visible, now);
    }

    fn shown(&self, now: Instant) -> bool {
        self.fade.value() || self.fade.is_animating(now)
    }
}

impl<M> Widget<M, Theme, Renderer> for Tooltip<'_, M> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[self.content.as_widget()]);
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
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
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        if matches!(
            event,
            Event::Mouse(_) | Event::Window(window::Event::RedrawRequested(_))
        ) {
            let state = tree.state.downcast_mut::<State>();
            let now = Instant::now();
            let hovering = self.enabled && cursor.position_over(layout.bounds()).is_some();
            let dismissed = matches!(
                event,
                Event::Mouse(mouse::Event::ButtonPressed(_) | mouse::Event::WheelScrolled { .. })
            );
            let warm = is_warm(LAST_SHOWN.get(), now);
            // A pending tooltip re-reads warmth: an earlier sibling can refresh LAST_SHOWN after this one ran.
            let before = match state.phase {
                Phase::Pending(_) if warm => Phase::Idle,
                phase => phase,
            };
            let after = next_phase(before, now, hovering, dismissed, warm);
            state.phase = after;

            match (before, after) {
                (Phase::Open, Phase::Open) => LAST_SHOWN.set(Some(now)),
                (_, Phase::Open) => {
                    LAST_SHOWN.set(Some(now));
                    state.set_visible(true, now);
                    shell.invalidate_layout();
                }
                (Phase::Open, _) => {
                    LAST_SHOWN.set(Some(now));
                    state.set_visible(false, now);
                }
                _ => {}
            }
            if let Phase::Pending(since) = after {
                if before == Phase::Idle {
                    shell.request_redraw();
                }
                shell.request_redraw_at(since + OPEN_DELAY);
            }
            if state.fade.is_animating(now) {
                shell.request_redraw();
            }
        }

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

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, Theme, Renderer>> {
        let state = tree.state.downcast_mut::<State>();
        let now = Instant::now();
        let progress = state
            .shown(now)
            .then(|| state.fade.interpolate(0.0_f32, 1.0, now));

        let content = self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        );

        let tooltip = progress.map(|progress| {
            let body = tooltip_body(&self.label, self.shortcut.as_deref(), progress);
            let body_tree = match &mut state.body {
                Some(existing) => {
                    existing.diff(body.as_widget());
                    existing
                }
                empty => empty.insert(Tree::new(&body)),
            };
            overlay::Element::new(Box::new(Floating {
                anchor: Rectangle::new(layout.position() + translation, layout.bounds().size()),
                tree: body_tree,
                body,
                progress,
            }))
        });

        match (content, tooltip) {
            (None, None) => None,
            (content, tooltip) => Some(
                overlay::Group::with_children(content.into_iter().chain(tooltip).collect())
                    .overlay(),
            ),
        }
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
}

impl<'a, M: 'a> From<Tooltip<'a, M>> for Element<'a, M> {
    fn from(tooltip: Tooltip<'a, M>) -> Self {
        Element::new(tooltip)
    }
}

fn tooltip_body<'a, M: 'a>(label: &str, shortcut: Option<&str>, alpha: f32) -> Element<'a, M> {
    let mut content = row![
        text(label.to_owned())
            .size(13)
            .wrapping(Wrapping::None)
            .style(style::faded(alpha))
    ]
    .spacing(10);
    if let Some(shortcut) = shortcut {
        content = content.push(
            text(shortcut.to_owned())
                .size(13)
                .wrapping(Wrapping::None)
                .style(style::faded_subtle(alpha)),
        );
    }
    container(content)
        .padding([6, 10])
        .style(style::tooltip(alpha))
        .into()
}

struct Floating<'b, M> {
    anchor: Rectangle,
    body: Element<'b, M>,
    tree: &'b mut Tree,
    progress: f32,
}

impl<M> overlay::Overlay<M, Theme, Renderer> for Floating<'_, M> {
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let node = self.body.as_widget_mut().layout(
            self.tree,
            renderer,
            &layout::Limits::new(Size::ZERO, bounds),
        );
        let placed = place(self.anchor, node.size(), bounds, GAP);
        let toward_anchor = match placed.side {
            Side::Below => -1.0,
            Side::Above => 1.0,
        };
        let slide = (1.0 - self.progress) * SLIDE * toward_anchor;
        node.move_to(Point::new(placed.position.x, placed.position.y + slide))
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        defaults: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
    ) {
        self.body.as_widget().draw(
            self.tree,
            renderer,
            theme,
            defaults,
            layout,
            mouse::Cursor::Unavailable,
            &Rectangle::with_size(Size::INFINITE),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: Size = Size::new(800.0, 600.0);
    const TIP: Size = Size::new(80.0, 28.0);

    fn anchor(x: f32, y: f32) -> Rectangle {
        Rectangle::new(Point::new(x, y), Size::new(40.0, 40.0))
    }

    #[test]
    fn stays_below_and_centered_when_there_is_room() {
        let p = place(anchor(100.0, 50.0), TIP, VIEWPORT, GAP);
        assert_eq!(p.side, Side::Below);
        assert_eq!(p.position, Point::new(80.0, 98.0));
    }

    #[test]
    fn flips_above_near_the_bottom_edge() {
        let p = place(anchor(100.0, 570.0), TIP, VIEWPORT, GAP);
        assert_eq!(p.side, Side::Above);
        assert_eq!(p.position.y, 570.0 - GAP - TIP.height);
    }

    #[test]
    fn stays_below_when_neither_side_fits() {
        let p = place(anchor(100.0, 10.0), TIP, Size::new(800.0, 40.0), GAP);
        assert_eq!(p.side, Side::Below);
    }

    #[test]
    fn shifts_left_near_the_right_edge() {
        let p = place(anchor(780.0, 50.0), TIP, VIEWPORT, GAP);
        assert_eq!(p.side, Side::Below);
        assert_eq!(p.position.x, VIEWPORT.width - TIP.width - EDGE);
    }

    #[test]
    fn shifts_right_near_the_left_edge() {
        let p = place(anchor(0.0, 50.0), TIP, VIEWPORT, GAP);
        assert_eq!(p.position.x, EDGE);
    }

    #[test]
    fn cold_hover_waits_for_the_open_delay() {
        let t0 = Instant::now();
        let pending = next_phase(Phase::Idle, t0, true, false, false);
        assert_eq!(pending, Phase::Pending(t0));
        let early = t0 + OPEN_DELAY.saturating_sub(Duration::from_millis(1));
        assert_eq!(next_phase(pending, early, true, false, false), pending);
        let due = t0 + OPEN_DELAY;
        assert_eq!(next_phase(pending, due, true, false, false), Phase::Open);
    }

    #[test]
    fn warm_hover_opens_instantly() {
        let now = Instant::now();
        assert_eq!(next_phase(Phase::Idle, now, true, false, true), Phase::Open);
    }

    #[test]
    fn warmth_expires_after_the_window() {
        let t0 = Instant::now();
        assert!(!is_warm(None, t0));
        assert!(is_warm(
            Some(t0),
            t0 + WARM_WINDOW.saturating_sub(Duration::from_millis(1))
        ));
        assert!(!is_warm(Some(t0), t0 + WARM_WINDOW));
    }

    #[test]
    fn press_closes_and_stays_closed_until_the_cursor_leaves() {
        let now = Instant::now();
        let closed = next_phase(Phase::Open, now, true, true, true);
        assert_eq!(closed, Phase::Dismissed);
        assert_eq!(next_phase(closed, now, true, false, true), Phase::Dismissed);
        assert_eq!(next_phase(closed, now, false, false, true), Phase::Idle);
    }

    #[test]
    fn press_during_the_delay_cancels_the_open() {
        let t0 = Instant::now();
        let pending = Phase::Pending(t0);
        let later = t0 + OPEN_DELAY * 2;
        assert_eq!(
            next_phase(pending, later, true, true, false),
            Phase::Dismissed
        );
    }

    #[test]
    fn leaving_closes_and_cancels() {
        let now = Instant::now();
        assert_eq!(
            next_phase(Phase::Open, now, false, false, true),
            Phase::Idle
        );
        assert_eq!(
            next_phase(Phase::Pending(now), now, false, false, false),
            Phase::Idle
        );
    }
}
