//! Мониторы: список, выбор монитора для показа (основной или любой из подключённых) и подгонка окон под него.
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

/// Окно (по дескриптору) — на этом мониторе: его центр внутри прямоугольника.
pub fn window_on(hwnd: isize, rect: (i32, i32, i32, i32)) -> bool {
    win::window_on(hwnd, rect)
}

/// Поставить окно (по дескриптору) ровно на прямоугольник; `extra_h` — добавка к высоте
/// (сцене нужна +1 точка: OpenGL-окно ровно по экрану Windows переводит в «полноэкранный режим»).
/// Окно не трогается, если оно уже на месте. Порядок окон и фокус не меняются.
/// Дескриптор, а не заголовок: две копии программы создают окна с одинаковыми заголовками.
pub fn fit_hwnd(hwnd: isize, rect: (i32, i32, i32, i32), extra_h: i32) {
    win::fit_hwnd(hwnd, rect, extra_h)
}

#[cfg(not(windows))]
mod win {
    use super::Monitor;
    pub fn list() -> Vec<Monitor> {
        Vec::new()
    }
    pub fn window_on(_: isize, _: (i32, i32, i32, i32)) -> bool {
        false
    }
    pub fn fit_hwnd(_: isize, _: (i32, i32, i32, i32), _: i32) {}
}

#[cfg(windows)]
mod win {
    use super::Monitor;
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER};

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

    pub fn window_on(hwnd: isize, rect: (i32, i32, i32, i32)) -> bool {
        let mut r = RECT::default();
        if hwnd == 0 || unsafe { GetWindowRect(HWND(hwnd as *mut _), &mut r) }.is_err() {
            return false;
        }
        let (cx, cy) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
        cx >= rect.0 && cx < rect.0 + rect.2 && cy >= rect.1 && cy < rect.1 + rect.3
    }

}

#[cfg(test)]
mod stage9_tests {
    use super::*;

    #[test]
    fn labels_preserve_dimensions_names_and_primary_marker() {
        let m = Monitor {
            device: r"\\.\DISPLAY2".into(), rect: (-1920, 0, 1920, 1080), primary: false,
        };
        assert_eq!(m.label(2), r"2: 1920×1080 (\\.\DISPLAY2)");
        assert!(!m.label(2).contains("основной"));
        let primary = Monitor { primary: true, ..m };
        assert!(primary.label(1).contains("основной"));
    }

    #[test]
    fn live_list_places_primary_before_secondary_displays() {
        let monitors = list();
        if let Some(first) = monitors.first() {
            if monitors.iter().any(|m| m.primary) {
                assert!(first.primary);
            }
        }
        for monitor in &monitors {
            assert!(monitor.rect.2 > 0 && monitor.rect.3 > 0);
        }
    }

    #[test]
    fn target_falls_back_to_first_or_none_for_missing_display() {
        let monitors = list();
        let default = target("");
        assert_eq!(default, monitors.first().cloned());
        assert_eq!(target("__missing_display_stage9__"), default);
        if let Some(first) = monitors.first() {
            assert_eq!(target(&first.device), Some(first.clone()));
        }
    }

    #[test]
    fn absent_window_handle_is_never_reported_on_monitor() {
        assert!(!window_on(0, (0, 0, 1920, 1080)));
        fit_hwnd(0, (0, 0, 1920, 1080), 1);
    }
}
