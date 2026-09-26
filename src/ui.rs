//! The single cleaner screen: the estimated total and Rescan on top, the
//! scrolling checklist in the middle, and Clean with the last result at the
//! bottom. It renders controller state and forwards user actions.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use futures::StreamExt;
use gpui::{
    AccessibleAction, AnyElement, App, AppContext, ClickEvent, Context, Div, FocusHandle,
    FontWeight, Hsla, InteractiveElement, IntoElement, KeyboardButton, KeyboardClickEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Render, Role, ScrollHandle,
    Size, Stateful, StatefulInteractiveElement, Styled, Subscription, Task, Toggled, Window,
    accesskit, actions, div, point, prelude::FluentBuilder, px, rems, size,
};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetWindowLongW, GetWindowRect, SWP_NOACTIVATE, SWP_NOZORDER,
    SetWindowPos, WINDOW_EX_STYLE, WINDOW_STYLE,
};

use crate::controller::{CleanProgress, Command, Controller, Event, OpId, Operation, Row};
use crate::fixture::Fixture;
use crate::format;
use crate::results::{CleanStatus, ScanResult};
use crate::targets::{Category, TargetId};
use crate::theme::{self, System, Theme};

actions!(cleaner, [FocusNext, FocusPrev]);

// Windows 11 system fonts.
const TEXT_FONT: &str = "Segoe UI Variable Text";
const DISPLAY_FONT: &str = "Segoe UI Variable Display";
const ICON_FONT: &str = "Segoe Fluent Icons";

// Segoe Fluent Icons code points.
const ICON_CHECK: &str = "\u{E73E}";
const ICON_REFRESH: &str = "\u{E72C}";
const ICON_WARNING: &str = "\u{E7BA}";
const ICON_ERROR: &str = "\u{EA39}";

/// Window size at 100% text size, in device-independent pixels.
const BASE_SIZE: Size<Pixels> = Size {
    width: px(520.),
    height: px(640.),
};

/// The window's content size, grown with the Windows text size.
pub fn window_size(text_scale: f32) -> Size<Pixels> {
    size(BASE_SIZE.width * text_scale, BASE_SIZE.height * text_scale)
}

/// Sets the window's content size, keeping its center where it was and its frame
/// inside the monitor's work area.
fn fit_window(window: &Window, content: Size<Pixels>) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(handle.hwnd.get() as *mut _);
    let scale = window.scale_factor();
    // SAFETY: `hwnd` is this window's live handle, and every out-pointer refers to a local.
    unsafe {
        let mut frame = RECT {
            right: (f32::from(content.width) * scale).round() as i32,
            bottom: (f32::from(content.height) * scale).round() as i32,
            ..Default::default()
        };
        let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32);
        let ex_style = WINDOW_EX_STYLE(GetWindowLongW(hwnd, GWL_EXSTYLE) as u32);
        let _ = AdjustWindowRectExForDpi(&mut frame, style, false, ex_style, GetDpiForWindow(hwnd));
        let mut current = RECT::default();
        let _ = GetWindowRect(hwnd, &mut current);
        let mut monitor = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(
            MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        );
        let work = monitor.rcWork;
        let width = (frame.right - frame.left).min(work.right - work.left);
        let height = (frame.bottom - frame.top).min(work.bottom - work.top);
        let x = ((current.left + current.right - width) / 2).clamp(work.left, work.right - width);
        let y = ((current.top + current.bottom - height) / 2).clamp(work.top, work.bottom - height);
        let _ = SetWindowPos(
            hwnd,
            None,
            x,
            y,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// The running operation's fixture task and its stop signal.
struct Worker {
    op: OpId,
    stop: Rc<Cell<bool>>,
    _task: Task<()>,
}

pub struct CleanerView {
    controller: Controller,
    fixture: Fixture,
    worker: Option<Worker>,
    system: System,
    root_focus: FocusHandle,
    rescan_focus: FocusHandle,
    clean_focus: FocusHandle,
    /// One per controller row, in the same order.
    row_focus: Vec<FocusHandle>,
    /// Each row's child index in the checklist from the last render, if shown.
    row_children: Rc<Vec<Option<usize>>>,
    list: ScrollHandle,
    /// Scrollbar drag start: pointer y and scroll distance.
    thumb_drag: Option<(Pixels, Pixels)>,
    _watcher: Option<theme::Watcher>,
    _subscriptions: Vec<Subscription>,
}

impl CleanerView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (changed_tx, mut changed) = futures::channel::mpsc::unbounded();
        cx.spawn_in(window, async move |this, cx| {
            while changed.next().await.is_some() {
                let refreshed =
                    this.update_in(cx, |view, window, cx| view.refresh_system(window, cx));
                if refreshed.is_err() {
                    break;
                }
            }
        })
        .detach();

        let controller = Controller::new(|target| target.default_selected);
        let row_focus: Vec<FocusHandle> = controller
            .rows()
            .iter()
            .map(|_| cx.focus_handle())
            .collect();
        let list = ScrollHandle::new();
        let row_children: Rc<Vec<Option<usize>>> = Rc::default();
        // Keep a row visible whenever it gains focus, from Tab or from assistive technology.
        let subscriptions = row_focus
            .iter()
            .enumerate()
            .map(|(ix, handle)| {
                // Focus listeners run after a frame is drawn, so request another to apply the scroll.
                cx.on_focus(handle, window, move |view, _, cx| {
                    if let Some(child) = view.row_children.get(ix).copied().flatten() {
                        view.list.scroll_to_item(child);
                        cx.notify();
                    }
                })
            })
            .collect();

        let system = System::read();
        // Keep a large text size from pushing the window off-screen.
        fit_window(window, window_size(system.text_scale));
        let root_focus = cx.focus_handle();
        window.focus(&root_focus, cx);
        let mut view = Self {
            controller,
            fixture: Fixture::default(),
            worker: None,
            system,
            root_focus,
            rescan_focus: cx.focus_handle(),
            clean_focus: cx.focus_handle(),
            row_focus,
            row_children,
            list,
            thumb_drag: None,
            _watcher: theme::watch(changed_tx).ok(),
            _subscriptions: subscriptions,
        };
        let scan = view.controller.start_scan();
        view.dispatch(scan, cx);
        view
    }

    fn refresh_system(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let system = System::read();
        if system == self.system {
            return;
        }
        if system.text_scale != self.system.text_scale {
            fit_window(window, window_size(system.text_scale));
        }
        self.system = system;
        cx.notify();
    }

    /// Hands a controller command to the fixture worker.
    fn dispatch(&mut self, command: Option<Command>, cx: &mut Context<Self>) {
        match command {
            None => {}
            Some(Command::Stop(op)) => {
                if let Some(worker) = self.worker.as_ref().filter(|w| w.op == op) {
                    worker.stop.set(true);
                }
            }
            Some(Command::Scan { op, targets }) => self.start_worker(op, targets, false, cx),
            Some(Command::Clean { op, targets }) => self.start_worker(op, targets, true, cx),
        }
    }

    /// Plays fixture results for `targets` in order, one after another, and
    /// checks the stop signal between steps.
    fn start_worker(
        &mut self,
        op: OpId,
        targets: Vec<TargetId>,
        clean: bool,
        cx: &mut Context<Self>,
    ) {
        let stop = Rc::new(Cell::new(false));
        let task_stop = stop.clone();
        let task = cx.spawn(async move |this, cx| {
            for id in targets {
                if task_stop.get() {
                    break;
                }
                let Ok((delay, event)) = this.update(cx, |view, cx| {
                    if clean {
                        view.apply(op, Event::Cleaning(id), cx);
                        let estimate = view.controller.estimate(id).unwrap_or(0);
                        let (delay, result) = view.fixture.clean(id, estimate);
                        (delay, Event::Cleaned(id, result))
                    } else {
                        let (delay, result) = view.fixture.scan(id);
                        (delay, Event::Scanned(id, result))
                    }
                }) else {
                    return;
                };
                // Wait in small steps so a stop takes effect mid-Target.
                let step = Duration::from_millis(50);
                let mut waited = Duration::ZERO;
                while waited < delay && !task_stop.get() {
                    cx.background_executor().timer(step).await;
                    waited += step;
                }
                if task_stop.get() {
                    break;
                }
                this.update(cx, |view, cx| view.apply(op, event, cx)).ok();
            }
            this.update(cx, |view, cx| view.apply(op, Event::Finished, cx))
                .ok();
        });
        self.worker = Some(Worker {
            op,
            stop,
            _task: task,
        });
    }

    fn apply(&mut self, op: OpId, event: Event, cx: &mut Context<Self>) {
        let next = self.controller.apply(op, event);
        cx.notify();
        self.dispatch(next, cx);
    }

    fn toggle(&mut self, id: TargetId, cx: &mut Context<Self>) {
        if self.controller.toggle(id) {
            cx.notify();
        }
    }

    fn rescan(&mut self, cx: &mut Context<Self>) {
        let command = self.controller.start_scan();
        self.dispatch(command, cx);
        cx.notify();
    }

    fn clean(&mut self, cx: &mut Context<Self>) {
        let command = self.controller.request_clean();
        self.dispatch(command, cx);
        cx.notify();
    }

    fn render_header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.system.theme;
        let totals = self.controller.totals();
        let rows = self.controller.rows();
        let caption = if totals.scanning {
            "Found so far"
        } else {
            "Estimated total"
        };
        let total = format::size(totals.complete_bytes);
        let note = if totals.scanning {
            let scanned = rows.iter().filter(|r| r.scan.is_some()).count();
            format!("Scanning… {scanned} of {} Targets scanned", rows.len())
        } else if totals.incomplete > 0 {
            format!(
                "Excludes {} with incomplete results",
                format::plural(totals.incomplete as u64, "Target")
            )
        } else {
            "Scan complete".to_string()
        };
        let idle = self.controller.operation() == Operation::Idle;
        let scrolled = self.list.offset().y < px(0.);

        div()
            .flex()
            .items_start()
            .justify_between()
            .gap(rems(1.))
            .px(rems(1.43))
            .pt(rems(1.14))
            .pb(rems(0.86))
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
                    // Labels take their accessible name from their value.
                    .aria_value(format!("{caption}: {total}. {note}"))
                    .flex()
                    .flex_col()
                    .child(div().text_color(t.text_secondary).child(caption))
                    .child(
                        div()
                            .font_family(DISPLAY_FONT)
                            .text_size(rems(2.))
                            .line_height(rems(2.57))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(total),
                    )
                    .child(div().text_color(t.text_secondary).child(note)),
            )
            .child(
                self.subtle_button("rescan", &self.rescan_focus, idle, window)
                    .aria_label("Rescan")
                    .tooltip(tooltip("Rescan", t))
                    .size(rems(2.29))
                    .when(idle, |b| {
                        b.on_click(cx.listener(|this, _, _, cx| this.rescan(cx)))
                    })
                    .child(self.icon(
                        ICON_REFRESH,
                        rems(1.14),
                        if idle { t.text } else { t.text_disabled },
                    )),
            )
    }

    fn render_list(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.system.theme;
        let scanning = self.controller.totals().scanning;
        let locked = self.controller.selection_locked();
        let rows = self.controller.rows();
        let mut children: Vec<AnyElement> = Vec::new();
        let mut row_children = vec![None; rows.len()];
        for category in Category::ALL {
            let shown: Vec<usize> = (0..rows.len())
                .filter(|&ix| rows[ix].target.category == category && !is_hidden(&rows[ix]))
                .collect();
            if shown.is_empty() {
                continue;
            }
            children.push(
                div()
                    .id(("heading", category as usize))
                    .role(Role::Heading)
                    .aria_level(2)
                    .aria_label(category.name())
                    .pt(rems(1.14))
                    .pb(rems(0.57))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(category.name())
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
        let status = row_status(row, scanning, &t);
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
            .py(rems(0.57))
            .min_h(rems(3.))
            .bg(t.card)
            .border_color(t.card_stroke)
            .border_x_1()
            .when(first, |d| d.border_t_1().rounded_t(px(4.)))
            .when(last, |d| d.border_b_1().rounded_b(px(4.)))
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
                    .when_some(status.icon, |d, (glyph, color)| {
                        d.child(self.icon(glyph, rems(1.), color))
                    })
                    .child(status.value),
            )
            .when(ring, |d| d.child(focus_ring(&t, px(1.), px(4.))))
    }

    fn render_scrollbar(&self, t: Theme, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let viewport = self.list.bounds().size.height;
        let max = self.list.max_offset().y;
        if max <= px(0.) || viewport <= px(0.) {
            return None;
        }
        let thumb = thumb_length(viewport, max);
        let scrolled = -self.list.offset().y;
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

    fn drag_thumb(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
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

    fn render_footer(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.system.theme;
        let totals = self.controller.totals();
        let enabled = self.controller.can_clean();
        let label = if totals.selected_bytes > 0 {
            format!(
                "Clean approximately {}",
                format::size(totals.selected_bytes)
            )
        } else {
            "Clean".to_string()
        };
        let ring = enabled && shows_focus(&self.clean_focus, window);

        div()
            .flex()
            .flex_col()
            .gap(rems(0.71))
            .px(rems(1.43))
            .pt(rems(0.86))
            .pb(rems(1.14))
            .border_t_1()
            .border_color(t.divider)
            .child(
                // A polite live region, so screen readers announce Clean progress and results.
                div()
                    .id("result")
                    .role(Role::Status)
                    .aria_label(self.status_text().unwrap_or_default())
                    .a11y_synthetic_children(|b| b.parent_node().set_live(accesskit::Live::Polite))
                    .when_some(self.status_text(), |d, text| d.child(text)),
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
                            .child("Sizes are estimates. Deleted file sizes can differ from the disk space reclaimed."),
                    )
                    .child(
                        div()
                            .id("clean")
                            .track_focus(&self.clean_focus.clone().tab_stop(enabled))
                            .role(Role::Button)
                            .aria_label(label.clone())
                            .when(!enabled, disabled)
                            .relative()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .min_h(rems(2.29))
                            .min_w(rems(8.57))
                            .px(rems(0.86))
                            .rounded(px(4.))
                            .when(enabled, |b| {
                                b.bg(t.accent)
                                    .text_color(t.on_accent)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(t.accent_hover))
                                    .active(|s| s.bg(t.accent_pressed))
                                    .on_click(cx.listener(|this, _, _, cx| this.clean(cx)))
                            })
                            .when(!enabled, |b| {
                                b.bg(t.accent_disabled).text_color(t.on_accent_disabled)
                            })
                            .when(t.high_contrast, |b| {
                                b.border_1().border_color(if enabled { t.on_accent } else { t.text_disabled })
                            })
                            .child(label)
                            .when(ring, |b| b.child(focus_ring(&t, px(-3.), px(7.)))),
                    ),
            )
    }

    /// The line under the checklist: handoff and Clean progress, then the last result.
    fn status_text(&self) -> Option<String> {
        let rows = self.controller.rows();
        if self.controller.selection_locked() {
            let queued = rows.iter().filter(|r| r.clean.is_some()).count();
            if queued == 0 {
                return Some("Stopping the Scan to start Clean…".into());
            }
            let done = rows
                .iter()
                .filter(|r| matches!(r.clean, Some(CleanProgress::Done(_))))
                .count();
            return Some(format!("Cleaning… {done} of {queued} Targets finished"));
        }
        self.controller.last_clean().map(format::clean_summary)
    }

    fn checkbox(&self, checked: bool, enabled: bool) -> Div {
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
                .child(self.icon(ICON_CHECK, rems(0.86), t.on_accent)),
            (true, false) => b
                .bg(t.accent_disabled)
                .when(t.high_contrast, |b| {
                    b.border_1().border_color(t.text_disabled)
                })
                .child(self.icon(ICON_CHECK, rems(0.86), t.on_accent_disabled)),
            (false, enabled) => b.bg(t.checkbox_fill).border_1().border_color(if enabled {
                t.checkbox_stroke
            } else {
                t.text_disabled
            }),
        }
    }

    fn subtle_button(
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
            .when(enabled, |b| {
                b.cursor_pointer()
                    .hover(|s| s.bg(t.subtle_hover))
                    .active(|s| s.bg(t.subtle_pressed))
            })
            .when(ring, |b| b.child(focus_ring(&t, px(-3.), px(7.))))
    }

    fn icon(&self, glyph: &'static str, size: gpui::Rems, color: Hsla) -> Div {
        div()
            .font_family(ICON_FONT)
            .text_size(size)
            .line_height(size)
            .text_color(color)
            .child(glyph)
    }
}

impl Render for CleanerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Windows' body text is 14px; scale it with the Windows text size setting.
        window.set_rem_size(px(14.) * self.system.text_scale);
        let t = self.system.theme;
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
            .child(self.render_header(window, cx))
            .child(self.render_list(window, cx))
            .child(self.render_footer(window, cx))
    }
}

fn is_hidden(row: &Row) -> bool {
    row.scan == Some(ScanResult::NotPresent) && row.clean.is_none()
}

struct RowStatus {
    value: String,
    value_color: Hsla,
    icon: Option<(&'static str, Hsla)>,
    detail: Option<String>,
    detail_color: Hsla,
}

/// What a row shows on the right, and the optional explanation under its name.
fn row_status(row: &Row, scanning: bool, t: &Theme) -> RowStatus {
    let mut s = RowStatus {
        value: String::new(),
        value_color: t.text_secondary,
        icon: None,
        detail: None,
        detail_color: t.text_secondary,
    };
    let blocked = |detail: String| {
        if row.selected {
            format!("{detail} · Untick or rescan to Clean")
        } else {
            detail
        }
    };
    match (&row.clean, &row.scan) {
        (Some(CleanProgress::Queued), _) => s.value = "Queued".into(),
        (Some(CleanProgress::Cleaning), _) => {
            s.value = "Cleaning…".into();
            s.value_color = t.text;
        }
        (Some(CleanProgress::Done(result)), _) => {
            let deleted = result
                .deleted_bytes
                .map(|b| format!("Deleted {}", format::size(b)));
            let mut notes: Vec<String> = result
                .skipped
                .iter()
                .map(|&(problem, count)| {
                    format!(
                        "{} skipped ({})",
                        format::plural(count, "file"),
                        problem.text()
                    )
                })
                .collect();
            if let Some(problem) = result.coverage_problem {
                notes.push(match result.status {
                    CleanStatus::Failed => sentence(problem.text()),
                    _ => format!("Incomplete: {}", problem.text()),
                });
            }
            match result.status {
                CleanStatus::Complete => {
                    s.icon = Some((ICON_CHECK, t.success));
                    s.value = deleted.unwrap_or_else(|| "Emptied (size unavailable)".into());
                }
                CleanStatus::Partial => {
                    s.icon = Some((ICON_WARNING, t.caution));
                    s.value = deleted.unwrap_or_else(|| "Partly cleaned".into());
                }
                CleanStatus::Failed => {
                    s.icon = Some((ICON_ERROR, t.critical));
                    s.value = "Clean failed".into();
                }
                CleanStatus::Stopped => {
                    s.value = "Stopped".into();
                    notes.extend(deleted.map(|d| format!("{d} before stopping")));
                }
            }
            s.detail = (!notes.is_empty()).then(|| notes.join(" · "));
        }
        (None, None) => {
            s.value = if scanning {
                "Scanning…"
            } else {
                "Not scanned"
            }
            .into();
            s.value_color = t.text_disabled;
        }
        (None, Some(ScanResult::Complete { bytes })) => s.value = format::size(*bytes),
        (None, Some(ScanResult::NotPresent)) => s.value = "Not present".into(),
        (None, Some(ScanResult::Partial { bytes, problem })) => {
            s.icon = Some((ICON_WARNING, t.caution));
            s.value = format!("{} (incomplete)", format::size(*bytes));
            s.detail = Some(blocked(sentence(problem.text())));
            s.detail_color = t.caution;
        }
        (None, Some(ScanResult::Failed { problem })) => {
            s.icon = Some((ICON_ERROR, t.critical));
            s.value = "Scan failed".into();
            s.detail = Some(blocked(sentence(problem.text())));
            s.detail_color = t.critical;
        }
        (None, Some(ScanResult::Stopped)) => {
            s.value = "Not scanned".into();
            s.detail = Some(blocked("Scan stopped".into()));
        }
    }
    s
}

fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The scrollbar thumb's length for a viewport and its maximum scroll distance.
fn thumb_length(viewport: Pixels, max_scroll: Pixels) -> Pixels {
    (viewport * (viewport / (viewport + max_scroll))).max(px(24.))
}

/// Marks an element disabled for assistive technology.
fn disabled(el: Stateful<Div>) -> Stateful<Div> {
    el.a11y_synthetic_children(|b| b.parent_node().set_disabled())
}

/// Whether `focus` holds focus that arrived from the keyboard, so it needs a focus rectangle.
fn shows_focus(focus: &FocusHandle, window: &Window) -> bool {
    focus.is_focused(window) && window.last_input_was_keyboard()
}

/// A Windows-style keyboard focus rectangle. `inset` is negative to draw outside the element.
fn focus_ring(t: &Theme, inset: Pixels, radius: Pixels) -> Div {
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

fn tooltip(text: &'static str, theme: Theme) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView {
    move |_, cx| cx.new(|_| Tooltip { text, theme }).into()
}

struct Tooltip {
    text: &'static str,
    theme: Theme,
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div()
            .font_family(TEXT_FONT)
            .px(rems(0.57))
            .py(rems(0.29))
            .rounded(px(4.))
            .bg(t.card)
            .border_1()
            .border_color(t.card_stroke)
            .text_color(t.text)
            .text_size(rems(0.86))
            .child(self.text)
    }
}
