//! The single cleaner screen: the selected estimate and Scan on top, the
//! scrolling checklist in the middle, and Clean with the last result at the
//! bottom. It renders controller state and forwards user actions.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, SystemTime};

use futures::StreamExt;
use gpui::{
    AccessibleAction, Animation, AnimationExt, AnyElement, App, ClickEvent, Context, Div,
    ElementId, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement, KeyboardButton,
    KeyboardClickEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Render,
    Role, ScrollHandle, Size, Stateful, StatefulInteractiveElement, Styled, Subscription, Toggled,
    Window, accesskit, actions, div, point, prelude::FluentBuilder, pulsating_between, px, rems,
    size,
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

use crate::controller::{CleanProgress, Command, Controller, OpId, Row};
use crate::core::recycle_bin::SystemShell;
use crate::core::{self, Roots};
use crate::format;
use crate::results::{CleanStatus, Event, ScanResult};
use crate::selection::{self, Choices, SelectionStore};
use crate::targets::{self, Category, TargetId};
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
///
/// The resize runs in a later task, as GPUI's own `Window::resize` does. Windows
/// delivers `WM_SIZE` during `SetWindowPos`, and GPUI drops that notification while
/// the window is being updated, which would leave the layout at the old size.
fn fit_window(window: &Window, content: Size<Pixels>, cx: &App) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(handle.hwnd.get() as *mut _);
    let scale = window.scale_factor();
    cx.foreground_executor()
        .spawn(async move { set_window_content_size(hwnd, content, scale) })
        .detach();
}

/// Resizes `hwnd` so its client area is `content` at `scale`, centered on its
/// current position and clamped to the monitor's work area.
fn set_window_content_size(hwnd: HWND, content: Size<Pixels>, scale: f32) {
    // SAFETY: every out-pointer refers to a local. A handle is not memory, so if
    // the window closed before this task ran, the calls fail without effect.
    unsafe {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "window sizes in device pixels are far below i32::MAX"
        )]
        let mut frame = RECT {
            right: (f32::from(content.width) * scale).round() as i32,
            bottom: (f32::from(content.height) * scale).round() as i32,
            ..Default::default()
        };
        // The style values are bit flags, so reinterpret the bits.
        let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE).cast_unsigned());
        let ex_style = WINDOW_EX_STYLE(GetWindowLongW(hwnd, GWL_EXSTYLE).cast_unsigned());
        let _ = AdjustWindowRectExForDpi(&mut frame, style, false, ex_style, GetDpiForWindow(hwnd));
        let mut current = RECT::default();
        let _ = GetWindowRect(hwnd, &mut current);
        let mut monitor = MONITORINFO {
            cbSize: size_of::<MONITORINFO>().try_into().unwrap(),
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

/// The running filesystem worker and its stop signal.
struct Worker {
    op: OpId,
    stop: Arc<AtomicBool>,
}

enum SelectionEvent {
    Loaded(Result<selection::Loaded, String>),
    Saved(Result<(), String>),
}

pub struct CleanerView {
    controller: Controller,
    save_tx: Sender<Choices>,
    worker: Option<Worker>,
    system: System,
    root_focus: FocusHandle,
    scan_focus: FocusHandle,
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

        let (save_tx, save_rx) = std::sync::mpsc::channel::<Choices>();
        let (selection_tx, mut selection_rx) = futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let store = SelectionStore::system().map_err(|error| format::error(&error));
            let loaded = store
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|store| store.load().map_err(|error| format::error(&error)));
            selection_tx
                .unbounded_send(SelectionEvent::Loaded(loaded))
                .ok();
            for choices in save_rx {
                let result = store
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|store| store.save(&choices).map_err(|error| format::error(&error)));
                selection_tx
                    .unbounded_send(SelectionEvent::Saved(result))
                    .ok();
            }
        });
        cx.spawn(async move |this, cx| {
            while let Some(event) = selection_rx.next().await {
                if this
                    .update_in(cx, |view, window, cx| {
                        view.apply_selection(event, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let controller = Controller::new();
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
        fit_window(window, window_size(system.text_scale), cx);
        let root_focus = cx.focus_handle();
        window.focus(&root_focus, cx);
        let view = Self {
            controller,
            save_tx,
            worker: None,
            system,
            root_focus,
            scan_focus: cx.focus_handle(),
            clean_focus: cx.focus_handle(),
            row_focus,
            row_children,
            list,
            thumb_drag: None,
            _watcher: theme::watch(changed_tx).ok(),
            _subscriptions: subscriptions,
        };
        let entity = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            entity
                .update(cx, |view, cx| view.request_close(cx))
                .unwrap_or(true)
        });
        view
    }

    fn refresh_system(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let system = System::read();
        if system == self.system {
            return;
        }
        if system.text_scale != self.system.text_scale {
            fit_window(window, window_size(system.text_scale), cx);
        }
        self.system = system;
        cx.notify();
    }

    /// Hands a controller command to the filesystem worker.
    fn dispatch(&mut self, command: Option<Command>, cx: &mut Context<Self>) {
        match command {
            None => {}
            Some(Command::Stop(op)) => {
                if let Some(worker) = self.worker.as_ref().filter(|w| w.op == op) {
                    worker.stop.store(true, Ordering::Release);
                }
            }
            Some(Command::Scan { op, targets }) => {
                self.start_worker(op, cx, move |roots, time, stop, report| {
                    core::scan_targets(&targets, roots, &SystemShell, time, stop, report)
                });
            }
            Some(Command::Clean {
                op,
                targets,
                drives,
            }) => {
                self.start_worker(op, cx, move |roots, time, stop, report| {
                    core::clean_targets(&targets, &drives, roots, &SystemShell, time, stop, report)
                });
            }
        }
    }

    /// Runs `work` on a filesystem worker thread with the system roots and the
    /// operation time, applying each Event it reports.
    fn start_worker(
        &mut self,
        op: OpId,
        cx: &mut Context<Self>,
        work: impl FnOnce(&Roots, SystemTime, &AtomicBool, &mut dyn FnMut(Event)) + Send + 'static,
    ) {
        let stop = Arc::new(AtomicBool::new(false));
        let task_stop = stop.clone();
        let (tx, mut updates) = futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            work(
                &Roots::system(),
                SystemTime::now(),
                &task_stop,
                &mut |event| {
                    tx.unbounded_send(event).ok();
                },
            );
        });
        cx.spawn(async move |this, cx| {
            while let Some(event) = updates.next().await {
                let finished = matches!(event, Event::Finished);
                if this
                    .update_in(cx, |view, window, cx| view.apply(op, event, window, cx))
                    .is_err()
                {
                    break;
                }
                if finished {
                    break;
                }
            }
        })
        .detach();
        self.worker = Some(Worker { op, stop });
    }

    fn apply(&mut self, op: OpId, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(event, Event::Finished) {
            self.worker = None;
        }
        let next = self.controller.apply(op, event);
        cx.notify();
        self.dispatch(next, cx);
        if self.controller.ready_to_exit() {
            window.remove_window();
        }
    }

    fn apply_selection(
        &mut self,
        event: SelectionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SelectionEvent::Loaded(loaded) => {
                let save = self.controller.load_selection(loaded);
                self.send_save(save);
            }
            SelectionEvent::Saved(result) => self.controller.save_finished(result),
        }
        cx.notify();
        if self.controller.ready_to_exit() {
            window.remove_window();
        }
    }

    /// Hands Choices the controller queued to the Selection store thread.
    fn send_save(&mut self, choices: Option<Choices>) {
        if let Some(choices) = choices
            && self.save_tx.send(choices).is_err()
        {
            self.controller
                .save_finished(Err("the save worker stopped".into()));
        }
    }

    fn toggle(&mut self, id: TargetId, cx: &mut Context<Self>) {
        let save = self.controller.toggle(id);
        if save.is_some() {
            self.send_save(save);
            cx.notify();
        }
    }

    fn request_close(&mut self, cx: &mut Context<Self>) -> bool {
        let stop = self.controller.close();
        self.dispatch(stop, cx);
        cx.notify();
        self.controller.ready_to_exit()
    }

    fn scan(&mut self, cx: &mut Context<Self>) {
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
        let found = rows
            .iter()
            .filter(|row| matches!(row.scan, Some(ScanResult::Complete { bytes }) if bytes > 0))
            .count();
        let selected = visible_selection_count(rows);
        let unscanned = !totals.scanning && never_scanned(rows);
        let overview = if totals.scanning {
            match self.controller.scanning_target().and_then(targets::find) {
                Some(target) => format!("Scanning {}…", target.name),
                None => "Scanning…".to_string(),
            }
        } else if unscanned {
            "Scan to estimate the space you can reclaim".to_string()
        } else if found == 0 && totals.incomplete == 0 {
            "No eligible files found".to_string()
        } else if found == 0 {
            "No complete estimates available".to_string()
        } else {
            format!(
                "{} estimated across {}",
                format::size(totals.complete_bytes),
                format::plural(found as u64, "Target")
            )
        };
        let scan_note = if totals.scanning {
            let scanned = rows.iter().filter(|r| r.scan.is_some()).count();
            format!("{scanned} of {} scanned", rows.len())
        } else if unscanned {
            "Not scanned yet".to_string()
        } else if totals.incomplete > 0 {
            format!(
                "Excludes {} with incomplete results",
                format::plural(totals.incomplete as u64, "Target")
            )
        } else {
            "Scan complete".to_string()
        };
        let note = format!(
            "{} selected · {scan_note}",
            format::plural(selected as u64, "Target")
        );
        let estimate = selected_estimate(rows, totals.scanning);
        let pending = estimate.pending;
        let selected_estimate = estimate.bytes.filter(|_| !unscanned).map(format::size);
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
                    // Labels take their accessible name from their value.
                    .aria_value(format!(
                        "Selected estimate: {}. {overview}. {note}",
                        selected_estimate.as_deref().unwrap_or(if unscanned {
                            "not scanned"
                        } else {
                            "calculating"
                        })
                    ))
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
                                match selected_estimate {
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
                self.subtle_button("scan", &self.scan_focus, idle, window)
                    .aria_label("Scan")
                    .gap(rems(0.43))
                    .px(rems(0.71))
                    .h(rems(2.43))
                    .border_1()
                    .border_color(t.card_stroke)
                    .bg(t.card)
                    .text_color(if idle { t.text } else { t.text_disabled })
                    .when(idle, |b| {
                        b.on_click(cx.listener(|this, _, _, cx| this.scan(cx)))
                    })
                    .child(self.icon(
                        ICON_REFRESH,
                        rems(1.14),
                        if idle { t.text } else { t.text_disabled },
                    ))
                    .child("Scan"),
            )
    }

    fn render_list(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.system.theme;
        let scanning = self.controller.totals().scanning;
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
        let active = self.controller.scanning_target() == Some(row.target.id);
        let status = row_status(row, scanning, active, &t);
        let focus = &self.row_focus[ix];
        let ring = !locked && shows_focus(focus, window);
        let id = row.target.id;
        let mut spoken = match &status.detail {
            Some(detail) => format!("{}. {detail}", status.value),
            None => status.value.clone(),
        };
        if status.updating {
            spoken.push_str(". Updating");
        }

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
        // The header's estimate, so the label stays put while a Scan updates it.
        let label = match selected_estimate(self.controller.rows(), totals.scanning).bytes {
            Some(bytes) if bytes > 0 => format!("Clean approximately {}", format::size(bytes)),
            _ => "Clean".to_string(),
        };
        let ring = enabled && shows_focus(&self.clean_focus, window);
        let status = self.status_line();

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
                                .child(self.icon(ICON_ERROR, rems(1.), t.critical))
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
                                b.border_1().border_color(if enabled {
                                    t.on_accent
                                } else {
                                    t.text_disabled
                                })
                            })
                            .child(label)
                            .when(ring, |b| b.child(focus_ring(&t, px(-3.), px(7.)))),
                    ),
            )
    }

    /// The line under the checklist: handoff and Clean progress, then the last result.
    fn status_line(&self) -> Option<StatusLine> {
        if self.controller.is_closing() {
            return Some(StatusLine::info("Stopping…"));
        }
        if let Some(error) = self.controller.selection_error() {
            return Some(StatusLine::error(error));
        }
        if !self.controller.is_selection_loaded() {
            return Some(StatusLine::info("Loading Selection…"));
        }
        let rows = self.controller.rows();
        if self.controller.selection_locked() {
            let queued = rows.iter().filter(|r| r.clean.is_some()).count();
            if queued == 0 {
                return Some(StatusLine::info("Stopping the Scan to start Clean…"));
            }
            let done = rows
                .iter()
                .filter(|r| matches!(r.clean, Some(CleanProgress::Done(_))))
                .count();
            return Some(StatusLine::info(format!(
                "Cleaning… {done} of {queued} Targets finished"
            )));
        }
        self.controller
            .last_clean()
            .map(|results| StatusLine::info(format::clean_summary(results)))
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

    /// Pulses `el` to show work in progress, unless Windows animation effects are off.
    fn pulsing(&self, id: impl Into<ElementId>, el: Div) -> AnyElement {
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
    row.scan.as_ref().or(row.previous.as_ref()) == Some(&ScanResult::NotPresent)
        && row.clean.is_none()
}

/// True until the first Scan reports a result.
fn never_scanned(rows: &[Row]) -> bool {
    rows.iter()
        .all(|row| row.scan.is_none() && row.previous.is_none())
}

/// Counts the checked rows currently shown in the checklist. An absent Target
/// can remain in the saved Selection without appearing as a checkbox.
fn visible_selection_count(rows: &[Row]) -> usize {
    rows.iter()
        .filter(|row| row.selected && !is_hidden(row))
        .count()
}

/// The selected estimate shown in the header and on the Clean button.
struct SelectedEstimate {
    /// `None` while a Scan has not reported any selected size yet.
    bytes: Option<u64>,
    /// A Scan has not yet reported every selected Target.
    pending: bool,
}

/// Includes previous results still shown while a Scan runs.
fn selected_estimate(rows: &[Row], scanning: bool) -> SelectedEstimate {
    let mut bytes = 0;
    let mut known = false;
    let mut pending = false;
    for row in rows.iter().filter(|row| row.selected && !is_hidden(row)) {
        pending |= row.scan.is_none();
        if let Some(ScanResult::Complete { bytes: size }) =
            row.scan.as_ref().or(row.previous.as_ref())
        {
            bytes += size;
            known = true;
        }
    }
    let pending = scanning && pending;
    SelectedEstimate {
        bytes: (known || !pending).then_some(bytes),
        pending,
    }
}

/// The shown rows' indices under each Category heading, in checklist order.
/// Absent Targets are hidden, and so is a Category with no shown rows.
fn sections(rows: &[Row]) -> Vec<(Category, Vec<usize>)> {
    Category::ALL
        .into_iter()
        .filter_map(|category| {
            let shown: Vec<usize> = (0..rows.len())
                .filter(|&ix| rows[ix].target.category == category && !is_hidden(&rows[ix]))
                .collect();
            (!shown.is_empty()).then_some((category, shown))
        })
        .collect()
}

/// The text under the checklist. An error renders like a row error.
struct StatusLine {
    text: String,
    error: bool,
}

impl StatusLine {
    fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: false,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: true,
        }
    }
}

struct RowStatus {
    value: String,
    value_color: Hsla,
    icon: Option<(&'static str, Hsla)>,
    detail: Option<String>,
    detail_color: Hsla,
    /// The value is from the previous Scan and is being replaced.
    updating: bool,
}

/// What a row shows on the right, and the optional explanation under its name.
/// During a Scan, a row not yet reached shows its previous result dimmed, or
/// `Waiting`; the `active` row is the one being scanned.
fn row_status(row: &Row, scanning: bool, active: bool, t: &Theme) -> RowStatus {
    let mut s = RowStatus {
        value: String::new(),
        value_color: t.text_secondary,
        icon: None,
        detail: None,
        detail_color: t.text_secondary,
        updating: false,
    };
    let previous = row.scan.is_none() && row.previous.is_some();
    let blocked = |detail: String| {
        if row.selected && !previous {
            format!("{detail} · Untick or rescan to Clean")
        } else {
            detail
        }
    };
    match (&row.clean, row.scan.as_ref().or(row.previous.as_ref())) {
        (Some(CleanProgress::Queued), _) => s.value = "Queued".into(),
        (Some(CleanProgress::Cleaning), _) => {
            s.value = "Cleaning…".into();
            s.value_color = t.text;
        }
        (Some(CleanProgress::Done(result)), _) => {
            let deleted = result
                .deleted_bytes
                .map(|bytes| format!("Deleted {}", format::size(bytes)));
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
            if deleted.is_none() && result.status != CleanStatus::Failed {
                notes.push("Size unavailable".into());
            }
            match result.status {
                CleanStatus::Complete => {
                    s.icon = Some((ICON_CHECK, t.success));
                    s.value = deleted.unwrap_or_else(|| "Emptied".into());
                }
                CleanStatus::Partial => {
                    s.icon = Some((ICON_WARNING, t.caution));
                    s.value = deleted.unwrap_or_else(|| "Partly emptied".into());
                }
                CleanStatus::Failed => {
                    s.icon = Some((ICON_ERROR, t.critical));
                    s.value = "Clean failed".into();
                }
                CleanStatus::Stopped => {
                    s.value = "Stopped".into();
                    if let Some(deleted) = deleted {
                        notes.push(format!("{deleted} before stopping"));
                    }
                }
            }
            s.detail = (!notes.is_empty()).then(|| notes.join(" · "));
        }
        (None, None) if active => {
            s.value = "Scanning…".into();
            s.value_color = t.text;
        }
        (None, None) => {
            s.value = if scanning { "Waiting" } else { "Not scanned" }.into();
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
    if previous {
        s.value_color = t.text_disabled;
        s.icon = s.icon.map(|(glyph, _)| (glyph, t.text_disabled));
        // Keep the detail line so the row keeps its height while it updates.
        s.detail_color = t.text_disabled;
        s.updating = true;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::targets::TARGETS;

    #[test]
    fn status_changes_preserve_checklist_bounds_and_scroll_position() {
        use gpui::{AppContext, TestAppContext, WindowHandle};

        fn draw(app: &mut TestAppContext, window: WindowHandle<CleanerView>) {
            app.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
        }

        for text_scale in [1., 2.25] {
            let mut app = TestAppContext::single();
            let window = app.open_window(BASE_SIZE, |_, cx| {
                let mut controller = Controller::new();
                controller.load_selection(Ok(selection::Loaded {
                    choices: selection::defaults(),
                    needs_save: false,
                }));
                let (save_tx, _) = std::sync::mpsc::channel();
                CleanerView {
                    row_focus: controller
                        .rows()
                        .iter()
                        .map(|_| cx.focus_handle())
                        .collect(),
                    controller,
                    save_tx,
                    worker: None,
                    system: System {
                        text_scale,
                        animations: false,
                        ..System::read()
                    },
                    root_focus: cx.focus_handle(),
                    scan_focus: cx.focus_handle(),
                    clean_focus: cx.focus_handle(),
                    row_children: Rc::default(),
                    list: ScrollHandle::new(),
                    thumb_drag: None,
                    _watcher: None,
                    _subscriptions: Vec::new(),
                }
            });
            draw(&mut app, window);
            window
                .update(&mut app, |view, _, cx| {
                    view.list
                        .set_offset(point(px(0.), -view.list.max_offset().y));
                    cx.notify();
                })
                .unwrap();
            draw(&mut app, window);
            let (bounds, offset) = window
                .update(&mut app, |view, _, _| {
                    (view.list.bounds(), view.list.offset())
                })
                .unwrap();
            assert!(
                offset.y < px(0.),
                "the checklist must be scrolled for this check"
            );

            for error in [
                "Access denied".to_string(),
                "The selection could not be written because the destination is unavailable. "
                    .repeat(12),
            ] {
                window
                    .update(&mut app, |view, _, cx| {
                        view.controller.toggle(TARGETS[0].id).unwrap();
                        view.controller.save_finished(Err(error));
                        cx.notify();
                    })
                    .unwrap();
                draw(&mut app, window);
                window
                    .update(&mut app, |view, _, _| {
                        assert_eq!(
                            view.list.bounds(),
                            bounds,
                            "status changed the checklist viewport at text scale {text_scale}"
                        );
                        assert_eq!(
                            view.list.offset(),
                            offset,
                            "status moved the scrolled checklist"
                        );
                    })
                    .unwrap();
            }

            window
                .update(&mut app, |view, _, cx| {
                    view.controller.toggle(TARGETS[0].id).unwrap();
                    view.controller.save_finished(Ok(()));
                    cx.notify();
                })
                .unwrap();
            draw(&mut app, window);
            window
                .update(&mut app, |view, _, _| {
                    assert_eq!(
                        view.list.bounds(),
                        bounds,
                        "clearing status changed the checklist viewport"
                    );
                    assert_eq!(
                        view.list.offset(),
                        offset,
                        "clearing status moved the scrolled checklist"
                    );
                })
                .unwrap();
        }
    }

    #[test]
    fn rescan_estimate_keeps_previous_sizes_until_scanned() {
        let mut rows: Vec<Row> = TARGETS
            .iter()
            .map(|target| Row {
                target,
                selected: matches!(target.id, "user-temp" | "windows-temp"),
                scan: None,
                previous: None,
                clean: None,
            })
            .collect();
        let estimate = selected_estimate(&rows, true);
        assert_eq!((estimate.bytes, estimate.pending), (None, true));
        rows[0].previous = Some(ScanResult::Complete { bytes: 4 });
        rows[1].scan = Some(ScanResult::Complete { bytes: 5 });
        let estimate = selected_estimate(&rows, true);
        assert_eq!((estimate.bytes, estimate.pending), (Some(9), true));
        rows[0].scan = Some(ScanResult::Complete { bytes: 1 });
        rows[0].previous = None;
        let estimate = selected_estimate(&rows, true);
        assert_eq!((estimate.bytes, estimate.pending), (Some(6), false));
    }

    #[test]
    fn absent_targets_and_empty_categories_are_hidden() {
        let mut rows: Vec<Row> = TARGETS
            .iter()
            .map(|target| Row {
                target,
                selected: true,
                scan: Some(match target.id {
                    "chrome-cache" | "directx-shader-cache" => ScanResult::NotPresent,
                    _ => ScanResult::Complete { bytes: 1 },
                }),
                previous: None,
                clean: None,
            })
            .collect();
        assert_eq!(visible_selection_count(&rows), TARGETS.len() - 2);
        rows[0].selected = false;
        assert_eq!(visible_selection_count(&rows), TARGETS.len() - 3);
        let shown: Vec<(&str, Vec<&str>)> = sections(&rows)
            .into_iter()
            .map(|(category, indices)| {
                (
                    category.name(),
                    indices.iter().map(|&ix| rows[ix].target.id).collect(),
                )
            })
            .collect();
        let headings: Vec<&str> = shown.iter().map(|(name, _)| *name).collect();
        assert_eq!(headings, ["Windows", "Developer"]);
        assert_eq!(
            shown[0].1,
            [
                "user-temp",
                "windows-temp",
                "recycle-bin",
                "thumbnail-cache",
                "crash-dumps"
            ]
        );
    }
}
