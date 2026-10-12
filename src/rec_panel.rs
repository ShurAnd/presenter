//! Вкладка «Запись»: выбор устройств, проверка сигнала, запись, уровень звука и превью.
//! Запись всегда идёт на этом компьютере (у клиента — у оператора), сервер о ней не знает.

use super::*;
use record::{Mode, RecEvent, Recorder, SessionEv};

/// «Без видео» / «без звука» в настройках; пусто — ещё не выбрано (подберём сами).
const NONE: &str = "-";
/// Сколько тишины подряд считать «нет звука».
const SILENCE: Duration = Duration::from_secs(10);
/// Тише этого (LUFS) — тишина.
const SILENT_LUFS: f32 = -60.0;

pub struct RecUi {
    tx: Sender<RecEvent>,
    rx: Receiver<RecEvent>,
    devices: Option<Result<(Vec<String>, Vec<String>), String>>,
    scanning: bool,
    modes: HashMap<String, Vec<Mode>>,
    modes_busy: HashSet<String>,
    encoder: Option<Result<(String, String), String>>,
    /// Идущий сеанс и проверка ли это (без файла).
    session: Option<Recorder>,
    checking: bool,
    gen: u64,
    secs: f64,
    bytes: u64,
    fps: f32,
    dropped: u64,
    level: Option<f32>,
    silent_since: Option<Instant>,
    preview: Option<egui::TextureHandle>,
    saving: bool,
    message: Option<(String, Color32)>,
    last_file: Option<PathBuf>,
    free: Option<(u64, Instant)>,
    /// Окно «идёт запись — выйти?» и выход после сохранения.
    confirm_quit: bool,
    quit_after: bool,
}

impl RecUi {
    pub fn new() -> RecUi {
        let (tx, rx) = channel();
        RecUi {
            tx,
            rx,
            devices: None,
            scanning: false,
            modes: HashMap::new(),
            modes_busy: HashSet::new(),
            encoder: None,
            session: None,
            checking: false,
            gen: 0,
            secs: 0.0,
            bytes: 0,
            fps: 0.0,
            dropped: 0,
            level: None,
            silent_since: None,
            preview: None,
            saving: false,
            message: None,
            last_file: None,
            free: None,
            confirm_quit: false,
            quit_after: false,
        }
    }

    /// Остановить и дождаться (выход из программы).
    pub fn finish(&mut self) {
        if let Some(s) = self.session.take() {
            s.stop_and_wait();
        }
    }

    /// Идёт запись в файл (не проверка).
    pub fn recording(&self) -> bool {
        self.session.is_some() && !self.checking
    }
}

impl App {
    fn rec_dir(&self) -> PathBuf {
        if self.cfg.rec_dir.trim().is_empty() { self.work_dir.join("Записи") } else { PathBuf::from(self.cfg.rec_dir.trim()) }
    }

    fn rec_scan(&mut self) {
        if self.rec.scanning {
            return;
        }
        self.rec.scanning = true;
        let ctx = self.ctx.clone();
        record::scan(self.rec.tx.clone(), move || ctx.request_repaint());
    }

    fn rec_scan_modes(&mut self, device: &str) {
        if device.is_empty() || device == NONE || self.rec.modes.contains_key(device) || !self.rec.modes_busy.insert(device.to_string()) {
            return;
        }
        let ctx = self.ctx.clone();
        record::scan_modes(device.to_string(), self.rec.tx.clone(), move || ctx.request_repaint());
    }

    /// События записи — в начале кадра.
    pub(crate) fn poll_rec(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.rec.rx.try_recv() {
            match ev {
                RecEvent::Devices(res) => {
                    self.rec.scanning = false;
                    if let Ok((video, audio)) = &res {
                        self.rec_auto_pick(video, audio);
                    }
                    self.rec.devices = Some(res);
                    let v = self.cfg.rec_video.clone();
                    self.rec_scan_modes(&v);
                }
                RecEvent::Modes { device, modes } => {
                    self.rec.modes_busy.remove(&device);
                    self.rec.modes.insert(device, modes);
                }
                RecEvent::Encoder(res) => self.rec.encoder = Some(res),
                RecEvent::Session { gen, ev } if gen == self.rec.gen => match ev {
                    SessionEv::Progress { secs, bytes, fps, dropped } => (self.rec.secs, self.rec.bytes, self.rec.fps, self.rec.dropped) = (secs, bytes, fps, dropped),
                    SessionEv::Level(l) => {
                        self.rec.level = Some(l);
                        if l > SILENT_LUFS {
                            self.rec.silent_since = None;
                        } else if self.rec.silent_since.is_none() {
                            self.rec.silent_since = Some(Instant::now());
                        }
                    }
                    SessionEv::Preview(img) => match &mut self.rec.preview {
                        Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                        None => self.rec.preview = Some(ctx.load_texture("rec_preview", img, egui::TextureOptions::LINEAR)),
                    },
                    SessionEv::Saving => self.rec.saving = true,
                    SessionEv::Done(res) => {
                        let checking = self.rec.checking;
                        self.rec.session = None;
                        self.rec.saving = false;
                        self.rec.level = None;
                        self.rec.silent_since = None;
                        self.rec.message = Some(match res {
                            Ok(Some(path)) => {
                                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                                self.rec.last_file = Some(path);
                                (format!("Запись сохранена: {name} ({})", size_label(size)), GREEN)
                            }
                            Ok(None) => ("Проверка остановлена".into(), GRAY),
                            Err(e) if checking => (format!("Проверка не удалась: {e}"), RED),
                            Err(e) => (format!("Запись не удалась: {e}"), RED),
                        });
                        if self.rec.quit_after {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                },
                RecEvent::Session { .. } => {}
            }
        }
    }

    /// Ничего не выбрано — карта захвата (по названию) или первое устройство; звук — того же
    /// устройства (у карт HDMI звук отдельным «микрофоном» с тем же названием в скобках).
    fn rec_auto_pick(&mut self, video: &[String], audio: &[String]) {
        let mut changed = false;
        if self.cfg.rec_video.is_empty() || (self.cfg.rec_video != NONE && !video.contains(&self.cfg.rec_video)) {
            let hint = |n: &String| ["hdmi", "capture", "usb video", "cam link", "elgato"].iter().any(|h| n.to_lowercase().contains(h));
            let pick = video.iter().find(|n| hint(n)).or(video.first()).cloned().unwrap_or(NONE.into());
            changed |= pick != self.cfg.rec_video;
            self.cfg.rec_video = pick;
        }
        if self.cfg.rec_audio.is_empty() || (self.cfg.rec_audio != NONE && !audio.contains(&self.cfg.rec_audio)) {
            let v = self.cfg.rec_video.to_lowercase();
            let pick = audio
                .iter()
                .find(|a| v != NONE && a.to_lowercase().contains(&v))
                .or_else(|| audio.iter().find(|a| ["hdmi", "capture", "digital audio interface"].iter().any(|h| a.to_lowercase().contains(h))))
                .or(audio.first())
                .cloned()
                .unwrap_or(NONE.into());
            changed |= pick != self.cfg.rec_audio;
            self.cfg.rec_audio = pick;
        }
        if changed {
            self.save_cfg();
        }
    }

    fn rec_mode(&self) -> Option<Mode> {
        let modes = self.rec.modes.get(&self.cfg.rec_video)?;
        modes.iter().find(|m| m.label() == self.cfg.rec_mode).or_else(|| record::best_mode(modes)).cloned()
    }

    fn rec_start(&mut self, check: bool) {
        // Устройство не делится между двумя FFmpeg — сначала закончить проверку.
        if let Some(s) = self.rec.session.take() {
            s.stop_and_wait();
        }
        let Some(Ok((encoder, _))) = &self.rec.encoder else {
            self.rec.message = Some(("Подождите: идёт поиск устройств и кодировщика…".into(), GRAY));
            return;
        };
        let pick = |s: &str| (!s.is_empty() && s != NONE).then(|| s.to_string());
        let opts = record::Opts {
            video: pick(&self.cfg.rec_video),
            mode: self.rec_mode(),
            audio: pick(&self.cfg.rec_audio),
            dir: (!check).then(|| self.rec_dir()),
            encoder: encoder.clone(),
        };
        self.rec.gen += 1;
        let ctx = self.ctx.clone();
        match Recorder::start(opts, self.rec.gen, self.rec.tx.clone(), move || ctx.request_repaint()) {
            Ok(r) => {
                self.rec.session = Some(r);
                self.rec.checking = check;
                (self.rec.secs, self.rec.bytes, self.rec.fps, self.rec.dropped, self.rec.level, self.rec.silent_since) = (0.0, 0, 0.0, 0, None, None);
                self.rec.message = None;
                self.rec.free = None;
            }
            Err(e) => self.rec.message = Some((e, RED)),
        }
    }

    fn rec_stop(&mut self) {
        if let Some(s) = &self.rec.session {
            s.stop();
        }
    }

    /// Закрытие окна во время записи — спросить; после «Остановить и выйти» ждём сохранения.
    pub(crate) fn rec_close_guard(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && self.rec.recording() && !self.rec.quit_after {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.rec.confirm_quit = true;
        }
        if !self.rec.confirm_quit && !self.rec.quit_after {
            return;
        }
        egui::Window::new("Идёт запись").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            if self.rec.quit_after {
                ui.label(RichText::new("Сохраняю запись, программа закроется сама…").size(14.0));
                ui.spinner();
                return;
            }
            ui.label(RichText::new("Остановить запись и выйти из программы?").size(14.0));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if btn(ui, "Остановить запись и выйти", RED).clicked() {
                    self.rec.confirm_quit = false;
                    self.rec.quit_after = true;
                    self.rec_stop();
                }
                if btn(ui, "Продолжить запись", GREEN).clicked() {
                    self.rec.confirm_quit = false;
                }
            });
        });
    }

    /// Для строки вкладок: «Запись 01:23:45» (кружок рисует строка вкладок — в шрифте его нет).
    pub(crate) fn rec_badge(&self) -> Option<(String, Color32)> {
        let s = self.rec.session.as_ref()?;
        if self.rec.checking {
            return Some(("Проверка сигнала".into(), BLUE));
        }
        if self.rec.saving {
            return Some(("Сохраняю запись…".into(), GRAY));
        }
        Some((format!("Запись {}", hms(s.started.elapsed().as_secs())), RED))
    }

    pub(crate) fn record_panel(&mut self, ui: &mut egui::Ui) {
        if self.rec.devices.is_none() && !self.rec.scanning {
            self.rec_scan();
        }
        let busy = self.rec.session.is_some();
        let recording = self.rec.recording();
        if busy {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }
        let (video, audio) = match &self.rec.devices {
            Some(Ok((v, a))) => (v.clone(), a.clone()),
            _ => (Vec::new(), Vec::new()),
        };

        egui::Grid::new("rec_settings").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label("Видео:");
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    let cur = if self.cfg.rec_video == NONE || self.cfg.rec_video.is_empty() { "— без видео (только звук) —".to_string() } else { self.cfg.rec_video.clone() };
                    let mut pick = None;
                    egui::ComboBox::from_id_salt("rec_video").width(300.0).selected_text(cur).show_ui(ui, |ui| {
                        for d in &video {
                            if ui.selectable_label(*d == self.cfg.rec_video, d).clicked() {
                                pick = Some(d.clone());
                            }
                        }
                        if ui.selectable_label(self.cfg.rec_video == NONE, "— без видео (только звук) —").clicked() {
                            pick = Some(NONE.to_string());
                        }
                    });
                    if let Some(p) = pick {
                        self.cfg.rec_video = p.clone();
                        self.cfg.rec_mode.clear();
                        self.save_cfg();
                        self.rec_scan_modes(&p);
                    }
                    // Формат видео: «Авто» — лучший, что отдаёт устройство.
                    if self.cfg.rec_video != NONE {
                        let modes = self.rec.modes.get(&self.cfg.rec_video).cloned().unwrap_or_default();
                        let best = record::best_mode(&modes).map(Mode::label);
                        let cur = if self.cfg.rec_mode.is_empty() { format!("Авто{}", best.as_ref().map(|b| format!(" ({b})")).unwrap_or_default()) } else { self.cfg.rec_mode.clone() };
                        let mut sorted = modes.clone();
                        sorted.sort_by(|a, b| (b.w * b.h).cmp(&(a.w * a.h)).then(b.fps.parse::<f64>().unwrap_or(0.0).total_cmp(&a.fps.parse::<f64>().unwrap_or(0.0))));
                        let mut pick = None;
                        egui::ComboBox::from_id_salt("rec_mode").width(230.0).selected_text(cur).show_ui(ui, |ui| {
                            if ui.selectable_label(self.cfg.rec_mode.is_empty(), "Авто — лучший формат устройства").clicked() {
                                pick = Some(String::new());
                            }
                            for m in &sorted {
                                let l = m.label();
                                if ui.selectable_label(l == self.cfg.rec_mode, &l).clicked() {
                                    pick = Some(l);
                                }
                            }
                        });
                        if let Some(p) = pick {
                            self.cfg.rec_mode = p;
                            self.save_cfg();
                        }
                    }
                });
            });
            ui.end_row();

            ui.label("Звук:");
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    let cur = if self.cfg.rec_audio == NONE || self.cfg.rec_audio.is_empty() { "— без звука —".to_string() } else { self.cfg.rec_audio.clone() };
                    let mut pick = None;
                    egui::ComboBox::from_id_salt("rec_audio").width(300.0).selected_text(cur).show_ui(ui, |ui| {
                        for d in &audio {
                            if ui.selectable_label(*d == self.cfg.rec_audio, d).clicked() {
                                pick = Some(d.clone());
                            }
                        }
                        if ui.selectable_label(self.cfg.rec_audio == NONE, "— без звука —").clicked() {
                            pick = Some(NONE.to_string());
                        }
                    });
                    if let Some(p) = pick {
                        self.cfg.rec_audio = p;
                        self.save_cfg();
                    }
                    let label = if self.rec.scanning { "Ищу устройства…" } else { "Обновить список" };
                    if ui.add_enabled(!self.rec.scanning, egui::Button::new(label)).on_hover_text("Подключили карту захвата или микрофон — найти заново").clicked() {
                        self.rec.modes.clear();
                        self.rec.devices = None;
                        self.rec_scan();
                    }
                });
            });
            ui.end_row();

            ui.label("Папка:");
            ui.horizontal(|ui| {
                let dir = self.rec_dir();
                ui.add(egui::Label::new(RichText::new(dir.display().to_string()).color(Color32::from_gray(0x44))).truncate());
                if ui.add_enabled(!busy, egui::Button::new("Выбрать…")).clicked() {
                    if let Some(p) = rfd::FileDialog::new().set_directory(&dir).pick_folder() {
                        self.cfg.rec_dir = p.display().to_string();
                        self.save_cfg();
                        self.rec.free = None;
                    }
                }
                if ui.button("Открыть папку").clicked() {
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                }
            });
            ui.end_row();

            ui.label("Сжатие:");
            match &self.rec.encoder {
                Some(Ok((_, label))) => ui.label(RichText::new(label).color(Color32::from_gray(0x44))),
                Some(Err(e)) => ui.label(RichText::new(e).color(RED)),
                None => ui.label(RichText::new("проверяю…").color(GRAY)),
            };
            ui.end_row();
        });
        if let Some(Err(e)) = &self.rec.devices {
            ui.label(RichText::new(format!("Не удалось получить список устройств: {e}")).color(RED));
        } else if self.rec.devices.is_some() && video.is_empty() && audio.is_empty() {
            ui.label(RichText::new("Устройств записи не найдено. Подключите карту захвата и нажмите «Обновить список».").color(RED));
        }

        // Свободное место — раз в 5 с.
        let dir = self.rec_dir();
        if self.rec.free.is_none_or(|(_, at)| at.elapsed() > Duration::from_secs(5)) {
            self.rec.free = record::free_space(&dir).map(|f| (f, Instant::now()));
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if recording {
                let label = if self.rec.saving { "Сохраняю…" } else { "■ Остановить запись" };
                if ui.add_enabled(!self.rec.saving, cbutton(label, RED).min_size(egui::vec2(190.0, 34.0))).clicked() {
                    self.rec_stop();
                }
            } else {
                let label = "Начать запись";
                let resp = ui.add(cbutton(label, RED).min_size(egui::vec2(190.0, 34.0))).on_hover_text("Запись в файл на этом компьютере");
                // Белый кружок слева от надписи (символа «●» в шрифте egui нет).
                let font = egui::TextStyle::Button.resolve(ui.style());
                let text_w = ui.painter().layout_no_wrap(label.into(), font, Color32::WHITE).size().x;
                ui.painter().circle_filled(egui::pos2(resp.rect.center().x - text_w / 2.0 - 12.0, resp.rect.center().y), 6.0, Color32::WHITE);
                if resp.clicked() {
                    self.rec_start(false);
                }
                let (label, color) = if self.rec.checking { ("Остановить проверку", GRAY) } else { ("Проверить сигнал", BLUE) };
                if ui.add(cbutton(label, color).min_size(egui::vec2(160.0, 34.0))).on_hover_text("Картинка и уровень звука без записи в файл").clicked() {
                    if self.rec.checking {
                        self.rec_stop();
                    } else {
                        self.rec_start(true);
                    }
                }
            }
            ui.add_space(10.0);
            ui.vertical(|ui| {
                if let Some(s) = &self.rec.session {
                    if self.rec.checking {
                        ui.label(RichText::new("Проверка сигнала — в файл не пишется").color(BLUE).strong());
                    } else {
                        ui.label(RichText::new(format!("{}   {}", hms(s.started.elapsed().as_secs()), size_label(self.rec.bytes))).color(RED).strong().size(16.0));
                    }
                    let mut info = Vec::new();
                    if !self.rec.checking && self.cfg.rec_video != NONE && self.rec.fps > 0.0 {
                        info.push(format!("{:.0} к/с", self.rec.fps));
                    }
                    if self.rec.dropped > 0 {
                        info.push(format!("пропущено кадров: {}", self.rec.dropped));
                    }
                    if let Some(name) = s.path.as_ref().and_then(|p| p.file_name()) {
                        info.push(name.to_string_lossy().to_string());
                    }
                    ui.label(RichText::new(info.join(" · ")).color(GRAY));
                } else if let Some((free, _)) = self.rec.free {
                    let rate = record::bytes_per_sec(self.cfg.rec_video != NONE, self.rec_mode().as_ref());
                    ui.label(RichText::new(format!("Свободно на диске {} — хватит примерно на {}", size_label(free), hours_label(free as f64 / rate))).color(if free < 2 << 30 { RED } else { GRAY }));
                }
            });
        });
        if busy {
            if let Some((free, _)) = self.rec.free {
                if free < 2 << 30 {
                    ui.label(RichText::new(format!("Мало места на диске: свободно {}", size_label(free))).color(RED).strong());
                }
            }
        }
        if let Some((msg, color)) = &self.rec.message {
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(RichText::new(msg).color(*color)).wrap());
                if *color == GREEN {
                    if let Some(p) = &self.rec.last_file {
                        if ui.small_button("Показать в папке").clicked() {
                            let _ = std::process::Command::new("explorer").arg(format!("/select,{}", p.display())).spawn();
                        }
                    }
                }
            });
        }

        // Уровень звука.
        if busy && self.cfg.rec_audio != NONE {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Звук:");
                level_meter(ui, self.rec.level);
                match self.rec.level {
                    Some(l) if l > SILENT_LUFS => ui.label(RichText::new(format!("{l:.0} LUFS")).color(GRAY)),
                    Some(_) => ui.label(RichText::new("тишина").color(GRAY)),
                    None => ui.label(RichText::new("…").color(GRAY)),
                };
                if self.rec.silent_since.is_some_and(|t| t.elapsed() > SILENCE) {
                    ui.label(RichText::new("Нет звука — проверьте источник").color(RED).strong());
                }
            });
        }

        // Превью — на всё оставшееся место, 16:9.
        ui.add_space(6.0);
        let avail = ui.available_size();
        let tex = self.rec.preview.as_ref().filter(|_| busy && self.cfg.rec_video != NONE);
        let aspect = tex.map_or(16.0 / 9.0, |t| t.aspect_ratio());
        let w = avail.x.min((avail.y - 4.0).max(60.0) * aspect).max(120.0);
        let size = egui::vec2(w, w / aspect);
        ui.vertical_centered(|ui| {
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            ui.painter().rect_filled(rect, 4.0, Color32::from_gray(0x22));
            match tex {
                Some(t) => {
                    ui.painter().image(t.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                }
                None => {
                    let text = if self.cfg.rec_video == NONE {
                        "Запись только звука"
                    } else if busy {
                        "Жду картинку с устройства…"
                    } else {
                        "Нажмите «Проверить сигнал», чтобы увидеть картинку с устройства"
                    };
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(14.0), Color32::from_gray(0xaa));
                }
            }
        });
    }
}

/// Полоса уровня громкости: −60…0 LUFS; зелёная, ближе к максимуму — жёлтая и красная.
fn level_meter(ui: &mut egui::Ui, level: Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(260.0, 14.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, Color32::from_gray(0xdd));
    if let Some(l) = level {
        let k = ((l + 60.0) / 60.0).clamp(0.0, 1.0);
        let color = if l > -6.0 { RED } else if l > -14.0 { Color32::from_rgb(0xe0, 0xb0, 0x00) } else { GREEN };
        let mut r = rect;
        r.set_width(rect.width() * k);
        p.rect_filled(r, 3.0, color);
    }
}

fn hms(secs: u64) -> String {
    format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

fn size_label(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} ГБ", b / 1e9).replace('.', ",")
    } else if b >= 1e6 {
        format!("{:.0} МБ", b / 1e6)
    } else {
        format!("{:.0} КБ", b / 1e3)
    }
}

fn hours_label(secs: f64) -> String {
    let h = secs / 3600.0;
    if h >= 10.0 { format!("{h:.0} ч") } else if h >= 1.0 { format!("{h:.1} ч").replace('.', ",") } else { format!("{:.0} мин", secs / 60.0) }
}
