//! Оформление операторского окна: светлая и тёмная темы.
//! Цвета — функции, а не константы: тема переключается на лету, без перезапуска.

use eframe::egui::{self, Color32};
use std::sync::atomic::{AtomicBool, Ordering};

static DARK: AtomicBool = AtomicBool::new(false);

pub(crate) fn is_dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

pub(crate) fn set_dark(on: bool) {
    DARK.store(on, Ordering::Relaxed);
}

fn pick(light: Color32, dark: Color32) -> Color32 {
    if is_dark() { dark } else { light }
}

/// Фон окна и панелей.
pub(crate) fn ui_bg() -> Color32 {
    pick(Color32::from_rgb(0xF2, 0xF5, 0xF9), Color32::from_rgb(0x0F, 0x13, 0x19))
}

/// Карточки, списки, поля ввода.
pub(crate) fn ui_surface() -> Color32 {
    pick(Color32::WHITE, Color32::from_rgb(0x18, 0x1E, 0x27))
}

/// Наведение на строку и вторичные заливки.
pub(crate) fn ui_surface_alt() -> Color32 {
    pick(Color32::from_rgb(0xEA, 0xEF, 0xF6), Color32::from_rgb(0x23, 0x2B, 0x36))
}

/// Основной текст.
pub(crate) fn ui_ink() -> Color32 {
    pick(Color32::from_rgb(0x24, 0x30, 0x41), Color32::from_rgb(0xE7, 0xEC, 0xF3))
}

/// Подписи и второстепенный текст.
pub(crate) fn ui_muted() -> Color32 {
    pick(Color32::from_rgb(0x5E, 0x6C, 0x7E), Color32::from_rgb(0x93, 0xA1, 0xB3))
}

/// Разделители и рамки.
pub(crate) fn ui_line() -> Color32 {
    pick(Color32::from_rgb(0xDA, 0xE1, 0xEA), Color32::from_rgb(0x2B, 0x34, 0x41))
}

/// Акцент: выбранная вкладка, ссылки, главные кнопки.
pub(crate) fn ui_accent() -> Color32 {
    pick(Color32::from_rgb(0x25, 0x63, 0xBE), Color32::from_rgb(0x5B, 0x9C, 0xFF))
}

/// Заливка выбранной строки.
pub(crate) fn ui_selected() -> Color32 {
    pick(Color32::from_rgb(0xE7, 0xF0, 0xFE), Color32::from_rgb(0x1E, 0x33, 0x50))
}

/// Заливка строки, которая сейчас на экране показа.
pub(crate) fn ui_live() -> Color32 {
    pick(Color32::from_rgb(0xE5, 0xF4, 0xEA), Color32::from_rgb(0x16, 0x32, 0x1F))
}

/// Тень карточек и всплывающих окон.
pub(crate) fn ui_shadow() -> egui::epaint::Shadow {
    egui::epaint::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: if is_dark() { Color32::from_black_alpha(120) } else { Color32::from_black_alpha(26) },
    }
}

/// Тёмный заголовок окна Windows. Без него тёмная тема выглядит светлой рамкой.
#[cfg(windows)]
pub(crate) fn apply_titlebar(hwnd: isize, dark: bool) {
    if hwnd == 0 {
        return;
    }
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(hwnd: isize, attr: u32, value: *const i32, size: u32) -> i32;
    }
    #[link(name = "user32")]
    extern "system" {
        fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
    }
    let value: i32 = if dark { 1 } else { 0 };
    let size = std::mem::size_of::<i32>() as u32;
    unsafe {
        // 20 — Windows 10 2004 и новее, 19 — более ранние сборки.
        let _ = DwmSetWindowAttribute(hwnd, 20, &value, size);
        let _ = DwmSetWindowAttribute(hwnd, 19, &value, size);
        // Заголовок перерисовывается не сразу — просим DWM обновить рамку.
        const SWP: u32 = 0x0001 | 0x0002 | 0x0004 | 0x0010 | 0x0020; // NOSIZE|NOMOVE|NOZORDER|NOACTIVATE|FRAMECHANGED
        let _ = SetWindowPos(hwnd, 0, 0, 0, 0, 0, SWP);
    }
}

#[cfg(not(windows))]
pub(crate) fn apply_titlebar(_hwnd: isize, _dark: bool) {}

/// Единый стиль интерфейса. Вызывается при запуске и при смене темы.
pub(crate) fn configure(ctx: &egui::Context) {
    ctx.set_theme(if is_dark() { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.style_mut(|s| {
        s.spacing.button_padding = egui::vec2(10.0, 6.0);
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.interact_size = egui::vec2(36.0, 30.0);
        for (kind, size) in [
            (egui::TextStyle::Body, 14.5),
            (egui::TextStyle::Button, 14.0),
            (egui::TextStyle::Small, 12.0),
            (egui::TextStyle::Heading, 21.0),
        ] {
            s.text_styles.insert(kind, egui::FontId::proportional(size));
        }
        s.visuals.override_text_color = Some(ui_ink());
        s.visuals.panel_fill = ui_bg();
        s.visuals.window_fill = ui_surface();
        s.visuals.extreme_bg_color = ui_surface();
        s.visuals.faint_bg_color = ui_bg();
        s.visuals.window_shadow = ui_shadow();
        s.visuals.popup_shadow = ui_shadow();
        s.visuals.window_corner_radius = egui::CornerRadius::same(14);
        s.visuals.menu_corner_radius = egui::CornerRadius::same(10);
        s.visuals.selection.bg_fill = ui_selected();
        s.visuals.selection.stroke = egui::Stroke::new(1.0_f32, ui_accent());
        let widgets = &mut s.visuals.widgets;
        for w in [&mut widgets.inactive, &mut widgets.hovered, &mut widgets.active] {
            w.corner_radius = egui::CornerRadius::same(8);
        }
        widgets.inactive.bg_fill = ui_surface_alt();
        widgets.inactive.weak_bg_fill = ui_surface_alt();
        widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, ui_line());
        widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, ui_ink());
        widgets.hovered.bg_fill = ui_selected();
        widgets.hovered.weak_bg_fill = ui_selected();
        widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, ui_accent());
        widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, ui_ink());
        widgets.active.bg_fill = ui_selected();
        widgets.active.weak_bg_fill = ui_selected();
        widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, ui_accent());
        widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, ui_ink());
    });
}
