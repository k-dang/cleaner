//! The cleaner view's lifetime, system settings, and worker orchestration.
//! Rendering, presentation, and control interaction live in focused child modules.

mod controls;
mod presentation;
mod render;

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::SystemTime;

use futures::StreamExt;
use gpui::{
    App, Context, FocusHandle, Pixels, ScrollHandle, Size, Subscription, Window, actions, px, size,
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

use crate::cleanup::recycle_bin::SystemShell;
use crate::cleanup::{self, Event, Roots, Snapshot};
use crate::selection::{SelectionEvent, SelectionStore};
use crate::targets;
use crate::theme::{self, System};
use cleaner_core::controller::OpId;
use cleaner_core::selection::Choices;
use cleaner_core::targets::TargetId;

type Controller = cleaner_core::controller::Controller<Snapshot>;
type Command = cleaner_core::controller::Command<Snapshot>;

actions!(cleaner, [FocusNext, FocusPrev]);

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

        let (save_tx, mut selection_rx) = SelectionStore::start_worker();
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

        let controller = Controller::new(targets::catalog());
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
                    cleanup::scan_targets(&targets, roots, &SystemShell, time, stop, report)
                });
            }
            Some(Command::Clean { op, targets }) => {
                self.start_worker(op, cx, move |roots, time, stop, report| {
                    cleanup::clean_targets(&targets, roots, &SystemShell, time, stop, report)
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

    fn send_save(&mut self, save: Option<Choices>) {
        if let Some(choices) = save
            && self.save_tx.send(choices).is_err()
        {
            self.controller
                .save_finished(Err("the save worker stopped".into()));
        }
    }

    fn toggle(&mut self, id: TargetId, cx: &mut Context<Self>) {
        let save = self.controller.toggle(id);
        self.send_save(save);
        cx.notify();
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::targets::TARGETS;
    use cleaner_core::results::ScanResult;
    use cleaner_core::selection;
    use gpui::{AppContext, TestAppContext, WindowHandle, point};

    /// A rendered checklist with fixture reports and no filesystem or save workers.
    fn fixture_window(app: &mut TestAppContext, text_scale: f32) -> WindowHandle<CleanerView> {
        app.open_window(BASE_SIZE, |_, cx| {
            let mut controller = Controller::new(targets::catalog());
            controller.load_selection(Ok(selection::Loaded {
                choices: selection::defaults(targets::catalog()),
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
        })
    }

    fn draw(app: &mut TestAppContext, window: WindowHandle<CleanerView>) {
        app.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    #[test]
    fn selection_edits_queue_in_order_and_a_stopped_store_keeps_the_ticks() {
        let mut app = TestAppContext::single();
        let window = fixture_window(&mut app, 1.);
        let (save_tx, saves) = std::sync::mpsc::channel();
        window
            .update(&mut app, |view, _, cx| {
                view.save_tx = save_tx;
                view.toggle("user-temp", cx);
                view.toggle("pip-cache", cx);
                view.toggle("go-build-cache", cx);
            })
            .unwrap();
        let first = saves.try_recv().unwrap();
        assert!(!first["user-temp"]);
        assert!(!first["pip-cache"]);
        let second = saves.try_recv().unwrap();
        assert!(second["pip-cache"]);
        assert!(!second["go-build-cache"]);
        let latest = saves.try_recv().unwrap();
        assert!(!latest["user-temp"]);
        assert!(latest["pip-cache"]);
        assert!(latest["go-build-cache"]);
        window
            .update(&mut app, |view, window, cx| {
                for _ in 0..3 {
                    view.apply_selection(SelectionEvent::Saved(Ok(())), window, cx);
                }
                assert!(view.controller.selection_error().is_none());
            })
            .unwrap();
        drop(saves);
        window
            .update(&mut app, |view, _, cx| {
                view.toggle("pip-cache", cx);
                assert!(
                    !view
                        .controller
                        .rows()
                        .iter()
                        .find(|row| row.target.id == "pip-cache")
                        .unwrap()
                        .selected
                );
                assert_eq!(
                    view.controller.selection_error(),
                    Some("Selection could not be saved: the save worker stopped")
                );
                assert!(
                    view.request_close(cx),
                    "a failed send cannot leave a save pending"
                );
            })
            .unwrap();
    }

    #[test]
    fn clean_enters_tab_order_only_after_scan_finishes() {
        let mut app = TestAppContext::single();
        let window = fixture_window(&mut app, 1.);
        let scan = window
            .update(&mut app, |view, _, cx| {
                view.controller.load_selection(Ok(selection::Loaded {
                    choices: selection::choices(targets::catalog(), |target| {
                        target.id == "user-temp"
                    }),
                    needs_save: false,
                }));
                let Some(Command::Scan { op, .. }) = view.controller.start_scan() else {
                    panic!("expected Scan")
                };
                view.controller.apply(
                    op,
                    Event::Scanned(
                        "user-temp",
                        ScanResult::Complete { bytes: 10 },
                        Snapshot::Folders,
                    ),
                );
                cx.notify();
                op
            })
            .unwrap();
        draw(&mut app, window);
        window
            .update(&mut app, |view, window, cx| {
                window.focus(view.row_focus.last().unwrap(), cx);
                window.focus_next(cx);
                assert!(
                    view.row_focus[0].is_focused(window),
                    "disabled Clean is skipped"
                );
                assert!(!view.controller.selection_locked());
                assert_eq!(view.controller.request_clean(), None);
                assert!(
                    presentation::status_line(&view.controller, &view.controller.summary())
                        .is_none()
                );
                assert_eq!(view.controller.apply(scan, Event::Finished), None);
                cx.notify();
            })
            .unwrap();
        draw(&mut app, window);
        window
            .update(&mut app, |view, window, cx| {
                window.focus(view.row_focus.last().unwrap(), cx);
                window.focus_next(cx);
                assert!(
                    view.clean_focus.is_focused(window),
                    "ready Clean can receive keyboard focus"
                );
                assert!(view.controller.can_clean());
                assert!(
                    presentation::status_line(&view.controller, &view.controller.summary())
                        .is_none()
                );
            })
            .unwrap();
    }

    #[test]
    fn status_changes_preserve_checklist_bounds_and_scroll_position() {
        for text_scale in [1., 2.25] {
            let mut app = TestAppContext::single();
            let window = fixture_window(&mut app, text_scale);
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
                        view.controller.toggle(TARGETS[0].target.id).unwrap();
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
                    view.controller.toggle(TARGETS[0].target.id).unwrap();
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
}
