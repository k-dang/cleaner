//! Control interaction, focus styling, and the checklist scrollbar.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, Context, Div, ElementId, FocusHandle, Hsla,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement,
    Pixels, Role, Stateful, StatefulInteractiveElement, Styled, Window, div, point,
    prelude::FluentBuilder, pulsating_between, px, rems,
};

use super::CleanerView;
use crate::theme::Theme;

// Windows 11 system fonts.
pub(super) const TEXT_FONT: &str = "Segoe UI Variable Text";
pub(super) const DISPLAY_FONT: &str = "Segoe UI Variable Display";
const ICON_FONT: &str = "Segoe Fluent Icons";

// Segoe Fluent Icons code points.
pub(super) const ICON_CHECK: &str = "\u{E73E}";
pub(super) const ICON_REFRESH: &str = "\u{E72C}";
pub(super) const ICON_WARNING: &str = "\u{E7BA}";
pub(super) const ICON_ERROR: &str = "\u{EA39}";

impl CleanerView {
    pub(super) fn render_scrollbar(
        &self,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let viewport = self.list.bounds().size.height;
        let max = self.list.max_offset().y;
        if max <= px(0.) || viewport <= px(0.) {
            return None;
        }
        let thumb = thumb_length(viewport, max);
        // Wheel events move the offset past either end until the next layout clamps it,
        // so clamp here too or the thumb overshoots the track for a frame.
        let scrolled = (-self.list.offset().y).clamp(px(0.), max);
        let top = (viewport - thumb) * (scrolled / max);
        let dragging = self.thumb_drag.is_some();
        Some(
            div()
                .id("scrollbar")
                .group("scrollbar")
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(px(12.))
                .child(
                    div()
                        .id("scrollbar-thumb")
                        .absolute()
                        .top(top)
                        .right(px(3.))
                        .h(thumb)
                        .w(if dragging { px(6.) } else { px(3.) })
                        .group_hover("scrollbar", |s| s.w(px(6.)))
                        .rounded_full()
                        .bg(t.checkbox_stroke)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                this.thumb_drag = Some((e.position.y, scrolled));
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        ),
                ),
        )
    }

    pub(super) fn drag_thumb(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some((start_y, start_scrolled)) = self.thumb_drag else {
            return;
        };
        if e.pressed_button != Some(MouseButton::Left) {
            self.thumb_drag = None;
            cx.notify();
            return;
        }
        let viewport = self.list.bounds().size.height;
        let max = self.list.max_offset().y;
        let travel = viewport - thumb_length(viewport, max);
        if travel <= px(0.) {
            return;
        }
        let scrolled =
            (start_scrolled + (e.position.y - start_y) * (max / travel)).clamp(px(0.), max);
        self.list.set_offset(point(px(0.), -scrolled));
        cx.notify();
    }

    pub(super) fn checkbox(&self, checked: bool, enabled: bool) -> Div {
        let t = self.system.theme;
        let b = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(rems(1.43))
            .rounded(px(4.));
        match (checked, enabled) {
            (true, true) => b
                .bg(t.accent)
                .child(icon(ICON_CHECK, rems(0.86), t.on_accent)),
            (true, false) => b
                .bg(t.accent_disabled)
                .when(t.high_contrast, |b| {
                    b.border_1().border_color(t.text_disabled)
                })
                .child(icon(ICON_CHECK, rems(0.86), t.on_accent_disabled)),
            (false, enabled) => b.bg(t.checkbox_fill).border_1().border_color(if enabled {
                t.checkbox_stroke
            } else {
                t.text_disabled
            }),
        }
    }

    pub(super) fn button(
        &self,
        id: &'static str,
        focus: &FocusHandle,
        enabled: bool,
        window: &Window,
    ) -> Stateful<Div> {
        let t = self.system.theme;
        let ring = enabled && shows_focus(focus, window);
        div()
            .id(id)
            .track_focus(&focus.clone().tab_stop(enabled))
            .role(Role::Button)
            .when(!enabled, disabled)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .when(enabled, |b| b.cursor_pointer())
            .when(ring, |b| b.child(focus_ring(&t, px(-3.), px(7.))))
    }

    /// Pulses `el` to show work in progress, unless Windows animation effects are off.
    pub(super) fn pulsing(&self, id: impl Into<ElementId>, el: Div) -> AnyElement {
        if !self.system.animations {
            return el.into_any_element();
        }
        el.with_animation(
            id,
            Animation::new(Duration::from_millis(1200))
                .repeat()
                .with_easing(pulsating_between(0.35, 1.)),
            |el, delta| el.opacity(delta),
        )
        .into_any_element()
    }
}

pub(super) fn icon(glyph: &'static str, size: gpui::Rems, color: Hsla) -> Div {
    div()
        .font_family(ICON_FONT)
        .text_size(size)
        .line_height(size)
        .text_color(color)
        .child(glyph)
}

/// The scrollbar thumb's length for a viewport and its maximum scroll distance.
fn thumb_length(viewport: Pixels, max_scroll: Pixels) -> Pixels {
    (viewport * (viewport / (viewport + max_scroll))).max(px(24.))
}

/// Marks an element disabled for assistive technology.
pub(super) fn disabled(el: Stateful<Div>) -> Stateful<Div> {
    el.a11y_synthetic_children(|b| b.parent_node().set_disabled())
}

/// Whether `focus` holds focus that arrived from the keyboard, so it needs a focus rectangle.
pub(super) fn shows_focus(focus: &FocusHandle, window: &Window) -> bool {
    focus.is_focused(window) && window.last_input_was_keyboard()
}

/// A Windows-style keyboard focus rectangle. `inset` is negative to draw outside the element.
pub(super) fn focus_ring(t: &Theme, inset: Pixels, radius: Pixels) -> Div {
    div()
        .absolute()
        .top(inset)
        .left(inset)
        .right(inset)
        .bottom(inset)
        .border_2()
        .border_color(t.focus)
        .rounded(radius)
}
