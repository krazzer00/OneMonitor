//! Window chrome (glass effect, rounded corners) and tray-anchored positioning.

use tauri::{AppHandle, Manager, PhysicalPosition, Rect, WebviewWindow};

fn build() -> u32 {
    #[cfg(windows)]
    {
        windows_version::OsVersion::current().build
    }
    #[cfg(not(windows))]
    {
        0
    }
}

pub fn is_win11() -> bool {
    build() >= 22000
}

/// Effects this Windows version can render: Mica is Windows 11 only, acrylic
/// needs Windows 10 1809+, blur works on any Windows 10.
pub fn supported_effects() -> Vec<&'static str> {
    let b = build();
    let mut v = vec![];
    if b >= 10240 {
        v.push("blur");
    }
    if b >= 17763 {
        v.push("acrylic");
    }
    if b >= 22000 {
        v.push("mica");
    }
    v
}

/// Whether a backdrop effect is actually applied for this setting.
pub fn effect_active(effect: &str) -> bool {
    supported_effects().contains(&effect)
}

/// Corner radius of the CSS shell on Windows 10 (Windows 11 uses DWM's 8 px).
const W10_RADIUS: f64 = 14.0;

/// Applies the backdrop effect and native rounded corners (Windows 11).
pub fn apply_style(win: &WebviewWindow, effect: &str) {
    #[cfg(windows)]
    {
        use tauri::utils::config::WindowEffectsConfig;
        use tauri::window::{Effect, EffectsBuilder};
        let eff = if !effect_active(effect) {
            None
        } else if win.label() == "popup" {
            // The popup is never activated; acrylic/mica fall back to a flat
            // colour on inactive windows, classic blur-behind does not.
            Some(Effect::Blur)
        } else {
            match effect {
                "acrylic" => Some(Effect::Acrylic),
                "blur" => Some(Effect::Blur),
                _ => Some(Effect::MicaDark),
            }
        };
        // System backdrops follow the window's light/dark mode: force dark,
        // otherwise acrylic/mica turn light grey under the dark glass tint.
        set_dark_mode(win);
        let _ = win.set_effects(None::<WindowEffectsConfig>);
        if let Some(e) = eff {
            let _ = win.set_effects(
                EffectsBuilder::new()
                    .effect(e)
                    .color(tauri::window::Color(16, 16, 20, 110))
                    .build(),
            );
        }
        let w11 = is_win11();
        // On Windows 11 the DWM shadow also gives the window native rounded corners.
        let _ = win.set_shadow(w11);
        if w11 {
            round_corners(win);
        } else {
            // Windows 10 has no rounded windows: clip the window itself so the
            // blur does not show in the square corners around the CSS shell.
            clip_rounded(win);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (win, effect);
    }
}

#[cfg(windows)]
fn set_dark_mode(win: &WebviewWindow) {
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    if let Ok(hwnd) = win.hwnd() {
        let on: i32 = 1;
        unsafe {
            DwmSetWindowAttribute(
                hwnd.0 as _,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &on as *const i32 as *const _,
                std::mem::size_of::<i32>() as u32,
            );
        }
    }
}

/// Clips the window to a rounded rectangle (Windows 10). Call again after resizing.
pub fn clip_rounded(win: &WebviewWindow) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};
        if is_win11() {
            return;
        }
        let (Ok(hwnd), Ok(size), Ok(scale)) = (win.hwnd(), win.outer_size(), win.scale_factor())
        else {
            return;
        };
        let d = (W10_RADIUS * 2.0 * scale).round() as i32;
        unsafe {
            // the system owns the region after SetWindowRgn succeeds
            let rgn = CreateRoundRectRgn(0, 0, size.width as i32 + 1, size.height as i32 + 1, d, d);
            SetWindowRgn(hwnd.0 as _, rgn, 1);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (win, W10_RADIUS);
    }
}

#[cfg(windows)]
fn round_corners(win: &WebviewWindow) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    if let Ok(hwnd) = win.hwnd() {
        let pref: i32 = DWMWCP_ROUND;
        unsafe {
            DwmSetWindowAttribute(
                hwnd.0 as _,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                &pref as *const i32 as *const _,
                std::mem::size_of::<i32>() as u32,
            );
        }
    }
}

/// Places `win` next to the tray icon (above the taskbar when it is at the bottom),
/// or in the bottom-right corner of the primary work area when no anchor is known.
pub fn place_near_tray(app: &AppHandle, win: &WebviewWindow, anchor: Option<Rect>) {
    let anchor = anchor.map(|r| {
        let p = r.position.to_physical::<f64>(1.0);
        let s = r.size.to_physical::<f64>(1.0);
        (p.x, p.y, s.width, s.height)
    });
    let monitor = match anchor {
        Some((x, y, aw, ah)) => app
            .monitor_from_point(x + aw / 2.0, y + ah / 2.0)
            .ok()
            .flatten(),
        None => None,
    }
    .or_else(|| win.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return };

    // A window that has never been shown may still report a 0x0 size;
    // fall back to its configured logical size.
    let size = win.outer_size().unwrap_or_default();
    let (w, h) = if size.width >= 50 && size.height >= 50 {
        (size.width as i32, size.height as i32)
    } else {
        let (lw, lh) = if win.label() == "popup" { (300.0, 180.0) } else { (392.0, 600.0) };
        let sf = monitor.scale_factor();
        ((lw * sf) as i32, (lh * sf) as i32)
    };

    let wa = monitor.work_area();
    let (wx, wy) = (wa.position.x, wa.position.y);
    let (ww, wh) = (wa.size.width as i32, wa.size.height as i32);
    let mon = monitor.position();
    let mon_size = monitor.size();
    let margin = (12.0 * monitor.scale_factor()) as i32;

    let (x, y) = match anchor {
        Some((ax, ay, aw, ah)) => {
            let cx = (ax + aw / 2.0) as i32;
            let cy = (ay + ah / 2.0) as i32;
            let mut x = cx - w / 2;
            let mut y;
            let bottom = cy > mon.y + mon_size.height as i32 / 2;
            let right = cx > mon.x + mon_size.width as i32 / 2;
            if cy > wy + wh || bottom {
                y = wy + wh - h - margin;
            } else {
                y = wy + margin;
            }
            // vertical taskbar: put the window beside it
            if ww < mon_size.width as i32 && (cx < wx || cx > wx + ww) {
                x = if right { wx + ww - w - margin } else { wx + margin };
                y = cy - h / 2;
            }
            (x, y)
        }
        None => (wx + ww - w - margin, wy + wh - h - margin),
    };
    let x = x.clamp(wx + margin, (wx + ww - w - margin).max(wx + margin));
    let y = y.clamp(wy + margin, (wy + wh - h - margin).max(wy + margin));
    let _ = win.set_position(PhysicalPosition::new(x, y));
}

pub fn window(app: &AppHandle, label: &str) -> Option<WebviewWindow> {
    app.get_webview_window(label)
}
