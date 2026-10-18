//! Вкладка «Сеть».

use super::*;

impl App {
    pub(crate) fn network_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Frame::new().fill(Color32::from_gray(0xf5)).corner_radius(5.0).inner_margin(14.0).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.label(RichText::new("🌐 Сетевое взаимодействие").size(18.0).strong());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let (t, c) = if self.server.is_some() { ("■ Остановить сервер", RED) } else { ("▶ Запустить сервер", BLUE) };
                if btn(ui, t, c).on_hover_text("Запустить или остановить сервер в этом окне").clicked() {
                    self.toggle_server(&ctx);
                }
                let (t, c) = if self.client.is_some() { ("🔌 Отключиться", RED) } else { ("🔌 Подключиться", GREEN) };
                if btn(ui, t, c).on_hover_text("Подключиться к серверу и управлять им").clicked() {
                    self.toggle_client(&ctx);
                }
            });
            ui.add_space(4.0);
            let st = if self.net_status.is_empty() { "⚫ Подключения нет" } else { self.net_status.as_str() };
            ui.label(RichText::new(st).size(14.0).strong());
            ui.add_space(8.0);

            let locked = self.server.is_some() || self.client.is_some();
            egui::Frame::new().fill(Color32::WHITE).corner_radius(5.0).inner_margin(12.0).show(ui, |ui| {
                ui.add_enabled_ui(!locked, |ui| {
                    egui::Grid::new("net").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                        // Сохранённые серверы: выбор подставляет адрес, порт, пароль и название.
                        let current = format!("{}:{}", self.cfg.ip.trim(), self.cfg.port.trim());
                        let known = self.cfg.servers.iter().position(|s| s.key() == current);
                        ui.label("Сохранённые серверы:");
                        ui.horizontal(|ui| {
                            let shown = match known {
                                Some(i) => self.cfg.servers[i].label(),
                                None if self.cfg.servers.is_empty() => "пока пусто — сервер запомнится после подключения".into(),
                                None => "выберите сервер…".into(),
                            };
                            let mut pick = None;
                            egui::ComboBox::from_id_salt("servers").selected_text(shown).width(300.0).show_ui(ui, |ui| {
                                for (i, srv) in self.cfg.servers.iter().enumerate() {
                                    if ui.selectable_label(known == Some(i), srv.label()).clicked() {
                                        pick = Some(i);
                                    }
                                }
                            });
                            if let Some(i) = pick {
                                let srv = self.cfg.servers[i].clone();
                                (self.cfg.ip, self.cfg.port, self.cfg.password, self.cfg.name) = (srv.ip, srv.port, srv.password, srv.name);
                            }
                            if ui.add_enabled(known.is_some(), egui::Button::new("Забыть")).on_hover_text("Убрать этот сервер из списка").clicked() {
                                if let Some(i) = known {
                                    let srv = self.cfg.servers.remove(i);
                                    self.forgotten.insert(srv.key());
                                    self.save_cfg();
                                }
                            }
                        });
                        ui.end_row();
                        ui.label("🏷 Название (необязательно):");
                        ui.add(egui::TextEdit::singleline(&mut self.cfg.name).hint_text("Например: Зал 1").desired_width(220.0));
                        ui.end_row();
                        ui.label("📡 IP адрес сервера:");
                        ui.add(egui::TextEdit::singleline(&mut self.cfg.ip).hint_text("Например: 192.168.1.100").desired_width(220.0));
                        ui.end_row();
                        ui.label("🔌 Порт:");
                        ui.add(egui::TextEdit::singleline(&mut self.cfg.port).hint_text("14999").desired_width(90.0));
                        ui.end_row();
                        ui.label("🔑 Пароль доступа:");
                        ui.add(egui::TextEdit::singleline(&mut self.cfg.password).password(true).desired_width(220.0));
                        ui.end_row();
                    });
                });
            });
            // Резервный сервер — просто настройка, менять можно и во время подключения.
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Резервный сервер:");
                let reserve = self.cfg.servers.iter().find(|s| s.key() == self.cfg.reserve).map(SavedServer::label);
                let mut pick: Option<String> = None;
                egui::ComboBox::from_id_salt("reserve")
                    .selected_text(reserve.unwrap_or_else(|| "не выбран".into()))
                    .width(300.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(self.cfg.reserve.is_empty(), "не выбран").clicked() {
                            pick = Some(String::new());
                        }
                        for srv in &self.cfg.servers {
                            if ui.selectable_label(self.cfg.reserve == srv.key(), srv.label()).clicked() {
                                pick = Some(srv.key());
                            }
                        }
                    });
                if let Some(k) = pick {
                    self.cfg.reserve = k;
                    self.save_cfg();
                }
                if ui.button("Добавить…").on_hover_text("Ввести резервный сервер, которого нет в списке").clicked() {
                    self.reserve_form = Some(SavedServer { port: "14999".into(), password: "default_password".into(), ..Default::default() });
                }
            })
            .response
            .on_hover_text("Если с основным сервером беда, на резервный можно переключиться одной кнопкой — в красной полосе или в окне показателей");
            self.reserve_window(&ctx);
            if !self.cfg.reserve.is_empty() {
                let mut m = self.cfg.mirror;
                if ui
                    .checkbox(&mut m, "Дублировать загружаемые файлы на резервный сервер")
                    .on_hover_text("Файлы, которые вы добавляете, сразу уходят и на резерв — при переключении там откроется та же презентация на том же слайде")
                    .changed()
                {
                    self.cfg.mirror = m;
                    self.save_cfg();
                }
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "💡 На компьютере, который выводит показ на монитор, нажмите «Запустить сервер».\n\
                     На управляющем компьютере выберите сервер из списка или введите его IP и нажмите «Подключиться».\n\
                     Серверы, к которым удалось подключиться, запоминаются в списке.\n\
                     Для теста на одном компьютере оставьте IP 127.0.0.1.",
                )
                .size(12.0)
                .color(Color32::from_gray(0x55)),
            );
        });
    }
}
