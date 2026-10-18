//! Превью слайдов у оператора, видео внутри слайда PowerPoint и панель увеличения.

use super::*;

/// Кружок с номером видео на слайде (как рамка на миниатюре): зелёный — играет, синий — стоит.
pub(crate) fn media_badge(ui: &mut egui::Ui, n: usize, state: u8) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
    let color = if state == 0 { GREEN } else { BLUE };
    ui.painter().circle_filled(r.center(), 10.0, color);
    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, n.to_string(), egui::FontId::proportional(12.0), Color32::WHITE);
}

/// Миниатюра нужна: предыдущий слайд, текущий и два следующих (при листании назад не ждём).
pub(crate) fn thumb_near(page: usize, cur: usize) -> bool {
    page + 1 >= cur && page <= cur + 2
}

/// Ширина панели увеличения справа от превью и размер клетки крестовины.
pub(crate) const ZOOM_PANEL_W: f32 = 160.0;
pub(crate) const ZOOM_CELL: f32 = 28.0;

/// Кнопка панели увеличения: синий квадрат, значок рисуем сами строго по центру
/// (символы шрифта в кнопке стоят неровно, а стрелок в шрифте egui нет вовсе).
pub(crate) fn icon_button(ui: &mut egui::Ui, size: egui::Vec2, enabled: bool, icon: impl FnOnce(&egui::Painter, egui::Pos2, Color32)) -> egui::Response {
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let (bg, fg) = match (enabled, resp.hovered(), resp.is_pointer_button_down_on()) {
        (false, ..) => (Color32::from_gray(0xee), Color32::from_gray(0xbb)),
        (true, _, true) => (Color32::from_rgb(0x2f, 0x8f, 0xad), Color32::WHITE),
        (true, true, false) => (Color32::from_rgb(0x46, 0xa8, 0xc6), Color32::WHITE),
        (true, false, false) => (BLUE, Color32::WHITE),
    };
    let p = ui.painter();
    p.rect_filled(rect, 4.0, bg);
    icon(p, rect.center(), fg);
    resp
}

/// Кнопка-стрелка крестовины. `dir`: 0 — вверх, 1 — вправо, 2 — вниз, 3 — влево.
pub(crate) fn arrow_button(ui: &mut egui::Ui, dir: u8, enabled: bool) -> egui::Response {
    icon_button(ui, egui::vec2(ZOOM_CELL, ZOOM_CELL), enabled, |p, c, fg| {
        let (a, b) = (7.0, 5.0);
        let pts = match dir {
            0 => vec![c + egui::vec2(0.0, -b), c + egui::vec2(a, b), c + egui::vec2(-a, b)],
            1 => vec![c + egui::vec2(b, 0.0), c + egui::vec2(-b, a), c + egui::vec2(-b, -a)],
            2 => vec![c + egui::vec2(0.0, b), c + egui::vec2(-a, -b), c + egui::vec2(a, -b)],
            _ => vec![c + egui::vec2(-b, 0.0), c + egui::vec2(b, -a), c + egui::vec2(b, a)],
        };
        p.add(egui::Shape::convex_polygon(pts, fg, egui::Stroke::NONE));
    })
}

impl App {
    /// Превью у оператора: миниатюры предыдущего, текущего и следующего слайда.
    pub(crate) fn preview_bar(&mut self, ui: &mut egui::Ui, view: &StateMsg) {
        let Some(id) = view.active else { return };
        const GAP: f32 = 12.0;
        // Увеличение — у PDF и картинок (их рисуем сами); панель справа от миниатюр.
        let kind = view.items.iter().find(|i| i.id == id).map(|i| i.kind.as_str()).unwrap_or("");
        let zoomable = kind == "PDF" || kind == KIND_IMAGE || kind == "PPTX";
        let panel = if zoomable { ZOOM_PANEL_W + GAP } else { 0.0 };
        // Три миниатюры 16:9 по ширине окна, но не выше 112 точек.
        // Учитываем и промежутки между элементами строки — иначе панель увеличения уезжала за край.
        let spacing = 4.0 * ui.spacing().item_spacing.x;
        let h = ((ui.available_width() - 2.0 * GAP - panel - spacing) / 3.0 * 9.0 / 16.0).clamp(50.0, 112.0);
        let zview = (view.zoom > 1.0).then_some(view.view);
        // Видео и звук на слайде PowerPoint — списком над миниатюрами, номера — как рамки на «Сейчас».
        if !view.media.is_empty() {
            self.media_list(ui, &view.media);
            ui.add_space(4.0);
        }
        let slot = |ui: &mut egui::Ui, title: String, tex: Option<&egui::TextureHandle>, empty: &str, current: bool| -> egui::Rect {
            ui.vertical(|ui| {
                let color = if current { Color32::from_gray(0x22) } else { Color32::from_gray(0x77) };
                ui.label(RichText::new(title).size(12.0).color(color).strong());
                // Слайды не 16:9 вписываются в ту же рамку.
                let frame = egui::vec2(h * 16.0 / 9.0, h);
                let (rect, _) = ui.allocate_exact_size(frame, egui::Sense::hover());
                let p = ui.painter();
                p.rect_filled(rect, 3.0, Color32::from_gray(0xe8));
                let rect = tex.map_or(rect, |t| {
                    let s = t.size_vec2();
                    let k = (frame.x / s.x.max(1.0)).min(frame.y / s.y.max(1.0));
                    egui::Rect::from_center_size(rect.center(), s * k)
                });
                match tex {
                    Some(t) => {
                        p.image(t.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    None => {
                        p.text(rect.center(), egui::Align2::CENTER_CENTER, empty, egui::FontId::proportional(12.0), GRAY);
                    }
                }
                // Текущий слайд — в зелёной рамке, как активная строка списка.
                let stroke = if current { egui::Stroke::new(3.0_f32, GREEN) } else { egui::Stroke::new(1.0_f32, Color32::from_gray(0xbb)) };
                p.rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Outside);
                // Увеличен — оранжевой рамкой та часть слайда, что сейчас на экране показа.
                if let (true, Some(v), Some(_)) = (current, zview, tex) {
                    let r = egui::Rect::from_min_max(rect.lerp_inside(egui::vec2(v[0], v[1])), rect.lerp_inside(egui::vec2(v[2], v[3])));
                    p.rect_stroke(r, 0.0, egui::Stroke::new(2.0_f32, ZOOM_FRAME), egui::StrokeKind::Inside);
                }
                rect
            })
            .inner
        };
        ui.horizontal(|ui| {
            let page = view.page;
            match page.checked_sub(1) {
                Some(prev) => slot(ui, format!("Предыдущий — слайд {}", prev + 1), self.previews.get(&(id, prev)), "готовится…", false),
                None => slot(ui, "Предыдущий".into(), None, "начало презентации", false),
            };
            ui.add_space(GAP);
            let cur = slot(ui, format!("Сейчас — слайд {}", page + 1), self.previews.get(&(id, page)), "готовится…", true);
            self.media_marks(ui, cur, &view.media);
            ui.add_space(GAP);
            if page + 1 < view.pages {
                slot(ui, format!("Далее — слайд {}", page + 2), self.previews.get(&(id, page + 1)), "готовится…", false);
            } else {
                slot(ui, "Далее".into(), None, "конец презентации", false);
            }
            if zoomable {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| self.zoom_panel(ui, view));
            }
        });
    }

    /// Видео и звук на слайде PowerPoint — все сразу, каждое своей строкой: номер (как рамка на
    /// миниатюре «Сейчас»), имя, состояние, время, «Старт / Пауза», «Стоп». Мыши на экране показа нет.
    pub(crate) fn media_list(&mut self, ui: &mut egui::Ui, media: &[MediaInfo]) {
        let mut cmds: Vec<Cmd> = Vec::new();
        let mut hover = None;
        let playing: Vec<i32> = media.iter().filter(|m| m.state == 0).map(|m| m.id).collect();
        ui.horizontal(|ui| {
            let videos = media.iter().filter(|m| m.video).count();
            let title = match (videos, media.len() - videos) {
                (v, 0) => format!("Видео на слайде: {v}"),
                (0, a) => format!("Звук на слайде: {a}"),
                (v, a) => format!("На слайде: видео {v}, звук {a}"),
            };
            ui.label(RichText::new(title).strong().size(14.0));
            ui.label(RichText::new("номера — как рамки на миниатюре «Сейчас»; щелчок по рамке — старт или пауза").size(11.5).color(GRAY));
            if playing.len() > 1 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if btn(ui, "■ Стоп всех", RED).clicked() {
                        cmds.extend(playing.iter().map(|&id| Cmd::PptMedia { id, action: MediaAction::Stop }));
                    }
                });
            }
        });
        // Больше четырёх — прокрутка, чтобы список не съедал окно.
        egui::ScrollArea::vertical().max_height(4.0 * (28.0 + ui.spacing().item_spacing.y) + 2.0).auto_shrink([false, true]).show(ui, |ui| {
            for (i, m) in media.iter().enumerate() {
                let row = ui.horizontal(|ui| {
                    ui.set_min_height(28.0);
                    media_badge(ui, i + 1, m.state);
                    let what = if m.video { "" } else { "звук · " };
                    let name = if m.name.trim().is_empty() { format!("Видео {}", i + 1) } else { m.name.clone() };
                    ui.allocate_ui_with_layout(egui::vec2(190.0, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_width(190.0);
                        ui.add(egui::Label::new(RichText::new(format!("{what}{name}")).size(13.0)).truncate()).on_hover_text(&m.name);
                    });
                    let (state, color) = match m.state {
                        0 => ("играет", GREEN),
                        1 => ("пауза", BLUE),
                        // Остановлено кнопкой или доиграло до конца.
                        2 => ("остановлено", GRAY),
                        // До первого запуска PowerPoint сообщает «не готово» (3) — запускать при этом можно.
                        _ => ("не запущено", GRAY),
                    };
                    ui.add_sized([86.0, 20.0], egui::Label::new(RichText::new(state).color(color).strong()));
                    let t = |s: u32| format!("{:02}:{:02}", s / 60, s % 60);
                    let time = if m.dur > 0 { format!("{} / {}", t(m.pos), t(m.dur)) } else { String::new() };
                    ui.add_sized([96.0, 20.0], egui::Label::new(RichText::new(time).size(12.0).color(Color32::from_gray(0x55))));
                    let size = egui::vec2(112.0, 0.0);
                    if m.state == 0 {
                        if ui.add(cbutton("⏸ Пауза", BLUE).min_size(size)).clicked() {
                            cmds.push(Cmd::PptMedia { id: m.id, action: MediaAction::Pause });
                        }
                    } else {
                        let label = if m.state == 1 { "▶ Продолжить" } else { "▶ Старт" };
                        if ui.add(cbutton(label, GREEN).min_size(size)).on_hover_text("Запустить на экране показа — как щелчок по видео").clicked() {
                            cmds.push(Cmd::PptMedia { id: m.id, action: MediaAction::Play });
                        }
                    }
                    if ui.add_enabled(m.state <= 1, cbutton("■ Стоп", RED)).on_hover_text("Остановить и вернуть в начало").clicked() {
                        cmds.push(Cmd::PptMedia { id: m.id, action: MediaAction::Stop });
                    }
                });
                if row.response.hovered() || ui.rect_contains_pointer(row.response.rect) {
                    hover = Some(i);
                }
            }
        });
        self.media_hover = hover;
        for c in cmds {
            self.act(c);
        }
    }

    /// Рамки видео на миниатюре «Сейчас»: номер как в списке; синяя — стоит, зелёная — играет.
    /// Щелчок по рамке — старт или пауза этого видео, как щелчок по нему на слайде.
    pub(crate) fn media_marks(&mut self, ui: &mut egui::Ui, rect: egui::Rect, media: &[MediaInfo]) {
        let mut cmd = None;
        for (i, m) in media.iter().enumerate() {
            let r = egui::Rect::from_min_max(rect.lerp_inside(egui::vec2(m.rect[0], m.rect[1])), rect.lerp_inside(egui::vec2(m.rect[2], m.rect[3]))).intersect(rect);
            // Совсем мелкое (иконка звука) — хотя бы 14 точек, чтобы попасть мышью.
            let r = egui::Rect::from_center_size(r.center(), r.size().max(egui::vec2(14.0, 14.0)));
            let resp = ui.interact(r, ui.id().with(("media_mark", i)), egui::Sense::click());
            let color = if m.state == 0 { GREEN } else { BLUE };
            let hot = resp.hovered() || self.media_hover == Some(i);
            let p = ui.painter();
            if hot {
                p.rect_filled(r, 2.0, color.gamma_multiply(0.25));
            }
            p.rect_stroke(r, 2.0, egui::Stroke::new(if hot { 3.0_f32 } else { 2.0 }, color), egui::StrokeKind::Inside);
            let c = r.left_top() + egui::vec2(9.0, 9.0);
            p.circle_filled(c, 8.0, color);
            p.text(c, egui::Align2::CENTER_CENTER, (i + 1).to_string(), egui::FontId::proportional(11.0), Color32::WHITE);
            let action = if m.state == 0 { MediaAction::Pause } else { MediaAction::Play };
            let tip = format!("{} {} — {}", if m.video { "Видео" } else { "Звук" }, i + 1, if m.state == 0 { "щелчок — пауза" } else { "щелчок — старт" });
            if resp.on_hover_text(tip).clicked() {
                cmd = Some(Cmd::PptMedia { id: m.id, action });
            }
        }
        if let Some(c) = cmd {
            self.act(c);
        }
    }

    /// Увеличение слайда: «+», масштаб, «−» и крестовина стрелок с «весь слайд» в центре.
    pub(crate) fn zoom_panel(&mut self, ui: &mut egui::Ui, view: &StateMsg) {
        let zoom = if view.zoom > 0.0 { view.zoom } else { 1.0 };
        let (can_in, can_out, zoomed) = (zoom < ZOOMS[ZOOMS.len() - 1] - 0.01, zoom > 1.01, zoom > 1.01);
        // Двигать можно, только если в ту сторону есть что показать.
        let v = view.view;
        let (left, right, up, down) = (zoomed && v[0] > 0.001, zoomed && v[2] < 0.999, zoomed && v[1] > 0.001, zoomed && v[3] < 0.999);
        let mut cmd = None;
        ui.vertical(|ui| {
            ui.set_width(ZOOM_PANEL_W);
            let title = if zoomed { format!("Увеличение — {:.0} %", zoom * 100.0) } else { "Увеличение".into() };
            ui.label(RichText::new(title).size(12.0).color(if zoomed { ZOOM_FRAME } else { Color32::from_gray(0x77) }).strong());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                // «+» и «−» столбиком.
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
                    // Две кнопки на высоту крестовины (3 клетки и 2 промежутка).
                    let size = egui::vec2(40.0, (ZOOM_CELL * 3.0 + 3.0) / 2.0);
                    if icon_button(ui, size, can_in, |p, c, fg| {
                        let s = egui::Stroke::new(3.0_f32, fg);
                        p.line_segment([c - egui::vec2(8.0, 0.0), c + egui::vec2(8.0, 0.0)], s);
                        p.line_segment([c - egui::vec2(0.0, 8.0), c + egui::vec2(0.0, 8.0)], s);
                    })
                    .on_hover_text("Крупнее (клавиша «+»)")
                    .clicked()
                    {
                        cmd = Some(Cmd::Zoom { step: 1 });
                    }
                    if icon_button(ui, size, can_out, |p, c, fg| {
                        p.line_segment([c - egui::vec2(8.0, 0.0), c + egui::vec2(8.0, 0.0)], egui::Stroke::new(3.0_f32, fg));
                    })
                    .on_hover_text("Мельче (клавиша «−»)")
                    .clicked()
                    {
                        cmd = Some(Cmd::Zoom { step: -1 });
                    }
                });
                ui.add_space(6.0);
                // Крестовина: стрелки — сдвиг увеличенного слайда, в центре — снова весь слайд.
                egui::Grid::new("zoom_pad").spacing([3.0, 3.0]).show(ui, |ui| {
                    ui.label("");
                    if arrow_button(ui, 0, up).on_hover_text("Выше (Ctrl+стрелка вверх)").clicked() {
                        cmd = Some(Cmd::Pan { dx: 0, dy: -1 });
                    }
                    ui.label("");
                    ui.end_row();
                    if arrow_button(ui, 3, left).on_hover_text("Левее (Ctrl+стрелка влево)").clicked() {
                        cmd = Some(Cmd::Pan { dx: -1, dy: 0 });
                    }
                    // Центр — «весь слайд»: рамка экрана.
                    if icon_button(ui, egui::vec2(ZOOM_CELL, ZOOM_CELL), zoomed, |p, c, fg| {
                        p.rect_stroke(egui::Rect::from_center_size(c, egui::vec2(14.0, 10.0)), 1.0, egui::Stroke::new(2.0_f32, fg), egui::StrokeKind::Middle);
                    })
                    .on_hover_text("Весь слайд")
                    .clicked()
                    {
                        cmd = Some(Cmd::Zoom { step: 0 });
                    }
                    if arrow_button(ui, 1, right).on_hover_text("Правее (Ctrl+стрелка вправо)").clicked() {
                        cmd = Some(Cmd::Pan { dx: 1, dy: 0 });
                    }
                    ui.end_row();
                    ui.label("");
                    if arrow_button(ui, 2, down).on_hover_text("Ниже (Ctrl+стрелка вниз)").clicked() {
                        cmd = Some(Cmd::Pan { dx: 0, dy: 1 });
                    }
                    ui.label("");
                    ui.end_row();
                });
            });
        });
        if let Some(c) = cmd {
            self.act(c);
        }
    }
}
