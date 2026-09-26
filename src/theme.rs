//! Windows light, dark, and high-contrast colors and text scaling, read from the
//! system and refreshed when the user changes them.

use futures::channel::mpsc::UnboundedSender;
use gpui::{Hsla, Rgba, rgb, rgba};
use windows::Foundation::TypedEventHandler;
use windows::UI::Color;
use windows::UI::ViewManagement::{UIColorType, UISettings};
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW,
    COLOR_WINDOWTEXT, GetSysColor, SYS_COLOR_INDEX,
};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};

/// Semantic colors, named after the WinUI tokens they follow.
#[derive(Clone, Copy, PartialEq)]
pub struct Theme {
    pub high_contrast: bool,
    pub background: Hsla,
    pub card: Hsla,
    pub card_hover: Hsla,
    pub card_stroke: Hsla,
    pub divider: Hsla,
    pub text: Hsla,
    pub text_secondary: Hsla,
    pub text_disabled: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_pressed: Hsla,
    pub on_accent: Hsla,
    pub accent_disabled: Hsla,
    pub on_accent_disabled: Hsla,
    pub subtle_hover: Hsla,
    pub subtle_pressed: Hsla,
    pub checkbox_stroke: Hsla,
    pub checkbox_fill: Hsla,
    pub focus: Hsla,
    pub success: Hsla,
    pub caution: Hsla,
    pub critical: Hsla,
}

/// Current system appearance settings.
#[derive(Clone, Copy, PartialEq)]
pub struct System {
    pub theme: Theme,
    /// The Windows "Text size" accessibility setting, from 1.0 to 2.25.
    pub text_scale: f32,
}

impl System {
    pub fn read() -> Self {
        let settings = UISettings::new().ok();
        let color = |kind| settings.as_ref().and_then(|s| s.GetColorValue(kind).ok());
        let text_scale = settings
            .as_ref()
            .and_then(|s| s.TextScaleFactor().ok())
            .unwrap_or(1.0) as f32;
        let dark = color(UIColorType::Foreground).is_some_and(|c| is_light(&c));
        let theme = if high_contrast_on() {
            Theme::high_contrast()
        } else if dark {
            // WinUI uses a lighter accent shade on dark backgrounds.
            Theme::dark(color(UIColorType::AccentLight2).map_or(rgb(0x60cdff), to_rgba))
        } else {
            Theme::light(color(UIColorType::AccentDark1).map_or(rgb(0x005fb8), to_rgba))
        };
        Self { theme, text_scale }
    }
}

/// Keeps change notifications flowing while alive. Windows raises them on a
/// background thread; each one sends `()` so the UI thread can re-read `System`.
pub struct Watcher {
    _settings: UISettings,
}

pub fn watch(changed: UnboundedSender<()>) -> windows::core::Result<Watcher> {
    let settings = UISettings::new()?;
    let tx = changed.clone();
    // Fires for light/dark, accent, and high-contrast changes.
    settings.ColorValuesChanged(&TypedEventHandler::new(move |_, _| {
        let _ = tx.unbounded_send(());
        Ok(())
    }))?;
    settings.TextScaleFactorChanged(&TypedEventHandler::new(move |_, _| {
        let _ = changed.unbounded_send(());
        Ok(())
    }))?;
    Ok(Watcher {
        _settings: settings,
    })
}

impl Theme {
    fn light(accent: Rgba) -> Self {
        Self {
            high_contrast: false,
            background: rgb(0xf3f3f3).into(),
            card: rgb(0xfbfbfb).into(),
            card_hover: rgb(0xf6f6f6).into(),
            card_stroke: rgb(0xe5e5e5).into(),
            divider: rgb(0xe0e0e0).into(),
            text: rgba(0x000000e4).into(),
            text_secondary: rgba(0x0000009b).into(),
            text_disabled: rgba(0x0000005c).into(),
            accent: accent.into(),
            accent_hover: with_alpha(accent, 0.9),
            accent_pressed: with_alpha(accent, 0.8),
            on_accent: rgb(0xffffff).into(),
            accent_disabled: rgba(0x00000037).into(),
            on_accent_disabled: rgb(0xffffff).into(),
            subtle_hover: rgba(0x0000000a).into(),
            subtle_pressed: rgba(0x00000006).into(),
            checkbox_stroke: rgba(0x0000009b).into(),
            checkbox_fill: rgba(0x00000006).into(),
            focus: rgba(0x000000e4).into(),
            success: rgb(0x0f7b0f).into(),
            caution: rgb(0x9d5d00).into(),
            critical: rgb(0xc42b1c).into(),
        }
    }

    fn dark(accent: Rgba) -> Self {
        Self {
            high_contrast: false,
            background: rgb(0x202020).into(),
            card: rgb(0x2b2b2b).into(),
            card_hover: rgb(0x323232).into(),
            card_stroke: rgb(0x1d1d1d).into(),
            divider: rgba(0xffffff15).into(),
            text: rgb(0xffffff).into(),
            text_secondary: rgba(0xffffffc8).into(),
            text_disabled: rgba(0xffffff5d).into(),
            accent: accent.into(),
            accent_hover: with_alpha(accent, 0.9),
            accent_pressed: with_alpha(accent, 0.8),
            on_accent: rgb(0x000000).into(),
            accent_disabled: rgba(0xffffff28).into(),
            on_accent_disabled: rgba(0xffffff87).into(),
            subtle_hover: rgba(0xffffff0f).into(),
            subtle_pressed: rgba(0xffffff0a).into(),
            checkbox_stroke: rgba(0xffffff8b).into(),
            checkbox_fill: rgba(0x00000029).into(),
            focus: rgb(0xffffff).into(),
            success: rgb(0x6ccb5f).into(),
            caution: rgb(0xfce100).into(),
            critical: rgb(0xff99a4).into(),
        }
    }

    /// Uses the user's high-contrast palette. State colors fall back to plain
    /// text, so meaning is always carried by words and icons too.
    fn high_contrast() -> Self {
        let window = sys_color(COLOR_WINDOW);
        let text = sys_color(COLOR_WINDOWTEXT);
        let highlight = sys_color(COLOR_HIGHLIGHT);
        let gray = sys_color(COLOR_GRAYTEXT);
        let transparent = gpui::transparent_black();
        Self {
            high_contrast: true,
            background: window,
            card: window,
            card_hover: window,
            card_stroke: text,
            divider: text,
            text,
            text_secondary: text,
            text_disabled: gray,
            accent: highlight,
            accent_hover: highlight,
            accent_pressed: highlight,
            on_accent: sys_color(COLOR_HIGHLIGHTTEXT),
            accent_disabled: window,
            on_accent_disabled: gray,
            subtle_hover: sys_color(COLOR_BTNFACE),
            subtle_pressed: sys_color(COLOR_BTNFACE),
            checkbox_stroke: text,
            checkbox_fill: transparent,
            focus: text,
            success: text,
            caution: text,
            critical: text,
        }
    }
}

fn high_contrast_on() -> bool {
    let mut info = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            info.cbSize,
            Some(&mut info as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && info.dwFlags.contains(HCF_HIGHCONTRASTON)
}

fn sys_color(index: SYS_COLOR_INDEX) -> Hsla {
    // COLORREF is 0x00BBGGRR.
    let c = unsafe { GetSysColor(index) };
    let [r, g, b] = [c & 0xff, (c >> 8) & 0xff, (c >> 16) & 0xff];
    rgb((r << 16) | (g << 8) | b).into()
}

fn to_rgba(c: Color) -> Rgba {
    rgb(((c.R as u32) << 16) | ((c.G as u32) << 8) | c.B as u32)
}

fn with_alpha(color: Rgba, a: f32) -> Hsla {
    Rgba { a, ..color }.into()
}

// Same test GPUI uses to detect dark mode from the foreground color.
fn is_light(c: &Color) -> bool {
    (5 * c.G as u32 + 2 * c.R as u32 + c.B as u32) > 8 * 128
}
