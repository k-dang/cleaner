//! Cleaner screen layout and accessible names.

use std::rc::Rc;

use cleaner_core::controller::Summary;
use gpui::{
    AccessibleAction, AnyElement, ClickEvent, Context, Div, FontWeight, InteractiveElement,
    IntoElement, KeyboardButton, KeyboardClickEvent, MouseButton, ParentElement, Render, Role,
    Stateful, StatefulInteractiveElement, Styled, Toggled, Window, accesskit, div,
    prelude::FluentBuilder, px, rems,
};

use super::controls::{
    DISPLAY_FONT, ICON_ERROR, ICON_REFRESH, TEXT_FONT, disabled, focus_ring, icon, shows_focus,
};
use super::presentation::{Header, header, row_status, sections, status_line};
use super::{CleanerView, FocusNext, FocusPrev};
use crate::{format, targets};

impl CleanerView {
    fn render_header(
        &self,
        summary: &Summary,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = self.system.theme;
        let Header {
            overview,
            note,
            estimate,
            pending,
            unscanned,
            accessible_name,
        } = header(
            summary,
            self.controller
                .scanning_target()
                .and_then(targets::find)
                .map(|recipe| recipe.target.name),
        );
        let idle = self.controller.can_scan();
        let scrolled = self.list.offset().y < px(0.);

        div()
            .flex()
            .items_start()
            .justify_between()
            .gap(rems(1.))
            .px(rems(1.43))
            .pt(rems(1.43))
            .pb(rems(1.14))
            .border_b_1()
            .border_color(if scrolled {
                t.divider
            } else {
                gpui::transparent_black()
            })
            .child(
                div()
                    .id("total")
                    .role(Role::Label)
                    .aria_value(accessible_name)
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_family(DISPLAY_FONT)
                            .text_size(rems(1.64))
                            .line_height(rems(1.86))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Review your cleanup"),
                    )
                    .child(div().text_color(t.text_secondary).child(overview))
                    .child(
                        div()
                            .pt(rems(0.86))
                            .flex()
                            .items_baseline()
                            .gap(rems(0.71))
                            .font_family(DISPLAY_FONT)
                            .child({
                                let number = div()
                                    .text_size(rems(2.86))
                                    .line_height(rems(3.))
                                    .font_weight(FontWeight::SEMIBOLD);
                                match estimate {
                                    Some(estimate) => number
                                        .when(pending, |d| d.text_color(t.text_secondary))
                                        .child(estimate)
                                        .into_any_element(),
                                    None if unscanned => number
                                        .text_color(t.text_secondary)
                                        .child("—")
                                        .into_any_element(),
                                    // Nothing is known yet. Hidden text sizes the placeholder
                                    // like a number, so nothing moves when one arrives.
                                    None => self.pulsing(
                                        "estimate-placeholder",
                                        number
                                            .rounded(px(4.))
                                            .bg(t.divider)
                                            .text_color(gpui::transparent_black())
                                            .child("00 GB"),
                                    ),
                                }
                            })
                            .child(
                                div()
                                    .text_size(rems(0.86))
                                    .text_color(t.text_secondary)
                                    .child("selected estimate"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(rems(0.86))
                            .text_color(t.text_secondary)
                            .child(note),
                    ),
            )
            .child(
                self.button("scan", &self.scan_focus, idle, window)
                    .aria_label("Scan")
                    .gap(rems(0.43))
                    .px(rems(0.71))
                    .h(rems(2.43))
                    .border_1()
                    .border_color(t.card_stroke)
                    .bg(t.card)
                    .text_color(if idle { t.text } else { t.text_disabled })
                    .when(idle, |b| {
                        b.hover(|s| s.bg(t.subtle_hover))
                            .active(|s| s.bg(t.subtle_pressed))
                            .on_click(cx.listener(|this, _, _, cx| this.scan(cx)))
                    })
                    .child(icon(
                        ICON_REFRESH,
                        rems(1.14),
                        if idle { t.text } else { t.text_disabled },
                    ))
                    .child("Scan"),
            )
    }

    fn render_list(
        &mut self,
        scanning: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = self.system.theme;
        let locked = self.controller.selection_locked();
        let rows = self.controller.rows();
        let mut children: Vec<AnyElement> = Vec::new();
        let mut row_children = vec![None; rows.len()];
        for (heading, (category, shown)) in sections(rows).into_iter().enumerate() {
            children.push(
                div()
                    .id(("heading", heading))
                    .role(Role::Heading)
                    .aria_level(2)
                    .aria_label(category)
                    .pt(rems(1.14))
                    .pb(rems(0.57))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(category)
                    .into_any_element(),
            );
            for (pos, &ix) in shown.iter().enumerate() {
                row_children[ix] = Some(children.len());
                let edges = (pos == 0, pos == shown.len() - 1);
                children.push(
                    self.render_row(ix, edges, scanning, locked, window, cx)
                        .into_any_element(),
                );
            }
        }
        self.row_children = Rc::new(row_children);

        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("checklist")
                    .role(Role::List)
                    .aria_label("Targets")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.list)
                    .px(rems(1.43))
                    .pb(rems(1.14))
                    .children(children),
            )
            .children(self.render_scrollbar(t, cx))
    }

    fn render_row(
        &self,
        ix: usize,
        (first, last): (bool, bool),
        scanning: bool,
        locked: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = self.system.theme;
        let row = &self.controller.rows()[ix];
        let active = self.controller.scanning_target() == Some(row.target.id);
        let status = row_status(row, scanning, active, &t);
        let focus = &self.row_focus[ix];
        let ring = !locked && shows_focus(focus, window);
        let id = row.target.id;
        let spoken = match &status.detail {
            Some(detail) => format!("{}. {detail}", status.value),
            None => status.value.clone(),
        };

        div()
            .id(("row", ix))
            // Disabled controls leave the tab order but keep focus they already have,
            // so keyboard navigation continues from them.
            .track_focus(&focus.clone().tab_stop(!locked))
            .role(Role::CheckBox)
            .aria_label(row.target.name)
            .aria_toggled(if row.selected {
                Toggled::True
            } else {
                Toggled::False
            })
            .aria_description(spoken)
            .when(locked, disabled)
            .relative()
            .flex()
            .items_center()
            .gap(rems(0.86))
            .px(rems(1.14))
            .py(rems(0.71))
            .min_h(rems(3.29))
            .bg(t.card)
            .border_color(t.card_stroke)
            .border_x_1()
            .when(first, |d| d.border_t_1().rounded_t(px(7.)))
            .when(last, |d| d.border_b_1().rounded_b(px(7.)))
            .when(!first, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(1.))
                        .bg(t.divider),
                )
            })
            .when(!locked, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(t.card_hover))
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        // In Windows, Space toggles a check box and Enter does not.
                        if !matches!(
                            event,
                            ClickEvent::Keyboard(KeyboardClickEvent {
                                button: KeyboardButton::Enter,
                                ..
                            })
                        ) {
                            this.toggle(id, cx);
                        }
                    }))
                    // GPUI's default accessible click presses the row's center, which can
                    // land on another control when the row is scrolled out of view.
                    .on_a11y_action(AccessibleAction::Click, {
                        let view = cx.entity().downgrade();
                        move |_, _, cx| {
                            view.update(cx, |this, cx| this.toggle(id, cx)).ok();
                        }
                    })
            })
            .child(self.checkbox(row.selected, !locked))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_color(if locked { t.text_disabled } else { t.text })
                            .child(row.target.name),
                    )
                    .when_some(status.detail, |d, detail| {
                        d.child(
                            div()
                                .text_size(rems(0.86))
                                .text_color(status.detail_color)
                                .child(detail),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(rems(0.43))
                    .text_color(status.value_color)
                    .when(active, |d| {
                        d.child(self.pulsing(
                            ("scan-indicator", ix),
                            div().size(rems(0.43)).rounded_full().bg(t.accent),
                        ))
                    })
                    .when_some(status.icon, |d, (glyph, color)| {
                        d.child(icon(glyph, rems(1.), color))
                    })
                    .child(status.value),
            )
            .when(ring, |d| d.child(focus_ring(&t, px(1.), px(4.))))
    }

    fn render_footer(
        &self,
        summary: &Summary,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = self.system.theme;
        let enabled = self.controller.can_clean();
        // Use the same current Scan estimate as the header.
        let label = match summary.selected_bytes {
            Some(bytes) if bytes > 0 => format!("Clean approximately {}", format::size(bytes)),
            _ => "Clean".to_string(),
        };
        let status = status_line(&self.controller, summary);

        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(rems(0.71))
            .px(rems(1.43))
            .pt(rems(0.86))
            .pb(rems(1.14))
            .border_t_1()
            .border_color(t.divider)
            .child(
                // Reserve two lines even when empty so status changes cannot resize the checklist.
                // Keep the full message available to scrolling and screen readers.
                div()
                    .id("result")
                    .role(Role::Status)
                    .aria_label(status.as_ref().map(|s| s.text.clone()).unwrap_or_default())
                    .a11y_synthetic_children(|b| b.parent_node().set_live(accesskit::Live::Polite))
                    .flex()
                    .flex_none()
                    .h(rems(2.86))
                    .items_center()
                    .gap(rems(0.43))
                    .when_some(status, |d, status| {
                        d.when(status.error, |d| {
                            d.text_color(t.critical)
                                .child(icon(ICON_ERROR, rems(1.), t.critical))
                        })
                        .child(
                            div()
                                .id("status-text")
                                .flex_1()
                                .min_w_0()
                                .max_h_full()
                                .overflow_y_scroll()
                                .child(status.text),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rems(1.14))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(rems(0.86))
                            .text_color(t.text_secondary)
                            .child("Clean permanently deletes eligible files. Reclaimed space may differ from estimates."),
                    )
                    .child(
                        self.button("clean", &self.clean_focus, enabled, window)
                            .aria_label(label.clone())
                            .min_h(rems(2.29))
                            .min_w(rems(8.57))
                            .px(rems(0.86))
                            .when(enabled, |b| {
                                b.bg(t.accent)
                                    .text_color(t.on_accent)
                                    .hover(|s| s.bg(t.accent_hover))
                                    .active(|s| s.bg(t.accent_pressed))
                                    .on_click(cx.listener(|this, _, _, cx| this.clean(cx)))
                            })
                            .when(!enabled, |b| {
                                b.bg(t.accent_disabled).text_color(t.on_accent_disabled)
                            })
                            .when(t.high_contrast, |b| {
                                b.border_1().border_color(if enabled {
                                    t.on_accent
                                } else {
                                    t.text_disabled
                                })
                            })
                            .child(label),
                    ),
            )
    }
}

impl Render for CleanerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Windows' body text is 14px; scale it with the Windows text size setting.
        window.set_rem_size(px(14.) * self.system.text_scale);
        let t = self.system.theme;
        let summary = self.controller.summary();
        div()
            .id("root")
            .track_focus(&self.root_focus)
            .on_action(cx.listener(|_, _: &FocusNext, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &FocusPrev, window, cx| window.focus_prev(cx)))
            .on_mouse_move(cx.listener(|this, e, _, cx| this.drag_thumb(e, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.thumb_drag = None;
                    cx.notify();
                }),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .font_family(TEXT_FONT)
            .text_size(rems(1.))
            .line_height(rems(1.43))
            .child(self.render_header(&summary, window, cx))
            .child(self.render_list(summary.scanning, window, cx))
            .child(self.render_footer(&summary, window, cx))
    }
}
