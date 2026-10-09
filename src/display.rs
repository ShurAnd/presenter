//! Мониторы: список, выбор монитора для показа (видеостены) и подгонка окон под него.
//! Координаты — физические пиксели (программа учитывает масштаб каждого монитора).

/// Монитор: системное имя (`\\.\DISPLAY2`), прямоугольник (x, y, ширина, высота), основной ли.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub device: String,
    pub rect: (i32, i32, i32, i32),
    pub primary: bool,
}

impl Monitor {
    /// Подпись для списка: «2: 3840×2160 (\\.\DISPLAY2)», у основного — «основной».
    pub fn label(&self, n: usize) -> String {
        let (_, _, w, h) = self.rect;
        format!("{n}: {w}×{h}{} ({})", if self.primary { ", основной" } else { "" }, self.device)
    }
}

/// Все мониторы; основной — первым.
pub fn list() -> Vec<Monitor> {
    let mut v = win::list();
    v.sort_by_key(|m| (!m.primary, m.rect.0, m.rect.1));
    v
}

/// Монитор для показа: выбранный по имени, а если его нет (отключили) — основной.
pub fn target(device: &str) -> Option<Monitor> {
    let all = list();
    all.iter().find(|m| !device.is_empty() && m.device == device).or_else(|| all.first()).cloned()
}

/// Поставить окно (по заголовку) ровно на прямоугольник; `extra_h` — добавка к высоте
/// (сцене нужна +1 точка: OpenGL-окно ровно по экрану Windows переводит в «полноэкранный режим»).
/// Окно не трогается, если оно уже на месте. Порядок окон и фокус не меняются.
pub fn fit_window(title: &str, rect: (i32, i32, i32, i32), extra_h: i32) {
    win::fit_window(title, rect, extra_h);
}

/// Поставить окно (по дескриптору) на прямоугольник монитора, если оно не там.
pub fn fit_hwnd(hwnd: isize, rect: (i32, i32, i32, i32)) {
    win::fit_hwnd(hwnd, rect, 0);
}

#[cfg(not(windows))]
mod win {
    use super::Monitor;
    pub fn list() -> Vec<Monitor> {
        Vec::new()
    }
    pub fn fit_window(_: &str, _: (i32, i32, i32, i32), _: i32) {}
    pub fn fit_hwnd(_: isize, _: (i32, i32, i32, i32), _: i32) {}
}

#[cfg(windows)]
mod win {
    use super::Monitor;
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER};
    use windows::core::{HSTRING, PCWSTR};

    unsafe extern "system" fn cb(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let list = &mut *(data.0 as *mut Vec<Monitor>);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(m, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
            let r = info.monitorInfo.rcMonitor;
            let len = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
            list.push(Monitor {
                device: String::from_utf16_lossy(&info.szDevice[..len]),
                rect: (r.left, r.top, r.right - r.left, r.bottom - r.top),
                primary: info.monitorInfo.dwFlags & 1 != 0,
            });
        }
        true.into()
    }

    pub fn list() -> Vec<Monitor> {
        let mut v: Vec<Monitor> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize));
        }
        v
    }

    pub fn fit_hwnd(hwnd: isize, rect: (i32, i32, i32, i32), extra_h: i32) {
        if hwnd == 0 {
            return;
        }
        let hwnd = HWND(hwnd as *mut _);
        unsafe {
            let mut cur = RECT::default();
            if GetWindowRect(hwnd, &mut cur).is_err() {
                return;
            }
            let (x, y, w, h) = (rect.0, rect.1, rect.2, rect.3 + extra_h);
            if (cur.left, cur.top, cur.right - cur.left, cur.bottom - cur.top) != (x, y, w, h) {
                let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
            }
        }
    }

    pub fn fit_window(title: &str, rect: (i32, i32, i32, i32), extra_h: i32) {
        unsafe {
            if let Ok(h) = FindWindowW(PCWSTR::null(), &HSTRING::from(title)) {
                fit_hwnd(h.0 as isize, rect, extra_h);
            }
        }
    }
}
