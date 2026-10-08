mod cleanup;
mod format;
mod selection;
mod targets;
mod theme;
mod ui;

use std::rc::Rc;

use gpui::{AppContext, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions};
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::Security::{
    GetLengthSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
use windows::core::{HSTRING, Result};

fn mutex_name(sid: &[u8]) -> HSTRING {
    let mut name = String::from("Global\\cc-cleaner-at-home-");
    for byte in sid {
        use std::fmt::Write;
        write!(name, "{byte:02x}").unwrap();
    }
    HSTRING::from(name)
}

fn current_user_mutex_name() -> Result<HSTRING> {
    let mut token = HANDLE::default();
    // SAFETY: the current process handle is valid and `token` receives the opened handle.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)? };
    let result = (|| {
        let mut size = 0;
        // SAFETY: this size query writes only the required length.
        if let Err(error) = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut size) }
            && size == 0
        {
            return Err(error);
        }
        let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        // SAFETY: the aligned buffer has at least `size` writable bytes.
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                size,
                &mut size,
            )?
        };
        // SAFETY: a successful TokenUser query returns a TOKEN_USER in `buffer`.
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        // SAFETY: TokenUser includes a valid SID that remains alive in `buffer`.
        let sid_size = unsafe { GetLengthSid(user.User.Sid) } as usize;
        // SAFETY: GetLengthSid returns the size of that SID in `buffer`.
        let sid = unsafe { std::slice::from_raw_parts(user.User.Sid.0.cast::<u8>(), sid_size) };
        Ok(mutex_name(sid))
    })();
    // SAFETY: `token` was opened above and is no longer used.
    let _ = unsafe { CloseHandle(token) };
    result
}

/// Opens the cleaner window and runs until it closes.
pub fn run() {
    std::panic::set_hook(Box::new(|info| {
        show_error(&format!(
            "Cleaner stopped because of an internal error.\n\n{info}"
        ));
    }));
    let mutex_name = match current_user_mutex_name() {
        Ok(name) => name,
        Err(error) => return show_error(&format!("Cleaner could not start.\n\n{error}")),
    };
    // The global namespace excludes the same account across sessions, while the
    // account SID keeps other accounts' Cleaner instances independent.
    // The handle is never closed, so the mutex is held until the process exits.
    // SAFETY: HSTRING supplies a terminated name for the call.
    if let Err(error) = unsafe { CreateMutexW(None, true, &mutex_name) } {
        return show_error(&format!("Cleaner could not start.\n\n{error}"));
    }
    // SAFETY: GetLastError reads this thread's last Win32 error immediately
    // after CreateMutexW.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return show_error("Cleaner is already running.");
    }
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

#[cfg(test)]
mod tests {
    use super::{current_user_mutex_name, mutex_name};

    #[test]
    fn mutex_names_are_scoped_to_the_account_sid() {
        assert_eq!(
            current_user_mutex_name().unwrap(),
            current_user_mutex_name().unwrap()
        );
        assert_eq!(mutex_name(&[1, 2]), mutex_name(&[1, 2]));
        assert_ne!(mutex_name(&[1, 2]), mutex_name(&[1, 3]));
    }
}
