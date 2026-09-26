mod controller;
mod fixture;
mod format;
mod results;
mod targets;
mod theme;
mod ui;

use std::rc::Rc;

use gpui::{AppContext, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
use windows::core::HSTRING;

/// Opens the cleaner window and runs until it closes.
pub fn run() {
    std::panic::set_hook(Box::new(|info| {
        show_error(&format!(
            "Cleaner stopped because of an internal error.\n\n{info}"
        ));
    }));
    let platform = match gpui_windows::WindowsPlatform::new(false) {
        Ok(platform) => platform,
        Err(error) => return show_error(&format!("Cleaner could not start.\n\n{error:#}")),
    };
    gpui::Application::with_platform(Rc::new(platform)).run(|cx| {
        cx.bind_keys([
            KeyBinding::new("tab", ui::FocusNext, None),
            KeyBinding::new("shift-tab", ui::FocusPrev, None),
        ]);
        let text_scale = theme::System::read().text_scale;
        let bounds = Bounds::centered(None, ui::window_size(text_scale), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Cleaner".into()),
                ..Default::default()
            }),
            is_resizable: false,
            app_id: Some("cc-cleaner-at-home".into()),
            ..Default::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            cx.new(|cx| ui::CleanerView::new(window, cx))
        });
        if let Err(error) = opened {
            show_error(&format!("Cleaner could not start.\n\n{error:#}"));
            cx.quit();
            return;
        }
        cx.on_window_closed(|cx, _| cx.quit()).detach();
    });
}

/// Reports a fatal error, such as unsupported graphics or a panic, since the app has no console.
fn show_error(text: &str) {
    // SAFETY: both strings are valid, null-terminated HSTRINGs that outlive the call.
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            &HSTRING::from("Cleaner"),
            MB_OK | MB_ICONERROR,
        )
    };
}
