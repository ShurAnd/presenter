//! Вкладка «Плеер».

use super::*;

impl App {
    /// Вкладка «Плеер»: режим и громкость, список треков, позиция и кнопки.
    pub(crate) fn player_panel(&mut self, ui: &mut egui::Ui, view: &StateMsg) {
        let a = view.audio.clone();
        // Режим и громкость.
        ui.horizontal(|ui| {
            if ui.selectable_label(!a.list, "Один трек").on_hover_text("Сыграть трек и остановиться").clicked() && a.list {
                self.act(Cmd::AudioMode { list: false, repeat: a.repeat });
            }
            if ui.selectable_label(a.list, "По списку").on_hover_text("После трека — следующий; после последнего — снова первый").clicked() && !a.list {
                self.act(Cmd::AudioMode { list: true, repeat: a.repeat });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut v = a.volume as f32;
                ui.spacing_mut().slider_width = 130.0;
                if ui.add(egui::Slider::new(&mut v, 0.0..=100.0).show_value(false)).on_hover_text(format!("{} %", a.volume)).changed() {
                    self.act(Cmd::AudioVolume { volume: v.round() as u8 });
                }
                ui.label(format!("Громкость {} %", a.volume));
            });
        });
        ui.add_space(4.0);
        // Позиция и кнопки — внизу, список занимает остальное.
        egui::TopBottomPanel::bottom("player_controls").frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(0, 6))).show_inside(ui, |ui| self.player_controls(ui, &a));
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0_f32, Color32::from_gray(0xcc)))
            .corner_radius(5.0)
            .inner_margin(6.0)
            .fill(Color32::WHITE)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                if a.tracks.is_empty() {
                    ui.centered_and_justified(|ui| {
                        let t = "Список плеера пуст.\nНажмите «Добавить» или перетащите сюда музыку (mp3, wav, flac, ogg, m4a, wma…) — например, фоновую музыку и фанфары.";
                        ui.label(RichText::new(t).color(GRAY).size(14.0));
                    });
                    return;
                }
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for (n, t) in a.tracks.iter().enumerate() {
                        let is_cur = a.current == Some(t.id) && a.state != AudioState::Stopped;
                        let is_ins = a.insert == Some(t.id);
                        let is_sel = self.audio_selected == Some(t.id);
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::click());
                        let bg = if is_cur || is_ins {
                            Color32::from_rgb(0xd4, 0xed, 0xda)
                        } else if is_sel {
                            Color32::from_rgb(0xd9, 0xee, 0xf7)
                        } else if resp.hovered() {
                            Color32::from_gray(0xf3)
                        } else {
                            Color32::WHITE
                        };
                        let p = ui.painter();
                        p.rect_filled(rect, 3.0, bg);
                        p.line_segment([rect.left_bottom(), rect.right_bottom()], egui::Stroke::new(1.0_f32, Color32::from_gray(0xee)));
                        let mark = if is_cur || is_ins { "▶ " } else { "" };
                        p.text(rect.left_center() + egui::vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, format!("{mark}{}. {}", n + 1, t.name), egui::FontId::proportional(14.0), Color32::from_gray(0x22));
                        let info = if is_ins {
                            Some((format!("вставка {}", audio_time(a.insert_pos, a.insert_dur)), INSERT))
                        } else if is_cur {
                            Some(match a.state {
                                AudioState::Playing if a.insert.is_some() => (format!("ждёт вставку · {}", audio_time(a.pos, a.dur)), BLUE),
                                AudioState::Playing => (format!("играет {}", audio_time(a.pos, a.dur)), GREEN),
                                AudioState::Paused if a.insert.is_some() => (format!("ждёт вставку · {}", audio_time(a.pos, a.dur)), BLUE),
                                AudioState::Paused => (format!("пауза {}", audio_time(a.pos, a.dur)), BLUE),
                                _ => ("затихает…".to_string(), GRAY),
                            })
                        } else {
                            None
                        };
                        if let Some((text, color)) = info {
                            p.text(rect.right_center() - egui::vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, text, egui::FontId::proportional(12.0), color);
                        }
                        if resp.clicked() {
                            self.audio_selected = Some(t.id);
                        }
                        if resp.double_clicked() {
                            self.audio_selected = Some(t.id);
                            self.act(Cmd::AudioPlay { id: t.id });
                        }
                    }
                });
            });
    }

    /// Позиция (с перемоткой) и кнопки плеера.
    pub(crate) fn player_controls(&mut self, ui: &mut egui::Ui, a: &AudioInfo) {
        let sel = self.audio_selected.filter(|id| a.tracks.iter().any(|t| t.id == *id));
        let ins = a.insert.is_some();
        // Позиция того, что звучит: вставки, если она идёт, иначе музыки.
        let (pos, dur, name) = if ins { (a.insert_pos, a.insert_dur, a.insert) } else { (a.pos, a.dur, a.current) };
        let active = if ins { a.insert_state != AudioState::Stopped } else { a.state != AudioState::Stopped };
        let name = name.and_then(|id| a.tracks.iter().find(|t| t.id == id)).map(|t| t.name.as_str()).filter(|_| active);
        ui.horizontal(|ui| {
            let title = match name {
                Some(n) if ins => format!("Вставка: {n}"),
                Some(n) => n.to_string(),
                None => "Ничего не играет".into(),
            };
            ui.add(egui::Label::new(RichText::new(title).strong().color(if ins { INSERT } else { Color32::from_gray(0x33) })).truncate());
        });
        ui.horizontal(|ui| {
            let t = |s: u32| format!("{:02}:{:02}", s / 60, s % 60);
            ui.label(t(self.audio_seek.map_or(pos, |v| v as u32)));
            ui.spacing_mut().slider_width = (ui.available_width() - 60.0).max(60.0);
            let mut v = self.audio_seek.unwrap_or(pos as f32);
            let r = ui.add_enabled(active && dur > 0, egui::Slider::new(&mut v, 0.0..=dur.max(1) as f32).show_value(false));
            if r.dragged() {
                self.audio_seek = Some(v);
            }
            if r.drag_stopped() || r.clicked() {
                self.audio_seek = None;
                self.act(Cmd::AudioSeek { secs: v as u32 });
            }
            ui.label(t(dur));
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(!a.tracks.is_empty(), cbutton("◀ Пред.", BLUE)).clicked() {
                self.act(Cmd::AudioPrev);
            }
            let (label, color, cmd) = match (ins, if ins { a.insert_state } else { a.state }) {
                (_, AudioState::Playing) => ("⏸ Пауза", BLUE, Cmd::AudioToggle),
                (_, AudioState::Paused) => ("▶ Продолжить", GREEN, Cmd::AudioToggle),
                _ => match sel.or(a.current).or(a.tracks.first().map(|t| t.id)) {
                    Some(id) => ("▶ Старт", GREEN, Cmd::AudioPlay { id }),
                    None => ("▶ Старт", GREEN, Cmd::AudioToggle),
                },
            };
            if ui.add_enabled(!a.tracks.is_empty(), cbutton(label, color)).on_hover_text("Двойной щелчок по треку — играть его").clicked() {
                self.act(cmd);
            }
            let playing = a.state != AudioState::Stopped || ins;
            let stopping = a.state == AudioState::Stopping || a.insert_state == AudioState::Stopping;
            let tip = if stopping { "Остановить сразу" } else { "Остановить с плавным затуханием (повторное нажатие — сразу)" };
            if ui.add_enabled(playing, cbutton("■ Стоп", RED)).on_hover_text(tip).clicked() {
                self.act(Cmd::AudioStop);
            }
            if ui.add_enabled(!a.tracks.is_empty(), cbutton("След. ▶", BLUE)).clicked() {
                self.act(Cmd::AudioNext);
            }
            ui.add_space(10.0);
            // «Повтор» — переключатель: включён (синий) — текущий трек по кругу при любом режиме.
            let (fill, tip) = if a.repeat {
                (BLUE, "Повтор включён: текущий трек играет по кругу. Нажмите, чтобы выключить")
            } else {
                (GRAY, "Повтор выключен. Нажмите — текущий трек будет играть по кругу (и в режиме «По списку»)")
            };
            if btn(ui, "Повтор", fill).on_hover_text(tip).clicked() {
                self.act(Cmd::AudioMode { list: a.list, repeat: !a.repeat });
            }
            // «Вставкой»: пока вставка звучит — та же кнопка обрывает её, и музыка продолжается.
            if ins {
                if btn(ui, "■ Стоп вставки", INSERT).on_hover_text("Оборвать вставку — музыка продолжится с того же места").clicked() {
                    self.act(Cmd::AudioEndInsert);
                }
            } else if ui
                .add_enabled(sel.is_some(), cbutton("Вставкой", INSERT))
                .on_hover_text("Сыграть выбранный трек один раз (фанфары): музыка встанет на паузу и после него продолжится с того же места. Повторное нажатие — оборвать вставку")
                .clicked()
            {
                self.act(Cmd::AudioInsert { id: sel.unwrap() });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(sel.is_some(), cbutton("Удалить", GRAY)).clicked() {
                    if let Some(id) = self.audio_selected {
                        let name = a.tracks.iter().find(|t| t.id == id).map(|t| t.name.clone()).unwrap_or_default();
                        self.confirm = Some((format!("Удалить «{name}» из плеера? Файл удалится и на сервере."), "Удалить", Cmd::AudioRemove { id }));
                    }
                }
            });
        });
        // Пока звучит — обновлять позицию (сервер присылает её раз в секунду).
        if active {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
    }
}
