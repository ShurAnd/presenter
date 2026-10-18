//! Интерфейс главного окна: панели, вкладки, список презентаций, окна, справка, клавиши и общие кнопки.

use super::*;

/// Справка: разделы и строки «что — как».
pub(crate) const HELP: &[(&str, &[(&str, &str)])] = &[
    ("С чего начать", &[
        ("Сервер", "Компьютер, который выводит показ: «Сеть», затем «Запустить сервер». На мониторе показа сразу появляется фон."),
        ("Клиент", "Компьютер оператора: «Сеть», затем выбрать сервер из списка (или ввести IP, порт, пароль) и нажать «Подключиться». Дальше всё, что вы делаете, выполняется на сервере."),
        ("Добавить файлы", "Кнопка «Добавить» или перетащить файлы в окно. PDF, PowerPoint (pptx, ppt, ppsx, pps), картинки (png, jpg, bmp, gif, webp — открываются как презентация из одного слайда, анимированная GIF движется), видео (mp4, mov, mkv, webm, avi, wmv и др.). У клиента файлы уходят на сервер."),
        ("Открыть", "Двойной щелчок по файлу, кнопка «Показать» или Enter по выбранному. Shift+1…9 — файл с этим номером в списке."),
        ("Порядок", "«Вверх» / «Вниз» двигают выбранный файл, «Удалить» убирает его, «Очистить» — весь список. Список сервера сохраняется и после перезапуска."),
    ]),
    ("Клавиши показа", &[
        ("Стрелки вправо и вниз, PageDown", "Вперёд (кликер). В PowerPoint — следующий шаг анимации."),
        ("Стрелки влево и вверх, PageUp, Backspace", "Назад."),
        ("Пробел", "Следующий шаг; у видео — старт и пауза; на экране таймера — старт и пауза таймера."),
        ("Home / End", "Первый / последний слайд. У видео Home — в начало, стрелки — перемотка на 10 с."),
        ("Цифры, затем Enter", "Перейти к слайду с этим номером (номер виден внизу окна, на экране показа — нет). Или поле «Слайд №» и «Перейти»."),
        ("Shift+0", "Закрыть показ."),
        ("Показать курсор", "Галочка внизу, рядом со «Справкой»: курсор мыши на экране показа виден, пока мышь двигают, и сам прячется через 3 с. В PowerPoint так можно щёлкнуть по видео в слайде. Работает и у клиента; после перезапуска — снова выключено."),
        ("Увеличение слайда", "Справа от превью: «+» / «−» — крупнее и мельче (до 400 %), стрелки — сдвинуть увеличенный слайд, кнопка в центре — снова весь слайд. На миниатюре «Сейчас» оранжевой рамкой — то, что видно на экране. С клавиатуры: «+» / «−», Ctrl+стрелки. Перелистнули — слайд снова целиком. PDF, картинки и PowerPoint; у PowerPoint увеличенный слайд — неподвижная картинка (анимация и видео в слайде не идут), первое увеличение слайда — через долю секунды."),
        ("Двойной Esc", "На экране показа: закрыть показ. На часах или таймере — вернуть фон. На фоне — убрать сцену. Одиночный Esc ничего не делает."),
        ("Кнопка ⌨", "Зелёная — клавиши в окне программы управляют показом. Серая — не управляют (на самом экране показа клавиши работают всегда)."),
    ]),
    ("Экран сервера: фон, часы, таймер", &[
        ("Фон", "Закрыть показ и оставить на экране показа фоновую картинку."),
        ("Часы", "Крупные часы с датой на экране показа. Открытие презентации возвращает фон."),
        ("Таймер", "Таймер на экране показа; внизу окна появляется строка управления."),
        ("Режим", "«Обратный отсчёт» или «Секундомер». Смена режима сбрасывает таймер."),
        ("мин : сек", "Длительность отсчёта: впишите минуты и секунды и нажмите Enter или Tab. «-» / «+» — на минуту меньше или больше, «+» работает и на идущем отсчёте."),
        ("Старт / Пауза, Сброс", "Запустить, остановить, вернуть на начало. Пробел — то же, что «Старт / Пауза»."),
        ("Цвета", "Последняя минута — жёлтым. Время вышло — «00:00», затем перерасход красным: «+00:12»."),
    ]),
    ("Превью слайдов", &[
        ("Предыдущий / Сейчас / Далее", "Над кнопками — предыдущий, текущий (в зелёной рамке) и следующий слайд открытой презентации. И на сервере, и у клиента."),
        ("PowerPoint", "Картинки слайдов делает PowerPoint во время показа, первые — через долю секунды после открытия. При следующем показе того же файла они уже готовы."),
        ("Видео", "Для видео превью нет."),
    ]),
    ("Плеер: музыка и фанфары", &[
        ("Вкладка «Плеер»", "Свой список музыки, отдельно от презентаций. Звук идёт из компьютера показа (сервера). Музыка играет и во время показа."),
        ("Добавить", "«Добавить» или перетащить файлы в окно: mp3, wav, flac, ogg, m4a, aac, wma и др. Музыка сама попадает в плеер."),
        ("Играть", "Двойной щелчок по треку или «Старт». «Пауза» / «Продолжить», «Пред.» / «След.», перемотка — полосой позиции."),
        ("Один трек / По списку", "«Один трек» — сыграть и остановиться. «По списку» — после трека следующий, после последнего — снова первый."),
        ("Повтор", "Кнопка рядом со «Стоп»: синяя — текущий трек играет по кругу, в любом режиме."),
        ("Вставкой", "Для фанфар: выбранный трек играет один раз, музыка на это время встаёт на паузу и потом продолжается с того же места. Пока вставка звучит, кнопка называется «Стоп вставки» — оборвать вставку и вернуть музыку."),
        ("Стоп", "Звук плавно затихает за 2 секунды. Нажать ещё раз — остановить сразу. Красная кнопка справа от вкладок — то же с любой вкладки."),
        ("Громкость", "Ползунок справа вверху; запоминается на сервере."),
    ]),
    ("PowerPoint и видео", &[
        ("PowerPoint", "Показывает сам PowerPoint, с анимацией и переходами. Пока он запускается, на экране показа фон. На последнем шаге «Вперёд» показ не закрывает."),
        ("Видео внутри слайда", "Если на слайде есть видео или звук, над превью появляется список «Видео на слайде»: у каждого свой номер, состояние, время и кнопки «Старт» / «Пауза» и «Стоп» — так запускается и видео, которое в PowerPoint включается щелчком мыши. На миниатюре «Сейчас» каждое видео обведено рамкой с тем же номером (зелёная — играет): видно, какое где; щелчок по рамке — старт или пауза. Играет несколько — «Стоп всех». Работает и у клиента."),
        ("Видео", "Готовится за фоном: «готово к запуску». «Старт» или пробел — сразу на экран показа. Доиграло — снова фон, а видео остаётся готовым: пробел — показать ещё раз с начала, без загрузки. Убрать видео — «Закрыть показ»."),
        ("Повтор видео", "Значок круговой стрелки справа в строке видео: синий — видео идёт по кругу, после конца сразу начинается заново (фон не появляется). По умолчанию выключен; переключать можно и во время показа. Запоминается в списке сервера."),
        ("Закрыть показ", "Кнопка «Закрыть показ», Shift+0 или двойной Esc на экране показа. На экране показа остаётся фон."),
    ]),
    ("Сеть, состояние сервера, резерв", &[
        ("Индикатор вверху", "Зелёный — норма, жёлтый — внимание, красный — проблема. Щелчок — показатели сервера: процессор, память, видеокарта, мониторы, связь."),
        ("Красная полоса", "Проблема на сервере или пропала связь. Клиент сам переподключается (до 15 попыток), можно сразу перейти на резерв."),
        ("Резервный сервер", "«Сеть», затем «Резервный сервер»: выбрать из списка или «Добавить…» — ввести название, IP, порт и пароль сервера, которого в списке ещё нет. Файлы можно сразу дублировать на него. «Переключиться на резерв» откроет там тот же файл на том же слайде."),
        ("Сохранённые серверы", "Сервер запоминается после успешного подключения. «Забыть» убирает его из списка."),
    ]),
    ("Настройки сервера", &[
        ("Монитор для показа", "«Сеть», затем «Монитор для показа»: основной или любой из подключённых (на компьютере с одним монитором — он же). Отключили выбранный — показ идёт на основном."),
        ("Автозапуск", "Галочка «Запускать сервер при входе в Windows»: сервер стартует сам и перезапускается при сбое или зависании."),
        ("Фон экрана показа", "Вкладка «Сеть» или правый щелчок по кнопке «Фон»: «Выбрать фон…» — картинка (PNG, JPG, BMP) или анимация (GIF, анимированные WebP и PNG). С клиента она уходит на сервер. Новый фон заменяет прежний; «Стандартный фон» — вернуть встроенный. Видео фоном не ставится — сделайте из ролика GIF любым конвертером (лучше до 1280 точек по ширине и 10–20 секунд)."),
        ("Брандмауэр", "При первом запуске сервера Windows спросит разрешение — разрешите, иначе клиенты с других компьютеров не подключатся."),
        ("Порт и пароль", "По умолчанию 14999 и default_password; должны совпадать на сервере и клиенте."),
    ]),
];

/// Цифры, набранные без Shift (Shift+цифра открывает презентацию по номеру), и нажат ли Enter.
pub(crate) fn typed_digits(ctx: &egui::Context) -> (String, bool) {
    ctx.input(|i| {
        let digits = if i.modifiers.shift || i.modifiers.ctrl || i.modifiers.alt {
            String::new()
        } else {
            i.events.iter().filter_map(|e| if let egui::Event::Text(t) = e { Some(t.as_str()) } else { None }).flat_map(str::chars).filter(char::is_ascii_digit).collect()
        };
        (digits, i.key_pressed(egui::Key::Enter))
    })
}

/// Клавиши управления показом — общие для главного окна и сцены. Esc обрабатывает только сцена (`esc_press`).
pub(crate) fn nav_keys(ctx: &egui::Context, view: &StateMsg) -> Vec<Cmd> {
    use egui::Key;
    let mut out = Vec::new();
    ctx.input(|i| {
        let active = view.active.is_some();
        if active {
            if i.key_pressed(Key::Space) {
                out.push(Cmd::Play);
            }
            if i.modifiers.ctrl {
                // Ctrl+стрелки — сдвиг увеличенного слайда.
                for (k, dx, dy) in [(Key::ArrowLeft, -1, 0), (Key::ArrowRight, 1, 0), (Key::ArrowUp, 0, -1), (Key::ArrowDown, 0, 1)] {
                    if i.key_pressed(k) {
                        out.push(Cmd::Pan { dx, dy });
                    }
                }
            } else {
                if [Key::ArrowRight, Key::ArrowDown, Key::PageDown].iter().any(|k| i.key_pressed(*k)) {
                    out.push(Cmd::Next);
                }
                if [Key::ArrowLeft, Key::ArrowUp, Key::PageUp, Key::Backspace].iter().any(|k| i.key_pressed(*k)) {
                    out.push(Cmd::Prev);
                }
            }
            if i.key_pressed(Key::Plus) || i.key_pressed(Key::Equals) {
                out.push(Cmd::Zoom { step: 1 });
            }
            if i.key_pressed(Key::Minus) {
                out.push(Cmd::Zoom { step: -1 });
            }
            if i.key_pressed(Key::Home) {
                out.push(Cmd::GoTo { page: 0 });
            }
            if i.key_pressed(Key::End) {
                out.push(Cmd::GoTo { page: usize::MAX });
            }
        } else if view.screen == Screen::Timer && i.key_pressed(Key::Space) {
            // На экране таймера пробел запускает и останавливает его.
            out.push(Cmd::TimerRun { run: !view.timer.running });
        }
        // Shift+1…9 — открыть презентацию с этим номером, Shift+0 — закрыть показ.
        if i.modifiers.shift && !i.modifiers.ctrl {
            let nums = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
            for (n, k) in nums.iter().enumerate() {
                if i.key_pressed(*k) {
                    if let Some(it) = view.items.get(n).filter(|it| it.ready) {
                        out.push(Cmd::Open { id: it.id });
                    }
                }
            }
            if i.key_pressed(Key::Num0) && active {
                out.push(Cmd::Close);
            }
        }
    });
    out
}

/// «01:23 / 03:45» (длительность, если известна).
pub(crate) fn audio_time(pos: u32, dur: u32) -> String {
    let t = |s: u32| format!("{:02}:{:02}", s / 60, s % 60);
    if dur > 0 { format!("{} / {}", t(pos), t(dur)) } else { t(pos) }
}

/// Что играет плеер — для строки вкладок (None — тишина).
pub(crate) fn now_playing(a: &AudioInfo) -> Option<(String, Color32)> {
    let name = |id: Option<u64>| id.and_then(|id| a.tracks.iter().find(|t| t.id == id)).map(|t| t.name.clone()).unwrap_or_default();
    if a.insert.is_some() && a.insert_state != AudioState::Stopped {
        return Some((format!("Вставка: {} {}", name(a.insert), audio_time(a.insert_pos, a.insert_dur)), INSERT));
    }
    match a.state {
        AudioState::Playing => Some((format!("Музыка: {} {}", name(a.current), audio_time(a.pos, a.dur)), GREEN)),
        AudioState::Paused => Some((format!("Музыка на паузе: {}", name(a.current)), BLUE)),
        AudioState::Stopping => Some(("Музыка затихает…".into(), GRAY)),
        AudioState::Stopped => None,
    }
}

/// Состояние видео для строки списка и нижней панели.
pub(crate) fn video_label(v: &VideoInfo) -> (String, Color32) {
    let t = |s: u32| format!("{:02}:{:02}", s / 60, s % 60);
    match v.state {
        VideoState::Loading => ("видео загружается…".into(), GRAY),
        VideoState::Armed => ("готово к запуску — пробел или «Старт»".into(), GREEN),
        VideoState::Playing => (format!("▶ {} / {}", t(v.pos), t(v.dur)), GREEN),
        VideoState::Paused => (format!("пауза {} / {}", t(v.pos), t(v.dur)), BLUE),
    }
}

pub(crate) fn level_color(level: health::Level) -> Color32 {
    match level {
        health::Level::Ok => GREEN,
        health::Level::Warn => Color32::from_rgb(0xe0, 0x9a, 0x00),
        health::Level::Bad => RED,
    }
}

/// Значок повтора (круговая стрелка) — рисуем сами: символов «🔁 ↻» в шрифте egui нет.
/// Включён — белая стрелка на синем, выключен — серая на светлом.
pub(crate) fn repeat_icon(p: &egui::Painter, r: egui::Rect, on: bool, hovered: bool) {
    let (bg, fg) = match (on, hovered) {
        (true, _) => (BLUE, Color32::WHITE),
        (false, true) => (Color32::from_gray(0xdd), Color32::from_gray(0x55)),
        (false, false) => (Color32::from_gray(0xee), Color32::from_gray(0x99)),
    };
    p.rect_filled(r, 4.0, bg);
    let c = r.center();
    let rad = 6.0;
    let stroke = egui::Stroke::new(1.8_f32, fg);
    // Дуга почти по кругу (разрыв справа сверху) и наконечник стрелки на её конце.
    let (a0, a1) = (-0.35_f32, 4.9_f32);
    let pts: Vec<egui::Pos2> = (0..=24).map(|k| {
        let a = a0 + (a1 - a0) * k as f32 / 24.0;
        c + egui::vec2(a.cos(), -a.sin()) * rad
    }).collect();
    p.add(egui::Shape::line(pts, stroke));
    let end = c + egui::vec2(a1.cos(), -a1.sin()) * rad;
    // Касательная в конце дуги — направление движения стрелки.
    let dir = egui::vec2(-a1.sin(), -a1.cos());
    let side = egui::vec2(-dir.y, dir.x);
    p.add(egui::Shape::convex_polygon(vec![end + dir * 3.5, end - dir * 1.5 + side * 3.5, end - dir * 1.5 - side * 3.5], fg, egui::Stroke::NONE));
}

pub(crate) fn cbutton(text: &str, color: Color32) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).color(Color32::WHITE).strong()).fill(color).corner_radius(5.0)
}

pub(crate) fn btn(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    ui.add(cbutton(text, color))
}

pub(crate) fn indicator(ui: &mut egui::Ui, color: Color32, text: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 6.0, color);
    ui.label(RichText::new(text).size(11.0).color(GRAY));
}

impl App {
    /// Красная полоса под верхней панелью, пока у сервера проблема.
    pub(crate) fn alarm_bar(&mut self, ctx: &egui::Context) {
        let problems: Vec<String> = if let (false, Some(reason)) = (self.connected, &self.lost) {
            // Общую причину «соединение потеряно» не повторяем, конкретную (например, «не отвечает») — показываем.
            let mut text = if reason.contains("потеряно") { "связь с сервером потеряна".to_string() } else { format!("связь с сервером потеряна: {reason}") };
            match self.reconnect {
                Some((n, _)) => text += &format!(" — переподключаюсь (попытка {} из {RECONNECT_TRIES})", n.max(1)),
                None => text += " — автоматически переподключиться не удалось",
            }
            vec![text]
        } else if self.connected {
            let (level, rows) = self.health_rows();
            if level != health::Level::Bad {
                return;
            }
            rows.iter().filter(|r| r.level == health::Level::Bad).map(|r| format!("{}: {}", r.name, r.value)).collect()
        } else {
            return;
        };
        let lost = !self.connected;
        let reserve = self.reserve_server();
        let frame = egui::Frame::new().fill(RED).inner_margin(egui::Margin::symmetric(10, 6));
        egui::TopBottomPanel::top("alarm").frame(frame).show(ctx, |ui| {
            // Две строки: полный текст проблемы, под ним кнопки — ничего не перекрывается.
            let text = format!("⚠ Проблема на сервере — {}", problems.join("; "));
            ui.label(RichText::new(&text).color(Color32::WHITE).strong());
            ui.horizontal(|ui| {
                if let Some(r) = &reserve {
                    let name = if r.name.trim().is_empty() { r.key() } else { r.name.trim().to_string() };
                    if btn(ui, &format!("Переключиться на резерв «{name}»"), GREEN).on_hover_text(r.label()).clicked() {
                        self.switch_to_reserve();
                    }
                }
                if lost {
                    if self.reconnect.is_none() && btn(ui, "Переподключиться", BLUE).clicked() {
                        self.lost = None;
                        let ctx = self.ctx.clone();
                        self.toggle_client(&ctx);
                    }
                    if btn(ui, "Скрыть", GRAY).clicked() {
                        self.lost = None;
                        self.reconnect = None;
                    }
                } else if btn(ui, "Показатели", GRAY).clicked() {
                    self.health_open = true;
                }
            });
        });
    }

    /// Окно с показателями сервера.
    pub(crate) fn health_window(&mut self, ctx: &egui::Context) {
        if !self.health_open {
            return;
        }
        if !(self.connected || self.server_online) {
            self.health_open = false;
            return;
        }
        if !self.connected {
            // Своё окно показателей на сервере обновляем вслед за замерами.
            ctx.request_repaint_after(Duration::from_secs(2));
        }
        let (level, rows) = self.health_rows();
        let reserve = self.reserve_server();
        let mut open = true;
        let mut switch = false;
        let title = if self.connected { "Состояние сервера" } else { "Состояние этого сервера" };
        egui::Window::new(title).open(&mut open).collapsible(false).resizable(false).default_width(520.0).show(ctx, |ui| {
            if rows.is_empty() {
                ui.label(if self.connected {
                    "Сервер не присылает показатели — вероятно, на нём старая версия Presentator (до 1.0.4)."
                } else {
                    "Показатели появятся через пару секунд."
                });
                return;
            }
            let summary = match level {
                health::Level::Ok => "Всё в норме",
                health::Level::Warn => "Есть показатели на грани — присмотритесь",
                health::Level::Bad => "Проблема — возможно, пора переключиться на резерв",
            };
            ui.label(RichText::new(summary).strong().size(15.0).color(level_color(level)));
            ui.add_space(6.0);
            egui::Grid::new("health").num_columns(2).spacing([16.0, 6.0]).striped(true).show(ui, |ui| {
                for r in &rows {
                    ui.label(r.name);
                    ui.label(RichText::new(&r.value).color(if r.level == health::Level::Ok { Color32::from_gray(0x33) } else { level_color(r.level) }).strong());
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            ui.label(RichText::new("Обновляется каждые 2 с. Жёлтый — на грани, красный — проблема.").size(11.5).color(GRAY));
            if self.connected {
                ui.add_space(4.0);
                match &reserve {
                    Some(r) => {
                        if btn(ui, &format!("Переключиться на резерв «{}»", r.label()), GREEN).clicked() {
                            switch = true;
                        }
                    }
                    None => {
                        ui.label(RichText::new("Резервный сервер не выбран — его можно указать на панели «Сеть».").size(11.5).color(GRAY));
                    }
                }
            }
        });
        self.health_open = open;
        if switch {
            self.switch_to_reserve();
        }
    }

    /// Окно ввода резервного сервера — те же поля, что у основного.
    pub(crate) fn reserve_window(&mut self, ctx: &egui::Context) {
        let Some(mut srv) = self.reserve_form.take() else { return };
        let (mut open, mut save, mut cancel) = (true, false, false);
        let port_ok = srv.port.trim().parse::<u16>().is_ok_and(|p| p > 0);
        let ip_ok = !srv.ip.trim().is_empty();
        let main_key = format!("{}:{}", self.cfg.ip.trim(), self.cfg.port.trim());
        egui::Window::new("Резервный сервер")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Grid::new("reserve_form").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                    ui.label("🏷 Название (необязательно):");
                    ui.add(egui::TextEdit::singleline(&mut srv.name).hint_text("Например: Зал 2").desired_width(220.0));
                    ui.end_row();
                    ui.label("📡 IP адрес сервера:");
                    ui.add(egui::TextEdit::singleline(&mut srv.ip).hint_text("Например: 192.168.1.101").desired_width(220.0));
                    ui.end_row();
                    ui.label("🔌 Порт:");
                    ui.add(egui::TextEdit::singleline(&mut srv.port).hint_text("14999").desired_width(90.0));
                    ui.end_row();
                    ui.label("🔑 Пароль доступа:");
                    ui.add(egui::TextEdit::singleline(&mut srv.password).password(true).desired_width(220.0));
                    ui.end_row();
                });
                if !port_ok && !srv.port.trim().is_empty() {
                    ui.label(RichText::new("Порт — число от 1 до 65535").color(RED));
                }
                if ip_ok && srv.key() == main_key {
                    ui.label(RichText::new("Это адрес основного сервера — резервным должен быть другой компьютер").color(RED));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let ok = ip_ok && port_ok;
                    if ui.add_enabled(ok, cbutton("Сохранить", GREEN)).on_hover_text("Запомнить сервер и выбрать его резервным").clicked() {
                        save = true;
                    }
                    if btn(ui, "Отмена", GRAY).clicked() {
                        cancel = true;
                    }
                });
            });
        if save {
            self.save_reserve(srv);
        } else if open && !cancel {
            self.reserve_form = Some(srv);
        }
    }

    pub(crate) fn top_bar(&mut self, ui: &mut egui::Ui, view: &StateMsg) {
        ui.horizontal(|ui| {
            if btn(ui, "➕ Добавить", BLUE).clicked() {
                if let Some(files) = rfd::FileDialog::new()
                    .add_filter("Презентации, картинки, видео и музыка", &[&["pdf", "pptx", "ppt", "ppsx", "pps"][..], &background::EXTS[..], &VIDEO_EXTS[..], &AUDIO_EXTS[..]].concat())
                    .add_filter("Картинки", &background::EXTS)
                    .add_filter("Музыка", &AUDIO_EXTS)
                    .pick_files()
                {
                    let ctx = ui.ctx().clone();
                    self.add_files(files, &ctx);
                }
            }
            // Переключатель «управление с клавиатуры»: зелёный — клавиши в этом окне управляют показом.
            let (fill, tip) = if self.cfg.keyboard {
                (GREEN, "Клавиатура включена: стрелки и PageUp/PageDown листают слайды, пробел — следующий шаг или старт/пауза видео, цифры + Enter — переход к слайду. Нажмите, чтобы выключить.")
            } else {
                (GRAY, "Клавиатура выключена: клавиши в этом окне показом не управляют. Нажмите, чтобы включить.")
            };
            let key_btn = egui::Button::new(RichText::new("⌨").size(14.0).color(Color32::WHITE)).fill(fill).corner_radius(5.0).min_size(egui::vec2(44.0, 0.0));
            if ui.add(key_btn).on_hover_text(tip).clicked() {
                self.cfg.keyboard = !self.cfg.keyboard;
                self.goto_buf.clear();
                self.save_cfg();
            }
            ui.add_space(4.0);
            if self.connected || self.server_online {
                // Состояние сервера: зелёный — норма, жёлтый — внимание, красный — проблема.
                let (level, rows) = self.health_rows();
                let who = if self.connected {
                    let name = self.cfg.name.trim();
                    if name.is_empty() { format!("Сервер {}", self.cfg.ip.trim()) } else { format!("Сервер «{name}»") }
                } else {
                    format!("Этот сервер ({} кл.)", self.clients)
                };
                let state = match (level, rows.is_empty()) {
                    (_, true) => "нет данных",
                    (health::Level::Ok, _) => "норма",
                    (health::Level::Warn, _) => "внимание",
                    (health::Level::Bad, _) => "проблема",
                };
                let color = if rows.is_empty() { OFF } else { level_color(level) };
                // Кружок рисуем сами: в шрифте egui нет символа «●».
                let (dot, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                ui.painter().circle_filled(dot.center(), 6.0, color);
                let resp = ui.add(egui::Button::new(RichText::new(format!("{who}: {state}")).color(color).strong()).frame(false));
                if resp.on_hover_text("Показатели сервера: процессор, память, видеокарта, мониторы, связь").clicked() {
                    self.health_open = !self.health_open;
                }
            } else {
                indicator(ui, OFF, "Сервер: офлайн");
                indicator(ui, OFF, "Клиент: не подключен");
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // На вкладке «Плеер» кнопки работают со списком плеера.
                let player = self.tab == Tab::Player;
                let tip = if player { "Очистить список плеера" } else { "Очистить список презентаций" };
                if btn(ui, "🗑 Очистить", RED).on_hover_text(tip).clicked() {
                    let (n, what) = if player { (view.audio.tracks.len(), "музыки из плеера") } else { (view.items.len(), "презентаций") };
                    if n > 0 {
                        let cmd = if player { Cmd::AudioClear } else { Cmd::Clear };
                        self.confirm = Some((format!("Очистить список: удалить все файлы {what} ({n} шт.)? Файлы удалятся и на сервере."), "Очистить", cmd));
                    }
                }
                let sel = if player {
                    self.audio_selected.filter(|id| view.audio.tracks.iter().any(|t| t.id == *id))
                } else {
                    self.selected.filter(|id| view.items.iter().any(|i| i.id == *id))
                };
                let mv = |id, up| if player { Cmd::AudioMove { id, up } } else { Cmd::Move { id, up } };
                if ui.add_enabled(sel.is_some(), cbutton("⬇ Вниз", GRAY)).clicked() {
                    self.act(mv(sel.unwrap(), false));
                }
                if ui.add_enabled(sel.is_some(), cbutton("⬆ Вверх", GRAY)).clicked() {
                    self.act(mv(sel.unwrap(), true));
                }
            });
        });
    }

    pub(crate) fn bottom_bar(&mut self, ui: &mut egui::Ui, view: &StateMsg) {
        let active = view.active.is_some();
        ui.horizontal(|ui| {
            if ui.add_enabled(active, cbutton("◀ Назад", BLUE)).clicked() {
                self.act(Cmd::Prev);
            }
            if ui.add_enabled(active, cbutton("Вперёд ▶", BLUE)).clicked() {
                self.act(Cmd::Next);
            }
            let sel = self.selected.and_then(|id| view.items.iter().find(|i| i.id == id)).filter(|i| i.ready);
            if ui.add_enabled(sel.is_some(), cbutton("▶ Показать", GREEN)).clicked() {
                self.act(Cmd::Open { id: sel.unwrap().id });
            }
            if let Some(v) = &view.video {
                let (t, c) = match v.state {
                    VideoState::Loading => ("Подготовка…", GRAY),
                    VideoState::Armed => ("▶ Старт", GREEN),
                    VideoState::Playing => ("⏸ Пауза", BLUE),
                    VideoState::Paused => ("▶ Продолжить", GREEN),
                };
                if ui.add_enabled(v.state != VideoState::Loading, cbutton(t, c)).on_hover_text("Пробел").clicked() {
                    self.act(Cmd::Play);
                }
            }
            if ui.add_enabled(active, cbutton("■ Закрыть показ", RED)).clicked() {
                self.act(Cmd::Close);
            }
            if ui.add_enabled(self.selected.is_some(), cbutton("Удалить", GRAY)).clicked() {
                if let Some(id) = self.selected {
                    let name = view.items.iter().find(|i| i.id == id).map(|i| i.name.clone()).unwrap_or_default();
                    self.confirm = Some((format!("Удалить «{name}» из списка? Файл удалится и на сервере."), "Удалить", Cmd::Remove { id }));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(v) = &view.video {
                    let (t, c) = video_label(v);
                    ui.label(RichText::new(t).strong().size(15.0).color(c));
                } else if active {
                    if !self.goto_buf.is_empty() {
                        // Номер, набранный с клавиатуры, — подсказка оператору (на экране показа его не видно).
                        ui.label(RichText::new(format!("К слайду {} — Enter", self.goto_buf)).strong().size(15.0).color(GREEN));
                    } else {
                        let zoom = if view.zoom > 1.0 { format!(" · {:.0} %", view.zoom * 100.0) } else { String::new() };
                        ui.label(RichText::new(format!("Слайд {} из {}{zoom}", view.page + 1, view.pages)).strong().size(15.0));
                    }
                } else {
                    ui.label(RichText::new("Показ не запущен").color(GRAY));
                }
            });
        });
        let text = self.status.clone();
        // Справа экран сервера (фон, часы, таймер) и переход к слайду по номеру, слева — «Справка»
        // и строка состояния в оставшееся место (обрезается, а не наезжает на кнопки).
        ui.horizontal(|ui| ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Справа налево. Текущий экран сервера (когда показа нет) подсвечен.
            let screens = [
                (Screen::Timer, Cmd::Timer, "Таймер", "Закрыть показ и вывести на экран сервера таймер"),
                (Screen::Clock, Cmd::Clock, "Часы", "Закрыть показ и вывести на экран сервера часы"),
                (Screen::Background, Cmd::Background, "Фон", "Закрыть показ и вывести на экран сервера фон"),
            ];
            for (scr, cmd, label, tip) in screens {
                let on = !active && view.screen == scr;
                let tip = if scr == Screen::Background { format!("{tip}. Правый щелчок — выбрать картинку фона") } else { tip.to_string() };
                let r = btn(ui, label, if on { BLUE } else { GRAY }).on_hover_text(tip);
                if r.clicked() {
                    self.act(cmd);
                }
                if scr == Screen::Background {
                    r.context_menu(|ui| self.background_menu(ui, &view.background));
                }
            }
            if active && view.video.is_none() {
                ui.add_space(10.0);
                let go = btn(ui, "Перейти", BLUE).on_hover_text("Перейти к слайду с этим номером");
                let field = ui.add(egui::TextEdit::singleline(&mut self.goto_field).hint_text("№").desired_width(44.0));
                ui.label("Слайд №");
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                self.goto_field.retain(|c| c.is_ascii_digit());
                if go.clicked() || enter {
                    let n = std::mem::take(&mut self.goto_field);
                    self.goto_number(&n, view);
                }
            }
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if btn(ui, "Справка", GRAY).on_hover_text("Что где нажимать: клавиши, экраны сервера, превью, сеть").clicked() {
                    self.help_open = !self.help_open;
                }
                let mut cursor = view.cursor;
                if ui
                    .checkbox(&mut cursor, "Показать курсор")
                    .on_hover_text("Курсор мыши на экране показа виден, пока мышь двигают (через 3 с покоя прячется). Например, чтобы щёлкнуть по видео в слайде PowerPoint. После перезапуска программы — снова выключено")
                    .changed()
                {
                    self.act(Cmd::Cursor { on: cursor });
                }
                ui.add(egui::Label::new(RichText::new(&text).size(11.5).color(Color32::from_gray(0x66))).truncate());
            });
        }));
        if !active && view.screen == Screen::Timer {
            self.timer_bar(ui, view);
        }
    }

    /// Управление таймером — пока на экране сервера таймер.
    pub(crate) fn timer_bar(&mut self, ui: &mut egui::Ui, view: &StateMsg) {
        let t = view.timer.clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Таймер:").strong());
            if ui.selectable_label(t.countdown, "Обратный отсчёт").clicked() && !t.countdown {
                self.act(Cmd::TimerSet { countdown: true, secs: t.total });
            }
            if ui.selectable_label(!t.countdown, "Секундомер").clicked() && t.countdown {
                self.act(Cmd::TimerSet { countdown: false, secs: t.total });
            }
            if t.countdown {
                ui.add_space(6.0);
                if ui.add_enabled(t.total > 60, egui::Button::new(" - ")).on_hover_text("На минуту меньше").clicked() {
                    self.act(Cmd::TimerSet { countdown: true, secs: t.total - 60 });
                }
                // Поля показывают значение сервера, пока их не редактируют. Длительность задаётся,
                // когда поле теряет фокус (Enter, Tab или щелчок мимо).
                let field = |ui: &mut egui::Ui, text: &mut String, salt: &str, value: String, hint: &str| {
                    let id = ui.make_persistent_id(salt);
                    if !ui.memory(|m| m.has_focus(id)) {
                        *text = value;
                    }
                    let r = ui.add(egui::TextEdit::singleline(text).id(id).desired_width(30.0).horizontal_align(egui::Align::Center)).on_hover_text(hint);
                    text.retain(|c| c.is_ascii_digit());
                    text.truncate(4);
                    r.lost_focus()
                };
                let done_min = field(ui, &mut self.timer_min, "timer_min", (t.total / 60).to_string(), "Минуты");
                ui.label(":");
                let done_sec = field(ui, &mut self.timer_sec, "timer_sec", format!("{:02}", t.total % 60), "Секунды");
                if done_min || done_sec {
                    let m: u32 = self.timer_min.parse().unwrap_or(0);
                    let sec: u32 = self.timer_sec.parse().unwrap_or(0);
                    let secs = m * 60 + sec;
                    if secs >= 1 && secs != t.total {
                        self.act(Cmd::TimerSet { countdown: true, secs });
                    }
                }
                if ui.button(" + ").on_hover_text("На минуту больше (и для идущего отсчёта)").clicked() {
                    self.act(Cmd::TimerSet { countdown: true, secs: t.total + 60 });
                }
                ui.label("мин : сек");
            }
            ui.add_space(6.0);
            let (label, color) = if t.running { ("⏸ Пауза", BLUE) } else { ("▶ Старт", GREEN) };
            if btn(ui, label, color).on_hover_text("Пробел").clicked() {
                self.act(Cmd::TimerRun { run: !t.running });
            }
            if btn(ui, "Сброс", GRAY).clicked() {
                self.act(Cmd::TimerReset);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (text, phase) = screen::timer_text(&t, self.timer_extra());
                let color = if phase == screen::Phase::Normal { Color32::from_gray(0x22) } else { screen::phase_color(phase).gamma_multiply(0.85) };
                ui.label(RichText::new(text).strong().size(18.0).color(color));
            });
        });
    }

    /// Сколько мс назад снято состояние таймера: у клиента — с прихода состояния сервера.
    pub(crate) fn timer_extra(&self) -> u64 {
        if self.connected { self.remote_at.elapsed().as_millis() as u64 } else { 0 }
    }

    /// Кнопки «Выбрать…» и «Стандартный» (во вкладке «Сеть» и в меню кнопки «Фон»).
    pub(crate) fn background_menu(&mut self, ui: &mut egui::Ui, current: &str) {
        if ui.button("Выбрать фон…").clicked() {
            ui.close_menu();
            self.pick_background();
        }
        if ui.add_enabled(!current.is_empty(), egui::Button::new("Стандартный фон")).clicked() {
            ui.close_menu();
            self.act(Cmd::BackgroundReset);
        }
    }

    /// Окно справки: что где нажимать.
    pub(crate) fn help_window(&mut self, ctx: &egui::Context) {
        if !self.help_open {
            return;
        }
        let mut open = true;
        egui::Window::new("Справка").open(&mut open).collapsible(false).default_width(600.0).default_height(470.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for (n, (title, rows)) in HELP.iter().enumerate() {
                    egui::CollapsingHeader::new(RichText::new(*title).strong().size(14.0)).default_open(n == 0).show(ui, |ui| {
                        egui::Grid::new(("help", n)).num_columns(2).spacing([14.0, 5.0]).striped(true).show(ui, |ui| {
                            for (key, text) in rows.iter() {
                                ui.label(RichText::new(*key).strong());
                                ui.add(egui::Label::new(*text).wrap());
                                ui.end_row();
                            }
                        });
                    });
                }
            });
        });
        self.help_open = open;
    }

    /// Вкладки «Презентации / Плеер / Сеть»; справа — что играет плеер (видно с любой вкладки).
    pub(crate) fn tab_row(&mut self, ui: &mut egui::Ui, view: &StateMsg) {
        ui.horizontal(|ui| {
            for (tab, label) in [(Tab::List, "Презентации"), (Tab::Player, "Плеер"), (Tab::Net, "Сеть"), (Tab::Settings, "Настройки")] {
                let on = self.tab == tab;
                let text = RichText::new(label).size(14.0).strong().color(if on { Color32::WHITE } else { Color32::from_gray(0x44) });
                let b = egui::Button::new(text).fill(if on { BLUE } else { Color32::from_gray(0xe4) }).corner_radius(5.0).min_size(egui::vec2(96.0, 0.0));
                if ui.add(b).clicked() && !on {
                    if tab == Tab::Net {
                        self.merge_servers_from_disk();
                    }
                    self.tab = tab;
                }
            }
            if let Some((text, color)) = now_playing(&view.audio) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(cbutton("■", RED)).on_hover_text("Остановить музыку (плавно)").clicked() {
                        self.act(Cmd::AudioStop);
                    }
                    ui.add(egui::Label::new(RichText::new(text).color(color).strong()).truncate());
                });
            }
        });
    }

    pub(crate) fn list(&mut self, ui: &mut egui::Ui, view: &StateMsg) {
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0_f32, Color32::from_gray(0xcc)))
            .corner_radius(5.0)
            .inner_margin(6.0)
            .fill(Color32::WHITE)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                if view.items.is_empty() {
                    ui.centered_and_justified(|ui| {
                        let t = if self.connected {
                            "Список сервера пуст.\nНажмите «Добавить» или перетащите сюда PDF / PPTX — файлы уйдут на сервер."
                        } else {
                            "Список пуст.\nНажмите «Добавить» или перетащите сюда файлы PDF / PPTX."
                        };
                        ui.label(RichText::new(t).color(GRAY).size(14.0));
                    });
                    return;
                }
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for (n, it) in view.items.iter().enumerate() {
                        let is_active = view.active == Some(it.id);
                        let is_sel = self.selected == Some(it.id);
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), egui::Sense::click());
                        let bg = if is_active {
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
                        let mark = if is_active { "▶ " } else { "" };
                        p.text(
                            rect.left_center() + egui::vec2(8.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            format!("{mark}{}. {}", n + 1, it.name),
                            egui::FontId::proportional(14.0),
                            Color32::from_gray(0x22),
                        );
                        let (info, col) = if !it.status.is_empty() {
                            (it.status.clone(), if it.ready || it.status.starts_with("обработка") { GRAY } else { RED })
                        } else if let Some(v) = view.video.as_ref().filter(|_| is_active) {
                            let (t, c) = video_label(v);
                            (format!("{} · {t}", it.kind), c)
                        } else if is_active {
                            (format!("{} · слайд {}/{}", it.kind, view.page + 1, view.pages), GREEN)
                        } else if it.pages == 0 || it.kind == KIND_IMAGE {
                            (it.kind.clone(), GRAY)
                        } else {
                            (format!("{} · {} стр.", it.kind, it.pages), GRAY)
                        };
                        // Видео по ссылке на отсутствующие файлы — предупреждение прямо в строке.
                        let (info, col) = if it.linked.is_empty() || !it.status.is_empty() {
                            (info, col)
                        } else {
                            (format!("{info} · видео по ссылке: {}", it.linked.len()), INSERT)
                        };
                        let info: String = if info.chars().count() > 60 { info.chars().take(60).chain("…".chars()).collect() } else { info };
                        // У видео справа — значок повтора; текст состояния сдвигается левее.
                        let is_video = it.kind == "ВИДЕО";
                        let right = if is_video { 44.0 } else { 8.0 };
                        p.text(rect.right_center() - egui::vec2(right, 0.0), egui::Align2::RIGHT_CENTER, info, egui::FontId::proportional(12.0), col);
                        if is_video {
                            let r = egui::Rect::from_center_size(rect.right_center() - egui::vec2(22.0, 0.0), egui::vec2(28.0, 22.0));
                            let icon = ui.interact(r, ui.id().with(("repeat", it.id)), egui::Sense::click());
                            repeat_icon(ui.painter(), r, it.repeat, icon.hovered());
                            let tip = if it.repeat { "Повтор включён: видео по окончании начнётся заново. Щёлкните, чтобы выключить" } else { "Повтор выключен: по окончании — фон. Щёлкните, чтобы видео шло по кругу" };
                            if icon.on_hover_text(tip).clicked() {
                                self.act(Cmd::VideoRepeat { id: it.id, on: !it.repeat });
                            }
                        }
                        let resp = if !it.status.is_empty() {
                            resp.on_hover_text(&it.status)
                        } else if !it.linked.is_empty() {
                            resp.on_hover_text(format!(
                                "Видео вставлены в презентацию ссылкой на файлы, которых нет на сервере, — на экране их не будет:\n{}\n\nВставьте видео в саму презентацию (в PowerPoint: Вставка → Видео → «Это устройство», без «Связать с файлом») и загрузите её заново.",
                                it.linked.join("\n")
                            ))
                        } else {
                            resp
                        };
                        if resp.clicked() {
                            self.selected = Some(it.id);
                        }
                        if resp.double_clicked() && it.ready {
                            self.selected = Some(it.id);
                            self.act(Cmd::Open { id: it.id });
                        }
                    }
                });
            });
    }

    pub(crate) fn keys(&mut self, ctx: &egui::Context, view: &StateMsg) {
        if self.confirm.is_some() {
            return;
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        let (digits, enter) = typed_digits(ctx);
        if self.cfg.keyboard {
            // Кнопка, нажатая мышью, остаётся в фокусе, и пробел или Enter «нажали» бы её снова, —
            // в режиме управления с клавиатуры они должны уходить в показ.
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            for cmd in nav_keys(ctx, view) {
                self.act(cmd);
            }
        }
        let digits = if self.cfg.keyboard { digits } else { String::new() };
        if !self.goto_typed(&digits, enter, view) && enter {
            if let Some(id) = self.selected {
                self.act(Cmd::Open { id });
            }
        }
    }

    /// Номер слайда с клавиатуры, как в PowerPoint: цифры, затем Enter. Работает в главном окне
    /// и на экране показа. Возвращает true, если Enter ушёл на переход к слайду.
    pub(crate) fn goto_typed(&mut self, digits: &str, enter: bool, view: &StateMsg) -> bool {
        if view.active.is_none() || view.video.is_some() || self.goto_at.is_some_and(|t| t.elapsed() > GOTO_TIMEOUT) {
            self.goto_buf.clear();
            self.goto_at = None;
        }
        if view.active.is_some() && view.video.is_none() && !digits.is_empty() {
            self.goto_buf.push_str(digits);
            self.goto_buf.truncate(4);
            self.goto_at = Some(Instant::now());
        }
        if !enter || self.goto_buf.is_empty() {
            return false;
        }
        let n = std::mem::take(&mut self.goto_buf);
        self.goto_number(&n, view);
        true
    }

    /// Перейти к слайду с номером `n` (с единицы).
    pub(crate) fn goto_number(&mut self, n: &str, view: &StateMsg) {
        match n.parse::<usize>() {
            Ok(n) if n >= 1 && n <= view.pages => self.act(Cmd::GoTo { page: n - 1 }),
            _ if n.is_empty() => {}
            _ => self.status = format!("Нет слайда {n}: в презентации {} слайдов", view.pages),
        }
    }

    /// Окно подтверждения («Удалить «…»?», «Очистить список?»): «Да» выполняет команду.
    pub(crate) fn confirm_window(&mut self, ctx: &egui::Context) {
        let Some((text, yes, _)) = &self.confirm else { return };
        let (text, yes) = (text.clone(), *yes);
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("confirm")).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.label(RichText::new("Подтвердите").strong().size(16.0));
            ui.add_space(6.0);
            ui.label(RichText::new(text).size(14.0));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if btn(ui, yes, RED).clicked() {
                    answer = Some(true);
                }
                if btn(ui, "Отмена", GRAY).clicked() {
                    answer = Some(false);
                }
            });
        });
        // Esc или щелчок мимо окна — отмена.
        if answer.is_none() && modal.should_close() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                if let Some((_, _, cmd)) = self.confirm.take() {
                    if let Cmd::Remove { .. } = cmd {
                        self.selected = None;
                    }
                    if let Cmd::AudioRemove { .. } = cmd {
                        self.audio_selected = None;
                    }
                    self.act(cmd);
                }
            }
            Some(false) => self.confirm = None,
            None => {}
        }
    }

    /// Жёлтая полоса у клиента: версия программы на сервере другая — часть кнопок может не работать.
    pub(crate) fn version_bar(&mut self, ctx: &egui::Context) {
        let ours = env!("CARGO_PKG_VERSION");
        if !self.connected || !self.got_state || self.version_hidden || self.remote.version == ours {
            return;
        }
        let theirs = self.remote.version.clone();
        let text = match version_cmp(&theirs, ours) {
            std::cmp::Ordering::Less => format!(
                "На сервере {} программы, у вас {ours} — новые кнопки на нём не сработают. Обновите программу на сервере.",
                if theirs.is_empty() { "старая версия".to_string() } else { format!("версия {theirs}") }
            ),
            _ => format!("На сервере версия {theirs}, у вас {ours} — обновите программу на этом компьютере."),
        };
        let frame = egui::Frame::new().fill(Color32::from_rgb(0xff, 0xe3, 0x8f)).inner_margin(egui::Margin::symmetric(10, 6));
        egui::TopBottomPanel::top("version").frame(frame).show(ctx, |ui| {
            // Кнопка справа, текст — в оставшееся место с переносом строк.
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if btn(ui, "Скрыть", GRAY).clicked() {
                        self.version_hidden = true;
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(format!("⚠ {text}")).color(Color32::from_gray(0x33)).strong()).wrap());
                    });
                });
            });
        });
    }

    /// Журнал: строка состояния и сетевой статус (только перемены), уровень состояния сервера,
    /// запрет сна, пока работает сервер или на экране сцена.
    pub(crate) fn journal_tick(&mut self) {
        // Ход передачи файла («Отправка «…»: 37%») в журнал не пишем — только итог.
        if self.status != self.logged_status {
            if !self.status.is_empty() && !self.status.starts_with("Отправка «") {
                jlog!("Сообщение: {}", self.status);
            }
            self.logged_status = self.status.clone();
        }
        if self.net_status != self.logged_net {
            if !self.net_status.is_empty() {
                jlog!("Сеть: {}", self.net_status);
            }
            self.logged_net = self.net_status.clone();
        }
        if self.connected || self.server_online {
            let (level, rows) = self.health_rows();
            let level = (!rows.is_empty()).then_some(level);
            if level != self.logged_level {
                if let Some(l) = level {
                    let bad: Vec<String> = rows.iter().filter(|r| r.level != health::Level::Ok).map(|r| format!("{}: {}", r.name, r.value)).collect();
                    jlog!("Состояние сервера: {:?}{}", l, if bad.is_empty() { String::new() } else { format!(" — {}", bad.join("; ")) });
                }
                self.logged_level = level;
            }
        }
        let awake = self.server_online || self.stage;
        if awake != self.awake {
            keep_awake(awake);
            jlog!("{}", if awake { "Экран и сон: не гасить (идёт показ или работает сервер)" } else { "Экран и сон: как в настройках Windows" });
            self.awake = awake;
        }
    }
}

/// Сравнение версий «2.2.6» по числам (пустая — самая старая).
pub(crate) fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let p = |v: &str| v.split('.').map(|x| x.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>();
    p(a).cmp(&p(b))
}
