//! Window chrome (glass effect, rounded corners) and tray-anchored positioning.

use tauri::{AppHandle, Manager, PhysicalPosition, Rect, WebviewWindow};

pub fn is_win11() -> bool {
    #[cfg(windows)]
    {
        windows_version::OsVersion::current().build >= 22000
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Whether a backdrop effect is actually applied for this setting.
/// Effects are Windows 11 only: on Windows 10 they fill the square window
/// rectangle and would break the rounded CSS corners.
pub fn effect_active(effect: &str) -> bool {
    is_win11() && matches!(effect, "acrylic" | "blur" | "mica")
}

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
                _ => Some(Effect::Mica),
            }
        };
        let _ = win.set_effects(None::<WindowEffectsConfig>);
        if let Some(e) = eff {
            let _ = win.set_effects(
                EffectsBuilder::new()
                    .effect(e)
                    .color(tauri::window::Color(16, 16, 20, 90))
                    .build(),
            );
        }
        let w11 = is_win11();
        // On Windows 11 the DWM shadow also gives the window native rounded corners.
        let _ = win.set_shadow(w11);
        if w11 {
            round_corners(win);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (win, effect);
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
    let Ok(size) = win.outer_size() else { return };
    let (w, h) = (size.width as i32, size.height as i32);

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
