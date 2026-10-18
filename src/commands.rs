//! Команды и события: единая точка управления (`act` / `apply`), разбор событий потоков (`poll`),
//! список файлов (добавление, сохранение, восстановление), превью и увеличение слайда.

use super::*;

impl App {
    pub(crate) fn local_state(&self) -> StateMsg {
        StateMsg {
            items: self
                .items
                .iter()
                .map(|i| ItemInfo {
                    id: i.id,
                    name: i.name.clone(),
                    kind: i.kind.clone(),
                    pages: i.pages,
                    ready: matches!(i.status, Status::Ready),
                    status: match &i.status {
                        Status::Preparing => "обработка…".into(),
                        Status::Ready => String::new(),
                        Status::Error(e) => format!("ошибка: {e}"),
                    },
                    repeat: i.repeat,
                    linked: i.linked.clone(),
                })
                .collect(),
            active: self.show.as_ref().map(|s| s.id),
            page: self.show.as_ref().map_or(0, |s| s.page),
            pages: self.show.as_ref().map_or(0, |s| s.pages),
            message: self.status.clone(),
            video: self.show.as_ref().and_then(Show::video).map(|v| VideoInfo { state: v.state, pos: v.pos as u32, dur: v.dur as u32 }),
            screen: self.screen,
            timer: self.timer.info(),
            audio: self.player.info(),
            background: self.cfg.background.clone(),
            zoom: self.show.as_ref().map_or(1.0, |s| s.zoom),
            view: self.slide_view(),
            media: self.slide_media(),
            version: env!("CARGO_PKG_VERSION").into(),
            cursor: self.show_cursor,
        }
    }

    /// Видимая часть текущего слайда (доли страницы).
    pub(crate) fn slide_view(&self) -> [f32; 4] {
        match &self.show {
            Some(s) => render::viewport(self.slide_aspect, self.stage_px, s.zoom, s.center),
            None => [0.0, 0.0, 1.0, 1.0],
        }
    }

    /// Увеличение и сдвиг — только у того, что рисуем сами: PDF и картинки.
    pub(crate) fn zoomable(&self) -> bool {
        self.show.as_ref().is_some_and(|s| match &s.kind {
            ShowKind::Pdf(_) | ShowKind::Image(_) => true,
            ShowKind::Ppt(p) => p.phase == PptPhase::Shown,
            ShowKind::Video(_) => false,
        })
    }

    /// Видео и звук на текущем слайде PowerPoint (список от моста — только если он про этот слайд).
    pub(crate) fn slide_media(&self) -> Vec<MediaInfo> {
        match self.show.as_ref().map(|s| (s.page, &s.kind)) {
            Some((page, ShowKind::Ppt(p))) if p.media.0 == page + 1 => p
                .media
                .1
                .iter()
                .map(|m| MediaInfo { id: m.id, name: m.name.clone(), video: m.video, state: m.state, pos: m.pos, dur: m.dur, rect: m.rect })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// То, что показывает интерфейс: список сервера, если мы подключены клиентом, иначе свой.
    pub(crate) fn view(&self) -> StateMsg {
        if self.connected { self.remote.clone() } else { self.local_state() }
    }

    /// Единая точка управления: клиент отправляет команду на сервер, иначе выполняем у себя.
    pub(crate) fn act(&mut self, cmd: Cmd) {
        if self.connected {
            if let Some(c) = &self.client {
                c.send(CliReq::Cmd(cmd));
            }
        } else {
            self.apply(cmd);
        }
    }

    pub(crate) fn apply(&mut self, cmd: Cmd) {
        // В журнал — все команды, кроме ползунков (громкость, перемотка, сдвиг увеличенного слайда).
        if !matches!(cmd, Cmd::AudioVolume { .. } | Cmd::AudioSeek { .. } | Cmd::Pan { .. }) {
            jlog!("Команда: {cmd:?}");
        }
        self.dirty = true;
        match cmd {
            Cmd::Open { id } => {
                let Some(it) = self.items.iter().find(|i| i.id == id) else { return };
                if !matches!(it.status, Status::Ready) {
                    return;
                }
                let (pages, pdf, src, name, is_video, slides, repeat) =
                    (it.pages, it.pdf.clone(), it.src.clone(), it.name.clone(), it.kind == "ВИДЕО", it.slides.clone(), it.repeat);
                let is_image = it.kind == KIND_IMAGE;
                jlog!("Открыть «{}» ({}, {} стр.)", it.name, it.kind, it.pages);
                if !it.linked.is_empty() {
                    let warn = format!("В «{}» видео по ссылке на файлы, которых нет на этом компьютере — на экране их не будет: {}", it.name, it.linked.join(", "));
                    jlog!("{warn}");
                }
                let linked_warn = (!it.linked.is_empty()).then(|| format!("Внимание: в «{}» {} — их не будет на экране", it.name, linked_text(&it.linked)));
                // Прежний показ закрываем: на экране остаётся сцена с фоном,
                // новый показ появится на ней, только когда будет готов.
                self.close_show();
                self.stage = true;
                self.screen = Screen::Background;
                self.show_gen += 1;
                let gen = self.show_gen;
                let kind = match pdf {
                    Some(pdf) => ShowKind::Pdf(pdf),
                    // Картинка грузится в отдельном потоке; пока не готова — виден фон.
                    None if is_image => {
                        let ctx = self.ctx.clone();
                        let max_side = self.ctx.input(|i| i.max_texture_side) as u32;
                        render::load_image(id, gen, src.clone(), max_side, self.out_tx.clone(), move || ctx.request_repaint());
                        ShowKind::Image(None)
                    }
                    // Видео сразу загружается за фоном и ждёт пробела или «Старт».
                    None if is_video => {
                        let (tx, ctx) = (self.out_tx.clone(), self.ctx.clone());
                        let rect = if self.cfg.windowed { None } else { display::target(&self.cfg.monitor).map(|m| m.rect) };
                        let player = video::Video::start(&src, self.cfg.windowed, rect, repeat, move |ev| {
                            let _ = tx.send(Out::Video { gen, ev });
                            ctx.request_repaint();
                        });
                        ShowKind::Video(VideoShow { player, state: VideoState::Loading, pos: 0.0, dur: 0.0 })
                    }
                    None => {
                        let (tx, c) = (self.out_tx.clone(), self.ctx.clone());
                        ppt::install_esc_hook(move || {
                            let _ = tx.send(Out::ShowEsc);
                            c.request_repaint();
                        });
                        // Прежний показ мост закрывает сам, перед открытием нового — в том же PowerPoint.
                        // PowerPoint получает монитор, на который реально пойдёт показ: выбранный, а если его
                        // отключили — основной. Монитор не выбран — его собственную настройку не трогаем.
                        let monitor = if self.cfg.monitor.is_empty() || self.cfg.windowed {
                            String::new()
                        } else {
                            display::target(&self.cfg.monitor).map(|m| m.device).unwrap_or_default()
                        };
                        if let Err(e) = self.ppt_host.open(gen, &src, self.cfg.windowed, &monitor, slides.as_deref(), THUMB_W) {
                            self.status = format!("PowerPoint: {e}");
                            return;
                        }
                        self.status = format!("Открытие в PowerPoint: «{name}»…");
                        ShowKind::Ppt(PptShow { phase: PptPhase::Loading, hwnd: 0, zoom_img: None, zoom_busy: None, zoom_up: false, media: (0, Vec::new()) })
                    }
                };
                self.show = Some(Show { id, page: 0, pages, kind, gen: self.show_gen, opened: Instant::now(), zoom: 1.0, center: (0.5, 0.5) });
                if let Some(w) = linked_warn {
                    self.status = w;
                }
            }
            // Видео: стрелки перематывают на 10 с, Home — в начало.
            Cmd::Next | Cmd::Prev | Cmd::GoTo { .. } if self.show.as_ref().is_some_and(|s| s.video().is_some()) => {
                let v = &self.show.as_ref().and_then(Show::video).unwrap().player;
                match cmd {
                    Cmd::Next => v.seek(10.0),
                    Cmd::Prev => v.seek(-10.0),
                    Cmd::GoTo { page: 0 } => v.seek_to(0.0),
                    _ => {}
                }
            }
            Cmd::Play => match self.show.as_mut().map(|s| &mut s.kind) {
                Some(ShowKind::Video(v)) => match v.state {
                    VideoState::Armed => {
                        v.player.play();
                        v.state = VideoState::Playing;
                    }
                    VideoState::Playing | VideoState::Paused => {
                        v.player.toggle();
                        v.state = if v.state == VideoState::Playing { VideoState::Paused } else { VideoState::Playing };
                    }
                    VideoState::Loading => {}
                },
                // У презентаций пробел — следующий шаг, как в PowerPoint.
                _ => self.apply(Cmd::Next),
            },
            Cmd::Next | Cmd::Prev if self.show.as_ref().is_some_and(Show::is_ppt) => {
                // Увеличенный слайд — неподвижная картинка; листаем уже живое слайд-шоу.
                if let Some(s) = self.show.as_mut().filter(|s| s.zoom > 1.0) {
                    (s.zoom, s.center) = (1.0, (0.5, 0.5));
                }
                self.ppt_host.send(if matches!(cmd, Cmd::Next) { "next" } else { "prev" });
            }
            Cmd::Next => self.go(|s| (s.page + 1).min(s.pages.saturating_sub(1))),
            Cmd::Prev => self.go(|s| s.page.saturating_sub(1)),
            Cmd::Background | Cmd::Clock | Cmd::Timer => {
                self.close_show();
                self.stage = true;
                self.screen = match cmd {
                    Cmd::Clock => Screen::Clock,
                    Cmd::Timer => Screen::Timer,
                    _ => Screen::Background,
                };
            }
            c @ (Cmd::AudioPlay { .. }
            | Cmd::AudioInsert { .. }
            | Cmd::AudioEndInsert
            | Cmd::AudioToggle
            | Cmd::AudioStop
            | Cmd::AudioNext
            | Cmd::AudioPrev
            | Cmd::AudioSeek { .. }
            | Cmd::AudioMode { .. }
            | Cmd::AudioVolume { .. }
            | Cmd::AudioRemove { .. }
            | Cmd::AudioMove { .. }
            | Cmd::AudioClear) => {
                let mode = matches!(c, Cmd::AudioMode { .. } | Cmd::AudioVolume { .. });
                for f in self.player.apply(c) {
                    if f.starts_with(&self.work_dir) {
                        let _ = std::fs::remove_file(f);
                    }
                }
                // Режим и громкость плеера запоминаются (записываются при смене режима и при выходе).
                if mode {
                    let changed = (self.cfg.audio_list, self.cfg.audio_repeat) != (self.player.list, self.player.repeat);
                    (self.cfg.audio_list, self.cfg.audio_repeat, self.cfg.audio_volume) = (self.player.list, self.player.repeat, self.player.volume);
                    if changed {
                        self.save_cfg();
                    }
                }
            }
            Cmd::BackgroundReset => {
                background::reset();
                self.cfg.background.clear();
                self.save_cfg();
                self.bg_checked = false; // сцена перечитает фон
                self.status = "Фон: стандартный".into();
            }
            Cmd::TimerSet { countdown, secs } => self.timer.set(countdown, secs),
            Cmd::TimerRun { run } => self.timer.run(run),
            Cmd::TimerReset => self.timer.reset(),
            Cmd::GoTo { page } => self.go(|s| page.min(s.pages.saturating_sub(1))),
            Cmd::Close => self.close_show(),
            Cmd::Remove { id } => {
                if self.show.as_ref().is_some_and(|s| s.id == id) {
                    self.close_show();
                }
                if let Some(pos) = self.items.iter().position(|i| i.id == id) {
                    let it = self.items.remove(pos);
                    self.delete_files(&it);
                }
            }
            Cmd::Zoom { step } if self.zoomable() => {
                let view = self.slide_view();
                if let Some(s) = &mut self.show {
                    let i = ZOOMS.iter().position(|&z| z >= s.zoom - 0.01).unwrap_or(0) as i32;
                    let next = if step == 0 { 0 } else { (i + step.signum()).clamp(0, ZOOMS.len() as i32 - 1) };
                    // Увеличиваем вокруг того, что сейчас в центре экрана.
                    s.center = ((view[0] + view[2]) / 2.0, (view[1] + view[3]) / 2.0);
                    s.zoom = ZOOMS[next as usize];
                    if s.zoom <= 1.0 {
                        s.center = (0.5, 0.5);
                    }
                }
                self.clamp_center();
            }
            Cmd::Pan { dx, dy } if self.zoomable() => {
                let v = self.slide_view();
                if let Some(s) = &mut self.show {
                    // На треть видимой части — соседний кусок частично остаётся на экране, не теряешься.
                    s.center.0 += dx.signum() as f32 * (v[2] - v[0]) / 3.0;
                    s.center.1 += dy.signum() as f32 * (v[3] - v[1]) / 3.0;
                }
                self.clamp_center();
            }
            Cmd::Zoom { .. } | Cmd::Pan { .. } => {}
            Cmd::Cursor { on } => {
                self.show_cursor = on;
                self.ppt_host.set_cursor(on);
                video::set_cursor(on);
            }
            Cmd::PptMedia { id, action } => {
                if self.show.as_ref().is_some_and(Show::is_ppt) {
                    let a = match action {
                        MediaAction::Play => "play",
                        MediaAction::Pause => "pause",
                        MediaAction::Stop => "stop",
                    };
                    self.ppt_host.media(a, id);
                }
            }
            Cmd::VideoRepeat { id, on } => {
                if let Some(it) = self.items.iter_mut().find(|i| i.id == id) {
                    it.repeat = on;
                }
                // Видео уже открыто — повтор меняется сразу, без перезапуска.
                if let Some(ShowKind::Video(v)) = self.show.as_ref().filter(|s| s.id == id).map(|s| &s.kind) {
                    v.player.set_repeat(on);
                }
            }
            Cmd::Move { id, up } => {
                if let Some(pos) = self.items.iter().position(|i| i.id == id) {
                    if up && pos > 0 {
                        self.items.swap(pos, pos - 1);
                    } else if !up && pos + 1 < self.items.len() {
                        self.items.swap(pos, pos + 1);
                    }
                }
            }
            Cmd::Clear => {
                self.close_show();
                for it in std::mem::take(&mut self.items) {
                    self.delete_files(&it);
                }
            }
        }
    }

    pub(crate) fn go(&mut self, f: impl Fn(&Show) -> usize) {
        if let Some(s) = &mut self.show {
            let p = f(s);
            match &s.kind {
                // Позицию PowerPoint сообщит сам — в нём шаг может быть анимацией, а не слайдом.
                ShowKind::Ppt(_) => {
                    if p == s.page + 1 {
                        self.ppt_host.send("next");
                    } else if p + 1 == s.page {
                        self.ppt_host.send("prev");
                    } else if p != s.page {
                        self.ppt_host.send(&format!("goto {}", p + 1));
                    }
                }
                ShowKind::Pdf(_) => {
                    if p != s.page {
                        s.page = p;
                        // Новый слайд — снова целиком.
                        (s.zoom, s.center) = (1.0, (0.5, 0.5));
                        let id = s.id;
                        self.trim_pages(id, p);
                    }
                }
                ShowKind::Video(_) | ShowKind::Image(_) => {}
            }
        }
    }

    /// Закрывает показ; сцена (если она есть) остаётся и показывает фон.
    pub(crate) fn close_show(&mut self) {
        if self.show.take().is_some_and(|s| s.is_ppt()) {
            // Сцена лежит прямо под слайд-шоу, поэтому при его закрытии сразу виден фон.
            self.ppt_host.close();
        }
        self.goto_buf.clear();
        self.thumb_failed.clear();
        self.reset_pages(true);
    }

    /// Вывод слайд-шоу на экран — по шагам, чтобы запуск не был виден, плюс сторож зависаний.
    pub(crate) fn tick_ppt(&mut self, ctx: &egui::Context) {
        let Some(show) = &mut self.show else { return };
        let ShowKind::Ppt(ps) = &mut show.kind else { return };
        match ps.phase {
            PptPhase::Loading => {
                if show.opened.elapsed() >= PPT_TIMEOUT {
                    // PowerPoint не отвечает (завис, висит его диалог) — перезапускаем мост.
                    self.ppt_host.restart();
                    self.status = "PowerPoint не отвечает — мост перезапущен, откройте презентацию ещё раз".into();
                    self.close_show();
                    self.dirty = true;
                } else {
                    ctx.request_repaint_after(Duration::from_millis(500));
                }
            }
            PptPhase::Ready(t) => {
                if t.elapsed() >= PPT_SETTLE && show.opened.elapsed() >= SWITCH_BG {
                    ps.phase = PptPhase::Lowered(Instant::now()); // сцена опустится в stage_window
                }
                ctx.request_repaint_after(Duration::from_millis(30));
            }
            PptPhase::Lowered(t) => {
                if t.elapsed() >= PPT_LOWER {
                    let target = if self.cfg.windowed { None } else { display::target(&self.cfg.monitor).map(|m| m.rect) };
                    self.ppt_host.reveal(ps.hwnd, target);
                    ps.phase = PptPhase::Shown;
                } else {
                    ctx.request_repaint_after(Duration::from_millis(30));
                }
            }
            PptPhase::Shown => {}
        }
        self.tick_ppt_zoom();
    }

    /// Файл крупной картинки слайда (с нуля) — в папке слайдов этой презентации.
    pub(crate) fn ppt_zoom_file(&self, page: usize) -> Option<PathBuf> {
        let s = self.show.as_ref()?;
        let dir = self.items.iter().find(|i| i.id == s.id)?.slides.clone()?;
        Some(dir.join(format!("zoom_{}.jpg", page + 1)))
    }

    /// Увеличение PowerPoint: нужна крупная картинка слайда — с диска (если уже сохранена) или
    /// заказать мосту. Вернули весь слайд — слайд-шоу снова наверх (сцена под ним, как при открытии).
    pub(crate) fn tick_ppt_zoom(&mut self) {
        let want = self.show.as_ref().filter(|s| s.zoom > 1.0).map(|s| s.page);
        let file = want.and_then(|p| self.ppt_zoom_file(p));
        let width = (self.stage_px.0 * 3.0).clamp(1920.0, 7680.0) as u32;
        let (out, ctx) = (self.out_tx.clone(), self.ctx.clone());
        let Some(show) = &mut self.show else { return };
        let gen = show.gen;
        let on = show.ppt_zoom_on();
        let ShowKind::Ppt(p) = &mut show.kind else { return };
        if let (Some(page), Some(file)) = (want, file) {
            let have = p.zoom_img.as_ref().is_some_and(|(pg, _)| *pg == page);
            if !have && p.zoom_busy != Some(page) && p.phase == PptPhase::Shown {
                p.zoom_busy = Some(page);
                if file.is_file() {
                    render::load_ppt_zoom(gen, page, file, out, move || ctx.request_repaint());
                } else {
                    self.ppt_host.export_zoom(page + 1, width, &file);
                }
            }
        }
        // Увеличение сняли — сцена опускается и слайд-шоу выходит наверх (как при открытии показа).
        // Пока это происходит, сцена показывает тот же слайд целиком — фон не мелькает.
        if p.zoom_up && !on && p.phase == PptPhase::Shown {
            p.phase = PptPhase::Lowered(Instant::now());
        }
        p.zoom_up = on;
    }

    /// Сделать картинку фоном сцены (на сервере и в автономном режиме).
    pub(crate) fn set_background(&mut self, path: &Path, name: &str) {
        match background::set(path) {
            Ok(()) => {
                self.cfg.background = name.to_string();
                self.save_cfg();
                self.bg_checked = false; // сцена перечитает фон
                self.status = format!("Фон: «{name}»");
            }
            Err(e) => self.status = format!("Не удалось сменить фон: {e}"),
        }
        self.dirty = true;
    }

    /// «Выбрать фон…»: у клиента картинка уходит на сервер (и на резерв), иначе — фон этого компьютера.
    pub(crate) fn pick_background(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("Картинка или анимация (GIF, WebP, APNG)", &background::EXTS).pick_file() else { return };
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if self.connected {
            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !background::EXTS.contains(&ext.as_str()) {
                self.status = format!("Формат .{ext} не подходит для фона (нужен PNG, JPG, BMP, GIF или WebP)");
                return;
            }
            if let Some(c) = &self.client {
                c.send(CliReq::UploadBackground(path.clone()));
            }
            if let Some(m) = self.mirror_uploader() {
                m.send_background(path);
            }
            self.status = format!("Фон «{name}» отправляется на сервер…");
        } else {
            self.set_background(&path, &name);
        }
    }

    /// Превью: держим миниатюры рядом с текущим слайдом; на сервере — готовим миниатюры PPTX
    /// (из сохранённых картинок слайдов) и рассылаем клиентам предыдущую, текущую и следующую.
    /// Миниатюры PDF заказываются вместе со страницами в `stage_window`.
    pub(crate) fn tick_previews(&mut self, view: &StateMsg) {
        let cur = view.active.filter(|_| view.video.is_none()).map(|id| (id, view.page));
        let near = |k: &(u64, usize)| cur.is_some_and(|(id, p)| k.0 == id && thumb_near(k.1, p));
        self.previews.retain(|k, _| near(k));
        self.thumb_jpg.retain(|k, _| near(k));
        if self.connected {
            return;
        }
        let ppt = self.show.as_ref().filter(|s| s.is_ppt()).and_then(|s| {
            let dir = self.items.iter().find(|i| i.id == s.id)?.slides.clone()?;
            Some((s.id, s.page, s.pages, dir))
        });
        if let Some((id, page, pages, dir)) = ppt {
            // Текущий, следующий, предыдущий — в порядке важности.
            for k in [Some((id, page)), Some((id, page + 1)), page.checked_sub(1).map(|p| (id, p))].into_iter().flatten() {
                if k.1 < pages && !self.previews.contains_key(&k) && !self.thumb_busy.contains(&k) && !self.thumb_failed.contains(&k) {
                    // Картинки ещё нет — мост сохранит её и сообщит (`PptEvent::Slide`), тогда и прочитаем.
                    let file = dir.join(format!("{}.jpg", k.1 + 1));
                    if file.is_file() {
                        self.thumb_busy.insert(k);
                        let ctx = self.ctx.clone();
                        render::load_thumb(id, k.1, file, self.out_tx.clone(), move || ctx.request_repaint());
                    }
                }
            }
        }
        if let Some(srv) = &self.server {
            let set = cur.map_or(Vec::new(), |(id, page)| {
                [page.checked_sub(1), Some(page), Some(page + 1)].into_iter().flatten().filter_map(|p| self.thumb_jpg.get(&(id, p)).map(|j| (id, p, j.clone()))).collect()
            });
            srv.publish_thumbs(set);
        }
    }

    /// Держим в памяти только страницы рядом с текущей: PREFETCH_AHEAD вперёд и пару назад.
    /// Центр видимой части — не дальше, чем позволяет край слайда (иначе сдвиг «копился» бы за краем).
    pub(crate) fn clamp_center(&mut self) {
        let v = self.slide_view();
        if let Some(s) = &mut self.show {
            s.center = ((v[0] + v[2]) / 2.0, (v[1] + v[3]) / 2.0);
        }
    }

    pub(crate) fn trim_pages(&mut self, id: u64, cur: usize) {
        self.textures.retain(|k, _| k.id == id && k.page + 2 >= cur && k.page <= cur + PREFETCH_AHEAD);
    }

    pub(crate) fn reset_pages(&mut self, drop_last: bool) {
        self.renderer.cancel();
        self.textures.clear();
        self.failed.clear();
        self.render_error = None;
        if drop_last {
            self.last_tex = None;
        }
    }

    /// Список хранит только сервер, и у каждого порта свой файл: копии программы на одном
    /// компьютере (клиенты, второй сервер) работают из той же папки и не должны мешать друг другу.
    pub(crate) fn list_path(&self) -> PathBuf {
        self.work_dir.join(format!("list_{}.json", self.cfg.port.trim()))
    }

    /// Список плеера — рядом, в своём файле.
    pub(crate) fn audio_path(&self) -> PathBuf {
        self.work_dir.join(format!("audio_{}.json", self.cfg.port.trim()))
    }

    /// Сохранить список (порядок, имена, файлы) — он переживает перезапуск программы и сбой.
    pub(crate) fn save_list(&mut self) {
        if !self.server_online {
            return;
        }
        let list: Vec<SavedItem> = self.items.iter().map(|i| SavedItem { name: i.name.clone(), src: i.src.clone(), repeat: i.repeat }).collect();
        let audio: Vec<SavedItem> = self.player.tracks.iter().map(|t| SavedItem { name: t.name.clone(), src: t.src.clone(), repeat: false }).collect();
        let (list_path, audio_path) = (self.list_path(), self.audio_path());
        for (list, path, saved) in [(list, list_path, &mut self.saved_list), (audio, audio_path, &mut self.saved_audio)] {
            let Ok(text) = serde_json::to_string_pretty(&list) else { continue };
            if text == *saved {
                continue;
            }
            let _ = std::fs::create_dir_all(&self.work_dir);
            // Через временный файл: при сбое посреди записи старый список останется целым.
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, &text).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
                *saved = text;
            }
        }
    }

    /// Восстановить список при запуске сервера. Файлы, которых больше нет, пропускаются; файлы
    /// в рабочей папке, на которые не ссылается ни один список (недокачанные при сбое), удаляются.
    pub(crate) fn load_list(&mut self, ctx: &egui::Context) {
        // list.json — файл первой сборки с сохранением списка (до раздельных файлов по портам).
        let list: Vec<SavedItem> = std::fs::read_to_string(self.list_path())
            .or_else(|_| std::fs::read_to_string(self.work_dir.join("list.json")))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let audio: Vec<SavedItem> = std::fs::read_to_string(self.audio_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let is_list = |name: &str| name.starts_with("list_") || name.starts_with("list.json") || name.starts_with("audio_");
        let mut keep: HashSet<PathBuf> = HashSet::new();
        if let Ok(dir) = std::fs::read_dir(&self.work_dir) {
            for e in dir.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if is_list(&name) && name.ends_with(".json") {
                    let other: Vec<SavedItem> = std::fs::read_to_string(e.path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
                    keep.extend(other.into_iter().map(|i| i.src));
                }
            }
        }
        if let Ok(dir) = std::fs::read_dir(&self.work_dir) {
            for e in dir.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if p.is_file() && !is_list(&name) && !keep.contains(&p) {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
        // Картинки слайдов файлов, которых нет ни в одном списке, тоже не нужны.
        let used: HashSet<PathBuf> = keep.iter().map(|p| self.slides_dir(p)).collect();
        if let Ok(dir) = std::fs::read_dir(self.work_dir.join(SLIDES_DIR)) {
            for e in dir.flatten() {
                if !used.contains(&e.path()) {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        let mut missing = 0;
        for it in list.into_iter().chain(audio) {
            if it.src.is_file() {
                let before = self.items.len();
                self.add_local(it.src, Some(it.name), ctx);
                if self.items.len() > before {
                    if let Some(last) = self.items.last_mut() {
                        last.repeat = it.repeat;
                    }
                }
            } else {
                missing += 1;
            }
        }
        if !self.items.is_empty() || !self.player.tracks.is_empty() {
            self.status = format!("Список восстановлен (файлов: {}", self.items.len() + self.player.tracks.len());
            if missing > 0 {
                self.status += &format!(", не найдено: {missing}");
            }
            self.status += ")";
        }
    }

    /// Удаляем только то, что сами создали в рабочей папке (загруженное и сконвертированное).
    pub(crate) fn delete_files(&self, it: &Item) {
        for p in [Some(&it.src), it.pdf.as_ref()].into_iter().flatten() {
            if p.starts_with(&self.work_dir) {
                let _ = std::fs::remove_file(p);
            }
        }
        // Картинки слайдов — если этот же файл не остался в списке ещё раз.
        if let Some(dir) = &it.slides {
            if !self.items.iter().any(|i| i.slides.as_ref() == Some(dir)) {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    /// Папка для слайдов PPTX в картинках: своя для каждого файла (по пути, размеру и времени
    /// изменения). Картинки переживают перезапуск — PowerPoint второй раз их не делает.
    pub(crate) fn slides_dir(&self, src: &Path) -> PathBuf {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        src.hash(&mut h);
        if let Ok(m) = std::fs::metadata(src) {
            m.len().hash(&mut h);
            m.modified().ok().hash(&mut h);
        }
        self.work_dir.join(SLIDES_DIR).join(format!("{:016x}", h.finish()))
    }

    pub(crate) fn add_local(&mut self, path: PathBuf, name: Option<String>, ctx: &egui::Context) {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        // Звук — в плеер, а не в список презентаций.
        if AUDIO_EXTS.contains(&ext.as_str()) {
            let name = name.unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
            self.player.add(path, name);
            self.dirty = true;
            return;
        }
        let kind = match ext.as_str() {
            "pdf" => "PDF",
            e if background::EXTS.contains(&e) => KIND_IMAGE,
            "pptx" | "ppt" | "ppsx" | "pps" => "PPTX",
            e if VIDEO_EXTS.contains(&e) => "ВИДЕО",
            _ => {
                self.status = format!("Формат .{ext} не поддерживается (нужен PDF, PPTX, картинка, видео или музыка)");
                return;
            }
        };
        let id = self.next_id;
        self.next_id += 1;
        let name = name.unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        let slides = (kind == "PPTX").then(|| self.slides_dir(&path));
        self.items.push(Item { id, name, kind: kind.into(), src: path.clone(), pdf: None, pages: 0, status: Status::Preparing, slides, repeat: false, linked: Vec::new() });
        if kind == "PPTX" {
            // PowerPoint запускаем заранее (скрыто) — к показу он уже будет готов.
            self.ppt_host.prewarm();
        }
        let ctx = ctx.clone();
        render::prepare(id, path, self.out_tx.clone(), move || ctx.request_repaint());
        self.dirty = true;
    }

    /// Добавление файлов: клиент отправляет их на сервер, иначе добавляем к себе.
    pub(crate) fn add_files(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        for p in paths {
            if self.connected {
                if let Some(c) = &self.client {
                    c.send(CliReq::Upload(p.clone()));
                }
                // Тот же файл — и на резервный сервер, чтобы на нём было всё для переключения.
                if let Some(m) = self.mirror_uploader() {
                    m.send(p);
                }
            } else {
                self.add_local(p, None, ctx);
            }
        }
    }

    // ------------------------------------------------------------ события

    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(out) = self.out_rx.try_recv() {
            match out {
                Out::Prepared { id, result, linked } => {
                    if let Some(it) = self.items.iter_mut().find(|i| i.id == id) {
                        match result {
                            Ok((pdf, pages)) => {
                                it.pdf = pdf;
                                it.pages = pages;
                                it.status = Status::Ready;
                            }
                            Err(e) => {
                                jlog!("Файл «{}» не подготовлен: {e}", it.name);
                                it.status = Status::Error(e);
                            }
                        }
                        // Видео по ссылке — предупредить сразу, при загрузке, а не на мероприятии.
                        if !linked.is_empty() {
                            jlog!("В «{}» видео по ссылке, файлов нет: {}", it.name, linked.join(", "));
                            self.status = format!("Внимание: в «{}» {} — их не будет на экране. Вставьте видео в саму презентацию", it.name, linked_text(&linked));
                        }
                        it.linked = linked;
                        self.dirty = true;
                    }
                }
                Out::Ppt { gen, ev } => {
                    if self.show.as_ref().is_some_and(|s| s.gen == gen) {
                        // Позиция без изменений и сохранённые слайды рассылки не требуют.
                        self.dirty |= !matches!(ev, ppt::PptEvent::Pos(..) | ppt::PptEvent::Slide(_));
                        match ev {
                            ppt::PptEvent::Ready(total, hwnd) => {
                                jlog!("PowerPoint: показ готов, слайдов {total}");
                                self.status.clear();
                                if let Some(s) = &mut self.show {
                                    s.pages = total.max(1);
                                    if let ShowKind::Ppt(p) = &mut s.kind {
                                        p.phase = PptPhase::Ready(Instant::now());
                                        p.hwnd = hwnd;
                                    }
                                }
                            }
                            ppt::PptEvent::Pos(cur, total) => {
                                if let Some(s) = &mut self.show {
                                    let (page, pages) = (cur.saturating_sub(1), total.max(1));
                                    self.dirty |= (s.page, s.pages) != (page, pages);
                                    if page != s.page {
                                        (s.zoom, s.center) = (1.0, (0.5, 0.5));
                                    }
                                    (s.page, s.pages) = (page, pages);
                                }
                            }
                            // Мост сохранил слайд картинкой — превью может его прочитать.
                            ppt::PptEvent::Slide(n) => {
                                if let Some(s) = &self.show {
                                    self.thumb_failed.remove(&(s.id, n.saturating_sub(1)));
                                }
                            }
                            // Крупная картинка слайда сохранена — читаем её.
                            ppt::PptEvent::Zoom(n) => {
                                if let Some(path) = self.ppt_zoom_file(n.saturating_sub(1)) {
                                    let c = ctx.clone();
                                    render::load_ppt_zoom(gen, n.saturating_sub(1), path, self.out_tx.clone(), move || c.request_repaint());
                                }
                            }
                            ppt::PptEvent::Media(slide, list) => {
                                if let Some(ShowKind::Ppt(p)) = self.show.as_mut().map(|s| &mut s.kind) {
                                    p.media = (slide, list);
                                }
                                self.dirty = true;
                            }
                            ppt::PptEvent::ZoomFailed(e) => {
                                if let Some(ShowKind::Ppt(p)) = self.show.as_mut().map(|s| &mut s.kind) {
                                    p.zoom_busy = None;
                                }
                                self.status = format!("PowerPoint: не удалось увеличить слайд: {e}");
                            }
                            ppt::PptEvent::Ended => {
                                jlog!("PowerPoint: показ закончился (закрыт в PowerPoint)");
                                self.close_show();
                            }
                            ppt::PptEvent::Error(e) => {
                                self.close_show();
                                self.status = format!("PowerPoint: {e}");
                            }
                        }
                    }
                }
                Out::PptZoom { gen, page, image } => {
                    if let Some(s) = self.show.as_mut().filter(|s| s.gen == gen) {
                        if let ShowKind::Ppt(p) = &mut s.kind {
                            p.zoom_busy = None;
                            match image {
                                Ok(img) => p.zoom_img = Some((page, ctx.load_texture("ppt_zoom", img, egui::TextureOptions::LINEAR))),
                                Err(e) => self.status = format!("PowerPoint: не удалось прочитать увеличенный слайд: {e}"),
                            }
                            self.dirty = true;
                        }
                    }
                }
                Out::ShowEsc => self.esc_press(),
                Out::Image { gen, result } => {
                    if let Some(s) = self.show.as_mut().filter(|s| s.gen == gen) {
                        match result {
                            Ok(frames) if !frames.is_empty() => s.kind = ShowKind::Image(Some(background::Background::new(ctx, frames))),
                            Ok(_) => {}
                            Err(e) => self.status = format!("Картинка: {e}"),
                        }
                    }
                }
                Out::Background { gen, result } => {
                    if gen == self.bg_gen {
                        match result {
                            Ok(frames) if !frames.is_empty() => self.bg = Some(background::Background::new(ctx, frames)),
                            Ok(_) => {}
                            Err(e) => self.status = format!("Фон: {e}"),
                        }
                    }
                }
                Out::Audio(ev) => {
                    self.dirty |= self.player.on_event(ev);
                    if let Some(m) = self.player.message.take() {
                        self.status = m;
                        self.dirty = true;
                    }
                }
                Out::Health(h) => self.local_health = Some(h),
                Out::Video { gen, ev } => {
                    let Some(s) = self.show.as_mut().filter(|s| s.gen == gen) else { continue };
                    let ShowKind::Video(v) = &mut s.kind else { continue };
                    match ev {
                        video::VideoEvent::Ready(dur) => {
                            v.state = VideoState::Armed;
                            v.dur = dur;
                            self.dirty = true;
                        }
                        video::VideoEvent::Pos { pos, dur, playing } => {
                            let state = match v.state {
                                VideoState::Loading | VideoState::Armed => v.state,
                                _ if playing => VideoState::Playing,
                                _ => VideoState::Paused,
                            };
                            // Рассылаем клиентам только смену секунды или состояния.
                            if pos as u32 != v.pos as u32 || state != v.state || dur as u32 != v.dur as u32 {
                                self.dirty = true;
                            }
                            (v.pos, v.dur, v.state) = (pos, dur, state);
                        }
                        // Доиграло — на экране фон, а видео остаётся загруженным, в начале:
                        // пробел или «Старт» запускают его снова сразу. Закрыть — «Закрыть показ».
                        video::VideoEvent::Ended => {
                            (v.state, v.pos) = (VideoState::Armed, 0.0);
                            self.status = "Видео доиграло — снова готово к запуску (пробел или «Старт»)".into();
                            self.dirty = true;
                        }
                        video::VideoEvent::Error(e) => {
                            self.close_show();
                            self.status = format!("Видео: {e}");
                            self.dirty = true;
                        }
                    }
                }
                Out::Page { key, image } => {
                    let cur = self.show.as_ref().filter(|s| s.id == key.id).map(|s| s.page);
                    // Страница, до которой уже не долистать в пределах кеша, не нужна.
                    if let Some(cur) = cur.filter(|&c| key.page + 2 >= c && key.page <= c + PREFETCH_AHEAD) {
                        let tex = ctx.load_texture(format!("p{}_{}", key.id, key.page), image, egui::TextureOptions::LINEAR);
                        self.textures.insert(key, tex);
                        self.trim_pages(key.id, cur);
                    }
                }
                Out::PageErr { key, msg } => {
                    self.failed.insert(key);
                    if self.show.as_ref().is_some_and(|s| s.id == key.id && s.page == key.page) {
                        self.render_error = Some(msg);
                    }
                }
                Out::Thumb { id, page, image, jpg } => {
                    self.thumb_busy.remove(&(id, page));
                    // Пока миниатюра готовилась, могли перелистнуть — ненужную не держим.
                    if self.show.as_ref().is_some_and(|s| s.id == id && thumb_near(page, s.page)) {
                        let tex = ctx.load_texture(format!("t{id}_{page}"), image, egui::TextureOptions::LINEAR);
                        self.previews.insert((id, page), tex);
                        self.thumb_jpg.insert((id, page), Arc::new(jpg));
                    }
                }
                Out::ThumbErr { id, page } => {
                    self.thumb_busy.remove(&(id, page));
                    self.thumb_failed.insert((id, page));
                }
            }
        }

        while let Ok(ev) = self.mirror_rx.try_recv() {
            match ev {
                CliEvent::UploadOk(name) => self.status = format!("Файл «{name}» передан и на резервный сервер"),
                CliEvent::UploadErr { name, msg } => self.status = format!("Резервный сервер: не удалось передать «{name}»: {msg}"),
                _ => {}
            }
        }

        while let Ok(ev) = self.srv_rx.try_recv() {
            match ev {
                SrvEvent::Started => {
                    self.server_online = true;
                    // Сервер восстанавливает свой список (если в окне ещё ничего нет).
                    if self.items.is_empty() && self.player.tracks.is_empty() {
                        self.load_list(ctx);
                    }
                    // Файлы из `--add` — после восстановления списка (иначе он не восстановился бы).
                    let add = std::mem::take(&mut self.pending_add);
                    self.add_files(add, ctx);
                    self.dirty = true;
                    self.start_collector();
                    self.stage = true; // на мониторе показа сервера сразу выводим фон
                    self.net_status = format!("Сервер запущен на порту {}", self.cfg.port);
                    self.dirty = true;
                }
                SrvEvent::Failed(e) => {
                    self.server = None;
                    self.server_online = false;
                    self.net_status = format!("Не удалось запустить сервер: {e}");
                }
                SrvEvent::Clients(n) => {
                    jlog!("Клиентов подключено: {n}");
                    self.clients = n;
                }
                SrvEvent::Hello { version } => {
                    let ours = env!("CARGO_PKG_VERSION");
                    jlog!("Подключился клиент, версия {}", if version.is_empty() { "старая (без номера)" } else { &version });
                    if version != ours {
                        let v = if version.is_empty() { "старой версии".to_string() } else { format!("версии {version}") };
                        self.status = format!("Подключился клиент {v}, здесь {ours} — обновите программу, иначе часть кнопок не работает");
                    }
                }
                SrvEvent::Cmd(cmd) => {
                    if self.server.is_some() {
                        self.apply(cmd);
                    }
                }
                SrvEvent::Background { name, path } => {
                    if self.server.is_some() {
                        self.set_background(&path, &name);
                    }
                    let _ = std::fs::remove_file(&path); // копия уже в папке настроек
                }
                SrvEvent::File { name, path } => {
                    if self.server.is_some() {
                        self.status = format!("Получен файл: {name}");
                        self.add_local(path, Some(name), ctx);
                    }
                }
            }
        }

        while let Ok(ev) = self.cli_rx.try_recv() {
            match ev {
                CliEvent::Rtt(ms) => {
                    self.rtt = Some(ms);
                    self.remote_live = true;
                    self.last_seen = Some(Instant::now());
                }
                CliEvent::Health(h) => {
                    self.remote_health = Some(h);
                    self.remote_live = true;
                    self.last_seen = Some(Instant::now());
                    self.health_at = self.last_seen;
                }
                CliEvent::Connected => {
                    (self.remote_health, self.rtt, self.remote_live, self.health_at, self.lost) = (None, None, false, None, None);
                    self.reconnect = None;
                    // Файлы из `--add` — на сервер (и на резерв).
                    let add = std::mem::take(&mut self.pending_add);
                    if !add.is_empty() {
                        self.connected = true;
                        self.add_files(add, ctx);
                    }
                    self.last_seen = Some(Instant::now());
                    self.connected = true;
                    self.close_show();
                    self.previews.clear(); // номера элементов у сервера свои
                    self.net_status = format!("Подключено к {}:{}", self.cfg.ip, self.cfg.port);
                    self.status.clear();
                    self.remember_server();
                }
                CliEvent::Closed(reason) => {
                    // Обрыв не по нашей команде (пустая причина — мы отключились сами) — тревога
                    // и автопереподключение. Неудачная попытка — следующая через RECONNECT_EVERY.
                    let retrying = self.reconnect.is_some();
                    if (self.connected || retrying) && !reason.is_empty() {
                        if self.connected {
                            self.lost = Some(reason.clone());
                        }
                        let attempt = self.reconnect.map_or(0, |(n, _)| n);
                        self.reconnect = (attempt < RECONNECT_TRIES && reason != "Неверный пароль").then(|| (attempt, Instant::now() + RECONNECT_EVERY));
                    }
                    (self.remote_health, self.rtt, self.remote_live, self.last_seen, self.health_at) = (None, None, false, None, None);
                    self.connected = false;
                    self.client = None;
                    self.remote = StateMsg::default();
                    self.previews.clear();
                    (self.got_state, self.version_hidden) = (false, false);
                    self.net_status = if reason.is_empty() { "Отключено".into() } else { reason };
                }
                CliEvent::State(s) => {
                    self.last_seen = Some(Instant::now());
                    self.remote_at = Instant::now();
                    if s.message != self.remote.message {
                        self.status = s.message.clone();
                    }
                    if !self.got_state {
                        self.got_state = true;
                        let ours = env!("CARGO_PKG_VERSION");
                        if s.version != ours {
                            jlog!("Версия сервера {} не совпадает с этой ({ours})", if s.version.is_empty() { "старая (без номера)" } else { &s.version });
                        }
                    }
                    self.remote = s;
                    self.last_show = self.remote.active.and_then(|id| self.remote.items.iter().find(|i| i.id == id)).map(|it| (it.name.clone(), self.remote.page));
                    self.continue_resume();
                }
                CliEvent::Progress { name, sent, total } => {
                    // Пока идёт передача файла, пинги ждут своей очереди — это не молчание сервера.
                    self.last_seen = Some(Instant::now());
                    let pct = if total > 0 { sent * 100 / total } else { 100 };
                    self.status = format!("Отправка «{name}»: {pct}%");
                }
                CliEvent::UploadOk(name) => self.status = format!("Файл «{name}» передан на сервер"),
                CliEvent::UploadErr { name, msg } => self.status = format!("Не удалось передать «{name}»: {msg}"),
                CliEvent::Thumb { id, page, image } => {
                    if self.connected && self.remote.active == Some(id) {
                        let tex = ctx.load_texture(format!("t{id}_{page}"), image, egui::TextureOptions::LINEAR);
                        self.previews.insert((id, page), tex);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ сеть
}

/// «2 видео по ссылке на файлы, которых нет на сервере (ролик.mp4 (слайд 3), …)».
pub(crate) fn linked_text(linked: &[String]) -> String {
    format!("{} видео по ссылке на файлы, которых нет на этом компьютере ({})", linked.len(), linked.join(", "))
}
