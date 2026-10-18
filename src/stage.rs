//! Сцена — полноэкранное окно показа на сервере: фон, PDF, картинки, часы и таймер; Esc; сворачивание главного окна.

use super::*;

impl App {
    /// Сцена: полноэкранное окно сервера. Показывает фон, а поверх — PDF;
    /// PowerPoint выводит своё слайд-шоу поверх неё. Скрывается двойным Esc, когда на ней только фон.
    pub(crate) fn stage_window(&mut self, ctx: &egui::Context) {
        if !self.stage {
            self.stage_sent = None;
            self.bg_checked = false;
            return;
        }
        if !self.bg_checked {
            // Фон перечитывается при каждом появлении сцены и после смены; пока грузится — прежний.
            self.bg_checked = true;
            self.bg_gen += 1;
            let (gen, tx, c) = (self.bg_gen, self.out_tx.clone(), ctx.clone());
            let max_side = ctx.input(|i| i.max_texture_side) as u32;
            std::thread::spawn(move || {
                let _ = tx.send(Out::Background { gen, result: background::load(max_side) });
                c.request_repaint();
            });
        }
        // Анимация фона — только пока он виден (на прошлом кадре был на экране).
        let bg_next = if self.bg_shown { self.bg.as_mut().and_then(background::Background::tick) } else { None };
        // Под слайд-шоу PowerPoint и под играющим видео фона не видно — анимацию не двигаем.
        let bg_hidden = self.show.as_ref().is_some_and(|s| match &s.kind {
            ShowKind::Ppt(p) => matches!(p.phase, PptPhase::Lowered(_) | PptPhase::Shown),
            ShowKind::Video(v) => matches!(v.state, VideoState::Playing | VideoState::Paused),
            ShowKind::Pdf(_) | ShowKind::Image(_) => false,
        });
        let mut bg_drawn = false;
        let pdf = match &self.show {
            Some(s) => match &s.kind {
                ShowKind::Pdf(pdf) => Some((s.id, s.page, s.pages, pdf.clone())),
                ShowKind::Ppt(_) | ShowKind::Video(_) | ShowKind::Image(_) => None,
            },
            None => None,
        };
        // Сразу после открытия какое-то время виден только фон, даже если страница уже готова.
        let hold = self.show.as_ref().map_or(Duration::ZERO, |s| SWITCH_BG.saturating_sub(s.opened.elapsed()));
        if !hold.is_zero() {
            ctx.request_repaint_after(hold);
        }
        // Картинка, открытая как презентация (анимация — по своим кадрам).
        let (img_tex, img_next) = match self.show.as_mut().map(|s| &mut s.kind) {
            Some(ShowKind::Image(Some(b))) => {
                let next = if hold.is_zero() { b.tick() } else { None };
                (Some(b.texture().clone()), next)
            }
            _ => (None, None),
        };
        let view = self.local_state();
        let windowed = self.cfg.windowed;
        let top = !windowed && self.show.as_ref().is_none_or(Show::stage_on_top);
        let level = |top: bool| if top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
        // «Полный экран» делаем сами: окно без рамки ровно по монитору. Режим fullscreen
        // у дочернего окна eframe на Windows оставляет окно прежнего размера.
        let monitor = ctx.input(|i| i.viewport().monitor_size).unwrap_or(egui::vec2(1920.0, 1080.0));
        // На 1 точку выше монитора: OpenGL-окно ровно по размеру экрана Windows переводит в режим
        // «полноэкранного приложения», и кадры перестают выводиться на экран.
        let (pos, size) = if windowed { (egui::pos2(80.0, 80.0), egui::vec2(960.0, 540.0)) } else { (egui::Pos2::ZERO, monitor + egui::vec2(0.0, 1.0)) };
        let builder = egui::ViewportBuilder::default()
            .with_title(ppt::STAGE_TITLE)
            .with_icon(app_icon())
            .with_position(pos)
            .with_inner_size(size)
            .with_decorations(windowed)
            .with_window_level(level(top));
        let mut cmds: Vec<Cmd> = Vec::new();
        let mut wanted: Vec<PageKey> = Vec::new();
        let mut hide = false;
        let mut esc = false;
        let mut typed = (String::new(), false);
        // Без показа на сцене может быть экран часов или таймера.
        let scr = if self.show.is_none() { self.screen } else { Screen::Background };
        let timer = self.timer.info();
        let mut repaint: Option<Duration> = None;
        // Флаги builder'а у нового окна eframe на Windows не применяются — шлём команды явно.
        let sent = self.stage_sent;
        let bg = self.bg.as_ref().map(background::Background::texture);
        // Увеличение: видимая часть слайда; у PDF — ключ куска страницы, который нужен для экрана.
        let zoom = self.show.as_ref().map_or(1.0, |s| s.zoom);
        let zview = self.slide_view();
        let zoomed = zoom > 1.0;
        // Устаревшие увеличенные куски (другой масштаб или место) не держим.
        let zkey_view = render::view_key(zview);
        self.textures.retain(|k, _| k.view == FULL_VIEW || k.view == zkey_view);
        let mut stage_px = self.stage_px;
        let mut slide_aspect = self.slide_aspect;
        let ppt_page = self.show.as_ref().map_or(0, |s| s.page);
        let ppt_zoom = match self.show.as_ref().map(|s| &s.kind) {
            Some(ShowKind::Ppt(p)) => p.zoom_img.clone(),
            _ => None,
        };
        let show_cursor = self.show_cursor;
        let mut cursor_moved = self.cursor_moved;
        let (textures, last_tex, error, failed) = (&self.textures, &mut self.last_tex, &self.render_error, &self.failed);

        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("stage"), builder, |ctx, _| {
            if sent.is_none_or(|(_, fs)| fs == windowed) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(windowed));
                // В оконном режиме размер задаёт egui; на весь экран — подгоняем ниже под монитор показа.
                if windowed {
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                }
            }
            if sent.is_none_or(|(t, _)| t != top) {
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level(top)));
                // Сцена появилась или снова поднялась после PowerPoint — забирает клавиатуру.
                if top {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
            }
            if ctx.input(|i| i.viewport().close_requested()) {
                if view.active.is_some() {
                    cmds.push(Cmd::Close);
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                } else {
                    hide = true;
                }
            }
            cmds.extend(nav_keys(ctx, &view));
            esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
            typed = typed_digits(ctx);
            // Пока на экране сцена (фон или презентация), курсора не видно. С «Показать курсор» —
            // виден, пока мышь двигают, и ещё CURSOR_IDLE после (как в PowerPoint и проигрывателях).
            let mut cursor_on = false;
            if show_cursor {
                if ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_down()) {
                    cursor_moved = Some(Instant::now());
                }
                if let Some(left) = cursor_moved.and_then(|t| CURSOR_IDLE.checked_sub(t.elapsed())) {
                    cursor_on = true;
                    ctx.request_repaint_after(left);
                }
            }
            ctx.set_cursor_icon(if cursor_on { egui::CursorIcon::Default } else { egui::CursorIcon::None });
            egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ctx, |ui| {
                let rect = ui.max_rect();
                stage_px = (rect.width() * ctx.pixels_per_point(), rect.height() * ctx.pixels_per_point());
                let full_uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                // Увеличенный слайд: текстура и какая её часть на экране.
                let mut zoom_tex: Option<(egui::TextureHandle, egui::Rect)> = None;
                let mut page_tex = None;
                if let Some((id, page, pages, _)) = &pdf {
                    let (id, page, pages) = (*id, *page, *pages);
                    let ppp = ctx.pixels_per_point();
                    let max_side = ctx.input(|i| i.max_texture_side).min(16384) as f32;
                    let w = (rect.width() * ppp).round().clamp(16.0, max_side) as u32;
                    let h = (rect.height() * ppp).round().clamp(16.0, max_side) as u32;
                    let key = PageKey { id, page, w, h, thumb: false, view: FULL_VIEW };
                    let tex = textures.get(&key).cloned();
                    if let Some(t) = &tex {
                        *last_tex = tex.clone();
                        let s = t.size_vec2();
                        slide_aspect = s.x / s.y.max(1.0);
                    }
                    // Увеличен: сначала нужен кусок страницы для экрана; пока он рисуется —
                    // готовая страница, растянутая до того же куска (чуть мягче, но без пустого кадра).
                    let zkey = PageKey { view: zkey_view, ..key };
                    if zoomed {
                        if let Some(z) = textures.get(&zkey) {
                            zoom_tex = Some((z.clone(), full_uv));
                        } else if let Some(t) = &tex {
                            zoom_tex = Some((t.clone(), egui::Rect::from_min_max(egui::pos2(zview[0], zview[1]), egui::pos2(zview[2], zview[3]))));
                        }
                        if !textures.contains_key(&zkey) && !failed.contains(&zkey) {
                            wanted.push(zkey);
                        }
                    }
                    // Очередь рендера по важности: текущая, следующие (по ним листают чаще всего),
                    // затем предыдущая. Для очень больших экранов вперёд готовим меньше — бережём память.
                    let ahead = if (w as u64) * (h as u64) > 9_000_000 { PREFETCH_AHEAD - 1 } else { PREFETCH_AHEAD };
                    let order = std::iter::once(page).chain(page + 1..=page + ahead).chain(page.checked_sub(1));
                    wanted.extend(order.filter(|&p| p < pages).map(|p| PageKey { page: p, ..key }).filter(|k| !textures.contains_key(k) && !failed.contains(k)));
                    // Пока новая страница рендерится, оставляем на экране предыдущую — без мигания.
                    // До первой готовой страницы новой презентации виден фон.
                    page_tex = tex.or(last_tex.clone()).filter(|_| hold.is_zero());
                }
                // PowerPoint увеличен: крупная картинка слайда поверх слайд-шоу. Пока сцена после
                // увеличения опускается обратно — тот же слайд целиком (фон не мелькает).
                if let Some((pg, t)) = &ppt_zoom {
                    let s = t.size_vec2();
                    slide_aspect = s.x / s.y.max(1.0);
                    let uv = if zoomed && *pg == ppt_page { egui::Rect::from_min_max(egui::pos2(zview[0], zview[1]), egui::pos2(zview[2], zview[3])) } else { full_uv };
                    zoom_tex = Some((t.clone(), uv));
                    page_tex = Some(t.clone());
                }
                if page_tex.is_none() {
                    page_tex = img_tex.clone().filter(|_| hold.is_zero());
                    if let Some(t) = &page_tex {
                        let s = t.size_vec2();
                        slide_aspect = s.x / s.y.max(1.0);
                        // Картинку увеличиваем частью той же текстуры.
                        if zoomed {
                            zoom_tex = Some((t.clone(), egui::Rect::from_min_max(egui::pos2(zview[0], zview[1]), egui::pos2(zview[2], zview[3]))));
                        }
                    }
                }
                let uv = full_uv;
                // Пока не прошёл фон после открытия — без увеличения тоже ничего не рисуем.
                let page_tex = match (page_tex, zoom_tex.filter(|_| hold.is_zero())) {
                    (Some(_), Some(z)) => Some(z),
                    (p, _) => p.map(|t| (t, uv)),
                };
                match page_tex {
                    Some((t, uv)) => {
                        // Видимая часть текстуры вписывается в экран целиком.
                        let ts = t.size_vec2() * uv.size();
                        let k = (rect.width() / ts.x).min(rect.height() / ts.y);
                        let r = egui::Rect::from_center_size(rect.center(), ts * k);
                        ui.painter().image(t.id(), r, uv, Color32::WHITE);
                    }
                    None => match bg {
                        // Картинка фона заполняет экран целиком (лишнее обрезается по краям).
                        Some(t) => {
                            let ts = t.size_vec2();
                            let k = (rect.width() / ts.x).max(rect.height() / ts.y);
                            let r = egui::Rect::from_center_size(rect.center(), ts * k);
                            ui.painter().with_clip_rect(rect).image(t.id(), r, uv, Color32::WHITE);
                            bg_drawn = true;
                        }
                        None => {
                            ui.painter().rect_filled(rect, 0.0, STAGE_BG);
                        }
                    },
                }
                if let (Some(e), Some(_)) = (error, &pdf) {
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, e, egui::FontId::proportional(20.0), Color32::LIGHT_RED);
                }
                match scr {
                    Screen::Clock => repaint = Some(screen::draw_clock(ui.painter(), rect)),
                    Screen::Timer => repaint = screen::draw_timer(ui.painter(), rect, &timer),
                    Screen::Background => {}
                }
            });
        });
        self.cursor_moved = cursor_moved;
        if (stage_px, slide_aspect) != (self.stage_px, self.slide_aspect) {
            (self.stage_px, self.slide_aspect) = (stage_px, slide_aspect);
            self.dirty = true; // клиентам — новая рамка видимой части
        }
        // Часы и идущий таймер перерисовываем ровно при смене секунды.
        if let Some(d) = repaint {
            ctx.request_repaint_after(d);
        }
        if let Some(d) = img_next {
            ctx.request_repaint_after(d);
        }
        // Анимированный фон: следующий кадр — в свой срок; фон только что показался — сразу.
        let bg_drawn = bg_drawn && !bg_hidden;
        if bg_drawn {
            match bg_next {
                Some(d) => ctx.request_repaint_after(d),
                None if !self.bg_shown && self.bg.as_ref().is_some_and(background::Background::animated) => ctx.request_repaint(),
                None => {}
            }
        }
        self.bg_shown = bg_drawn;
        // Уровень окна сцены дублируем напрямую через Windows: при смене, а пока PowerPoint
        // готовит показ за сценой — каждый кадр (его окно не должно вылезти поверх фона).
        let ppt_loading = self.show.as_ref().is_some_and(|s| matches!(&s.kind, ShowKind::Ppt(p) if matches!(p.phase, PptPhase::Loading | PptPhase::Ready(_))));
        // Проверяем на каждой перерисовке: после показа PowerPoint сцена теряла «поверх всех»
        // и не получала его обратно — тогда подготовленное видео оказывалось над фоном.
        if !windowed {
            ppt::stage_topmost(top);
        }
        if self.show.as_ref().is_some_and(Show::video_on_screen) {
            video::stage_under_video(ppt::STAGE_TITLE);
        }
        if ppt_loading {
            ctx.request_repaint_after(Duration::from_millis(15));
        }
        self.stage_sent = Some((top, !windowed));
        // На весь экран: окно сцены ровно на монитор показа (+1 точка по высоте, см. выше), в физических
        // пикселях — так верно и при разном масштабе мониторов. Окно на месте — ничего не делается.
        if !windowed {
            if let Some(m) = display::target(&self.cfg.monitor) {
                display::fit_window(ppt::STAGE_TITLE, m.rect, 1);
            }
        }

        if let Some((id, page, pages, pdf)) = pdf {
            // Миниатюры для превью — после страниц для экрана.
            for p in [Some(page), Some(page + 1), page.checked_sub(1)].into_iter().flatten().filter(|&p| p < pages) {
                if !self.previews.contains_key(&(id, p)) && !self.thumb_failed.contains(&(id, p)) {
                    wanted.push(PageKey { id, page: p, w: THUMB_W, h: THUMB_H, thumb: true, view: FULL_VIEW });
                }
            }
            self.renderer.want(&pdf, wanted);
        }
        for cmd in cmds {
            self.apply(cmd);
        }
        if esc {
            self.esc_press();
        }
        self.goto_typed(&typed.0, typed.1, &self.local_state());
        if hide && self.show.is_none() {
            self.stage = false;
            self.screen = Screen::Background;
            self.dirty = true;
        }
    }

    /// Пока на мониторе главного окна сцена (фон, презентации, видео), главное окно свёрнуто —
    /// чтобы не проскакивало между открытиями и закрытиями окон показа. Сцену убрали (двойной Esc
    /// на фоне) — окно разворачивается. Сцена на другом мониторе или «в окне» — окно не трогаем.
    /// Действуем только при смене: если оператор сам развернул окно, оно так и останется.
    pub(crate) fn minimize_for_stage(&mut self, ctx: &egui::Context) {
        let stage = self.stage && !self.connected && !self.cfg.windowed;
        if stage && !self.main_minimized && !self.stage_up {
            let same = display::target(&self.cfg.monitor).is_some_and(|m| display::window_on(self.main_hwnd, m.rect));
            if same {
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Minimized(true));
                self.main_minimized = true;
            }
        } else if !stage && self.main_minimized {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
            self.main_minimized = false;
        }
        self.stage_up = stage;
    }

    /// Esc на сцене или в слайд-шоу PowerPoint. Одиночное нажатие ничего не делает.
    /// Двойное: закрывает презентацию, а если её нет — убирает сцену с фоном.
    pub(crate) fn esc_press(&mut self) {
        if !self.esc_at.is_some_and(|t| t.elapsed() <= DOUBLE_ESC) {
            self.esc_at = Some(Instant::now());
            return;
        }
        self.esc_at = None;
        if self.show.is_some() {
            self.apply(Cmd::Close);
        } else if self.screen != Screen::Background {
            // Часы или таймер — сначала возвращаем фон.
            self.screen = Screen::Background;
            self.dirty = true;
        } else {
            self.stage = false;
        }
    }
}
