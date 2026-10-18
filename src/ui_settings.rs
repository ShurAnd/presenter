//! Вкладка «Настройки»: экран показа, запуск, управление, журнал. Всё, что не про сеть
//! (прежде лежало на вкладке «Сеть»).

use super::*;

impl App {
    pub(crate) fn settings_panel(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new().fill(Color32::from_gray(0xf5)).corner_radius(5.0).inner_margin(14.0).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.label(RichText::new("Настройки").size(18.0).strong());

                section(ui, "Экран показа");
                ui.label(RichText::new("Монитор и «в окне» — для этого компьютера, когда он сервер; фон у клиента меняется на сервере.").size(11.5).color(GRAY));
                // Монитор для показа — на сервере: основной или любой из подключённых.
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Монитор для показа:");
                    let monitors = display::list();
                    let current = monitors.iter().position(|m| m.device == self.cfg.monitor);
                    let shown = match current {
                        Some(i) => monitors[i].label(i + 1),
                        None if self.cfg.monitor.is_empty() => "основной".into(),
                        None => format!("{} — не подключён, показ на основном", self.cfg.monitor),
                    };
                    let mut pick: Option<String> = None;
                    egui::ComboBox::from_id_salt("monitor").selected_text(shown).width(330.0).show_ui(ui, |ui| {
                        if ui.selectable_label(self.cfg.monitor.is_empty(), "основной").clicked() {
                            pick = Some(String::new());
                        }
                        for (i, m) in monitors.iter().enumerate() {
                            if ui.selectable_label(current == Some(i), m.label(i + 1)).clicked() {
                                pick = Some(m.device.clone());
                            }
                        }
                    });
                    if let Some(d) = pick {
                        self.cfg.monitor = d;
                        self.save_cfg();
                    }
                })
                .response
                .on_hover_text("На какой монитор сервера выводить фон, часы, PDF, видео и PowerPoint: основной или любой из подключённых");

                // Фон экрана показа — у клиента меняется на сервере.
                ui.add_space(4.0);
                let current = self.view().background;
                ui.horizontal(|ui| {
                    ui.label("Фон экрана показа:");
                    ui.label(RichText::new(if current.is_empty() { "стандартный" } else { current.as_str() }).strong());
                    self.background_menu(ui, &current);
                })
                .response
                .on_hover_text("Картинка (PNG, JPG, BMP) или анимация (GIF, анимированные WebP и PNG). Копируется в папку настроек сервера; прежний выбранный фон заменяется");

                ui.add_space(4.0);
                ui.checkbox(&mut self.cfg.windowed, "Сцена и показ в окне, а не на весь экран (только для теста на одном компьютере)");

                section(ui, "Запуск");
                let mut auto = self.autostart;
                let resp = ui.checkbox(&mut auto, "Запускать сервер при входе в Windows (с самовосстановлением после сбоя)").on_hover_text(
                    r"Сервер стартует сам при входе в Windows. Сторож следит за ним: упал или завис — перезапустит, с тем же списком. Закрыли сервер сами — сторож тоже завершится. Журнал: %APPDATA%\presenter\watchdog.log",
                );
                if resp.changed() {
                    match watchdog::set_autostart(auto) {
                        Ok(()) => self.autostart = auto,
                        Err(e) => self.status = format!("Не удалось изменить автозапуск: {e}"),
                    }
                }

                section(ui, "Управление");
                let mut kb = self.cfg.keyboard;
                if ui
                    .checkbox(&mut kb, "Клавиши в окне программы управляют показом (то же, что кнопка ⌨ вверху)")
                    .on_hover_text("Стрелки и PageUp/PageDown — слайды, пробел — следующий шаг или старт/пауза видео, цифры + Enter — переход к слайду")
                    .changed()
                {
                    self.cfg.keyboard = kb;
                    self.goto_buf.clear();
                    self.save_cfg();
                }

                section(ui, "Журнал событий");
                let log = journal::path();
                ui.label(RichText::new("Что открывали, ответы PowerPoint, ошибки, обрывы связи — если что-то не сработало, пришлите этот файл.").size(12.0).color(Color32::from_gray(0x55)));
                ui.horizontal(|ui| {
                    if ui.button("Открыть папку журнала").clicked() {
                        let _ = std::process::Command::new("explorer").arg(format!("/select,{}", log.display())).spawn();
                    }
                    ui.label(RichText::new(log.display().to_string()).size(11.5).color(GRAY));
                });

                ui.add_space(14.0);
                ui.label(RichText::new(format!("Presentator v{}", env!("CARGO_PKG_VERSION"))).size(11.5).color(GRAY));
            });
        });
    }
}

/// Заголовок раздела настроек.
fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(12.0);
    ui.label(RichText::new(title).strong().size(15.0));
    ui.separator();
}
