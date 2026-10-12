#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod audio;
mod background;
mod display;
mod health;
mod net;
mod player;
mod ppt;
mod rec_panel;
mod record;
mod render;
mod screen;
#[cfg(windows)]
mod mpv;
mod video;
mod watchdog;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use net::{AudioInfo, AudioState, CliEvent, CliReq, ClientHandle, Cmd, ItemInfo, Screen, ServerHandle, SrvEvent, StateMsg, VideoInfo, VideoState};
use render::{Out, PageKey, Renderer, AUDIO_EXTS, THUMB_H, THUMB_W, VIDEO_EXTS};
use serde::{Deserialize, Serialize};

const BLUE: Color32 = Color32::from_rgb(0x5b, 0xc0, 0xde);
const GREEN: Color32 = Color32::from_rgb(0x28, 0xa7, 0x45);
const RED: Color32 = Color32::from_rgb(0xd9, 0x53, 0x4f);
const GRAY: Color32 = Color32::from_rgb(0x88, 0x88, 0x88);
const OFF: Color32 = Color32::from_rgb(0x6c, 0x75, 0x7d);
/// Вставка (фанфары) в плеере.
const INSERT: Color32 = Color32::from_rgb(0xe0, 0x8a, 0x00);
/// Фон сцены, если картинки фона нет.
const STAGE_BG: Color32 = Color32::from_rgb(0x1f, 0x6f, 0xc8);
/// Сколько ждать после «ready» от PowerPoint, прежде чем убрать сцену: слайд-шоу должно успеть отрисоваться.
const PPT_SETTLE: Duration = Duration::from_millis(300);
/// PowerPoint не открыл показ за это время — считаем, что он завис, и перезапускаем мост.
const PPT_TIMEOUT: Duration = Duration::from_secs(25);
/// Пауза между опусканием сцены и выводом слайд-шоу на передний план.
const PPT_LOWER: Duration = Duration::from_millis(120);
/// Сколько минимум виден фон при открытии презентации (и при переключении между ними).
const SWITCH_BG: Duration = Duration::from_millis(500);
/// Сколько страниц PDF вперёд готовить заранее (на очень больших экранах — на одну меньше).
const PREFETCH_AHEAD: usize = 3;
/// Набранный с клавиатуры номер слайда забывается, если Enter не нажали за это время.
const GOTO_TIMEOUT: Duration = Duration::from_secs(4);
/// Два нажатия Esc с интервалом не больше этого скрывают сцену.
const DOUBLE_ESC: Duration = Duration::from_millis(600);

fn main() -> eframe::Result {
    // `--watchdog`: окна нет — сторож запускает сервер (остальные ключи) и перезапускает его при сбое.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--watchdog") {
        watchdog::run(args.into_iter().filter(|a| a != "--watchdog").collect());
        return Ok(());
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(format!("Presentator v{}", env!("CARGO_PKG_VERSION")))
        .with_inner_size([750.0, 620.0])
        .with_min_inner_size([640.0, 420.0])
        .with_icon(app_icon());
    // По центру рабочей области: иначе Windows при каскадной раскладке мог открыть окно так низко,
    // что нижняя панель с кнопками уходила за край экрана (1080p при масштабе 150 %).
    let geometry = initial_geometry();
    if let Some((pos, size)) = geometry {
        viewport = viewport.with_position(pos).with_inner_size(size);
    }
    let options = eframe::NativeOptions { viewport, centered: geometry.is_none(), ..Default::default() };
    eframe::run_native("presentator", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Config {
    ip: String,
    port: String,
    password: String,
    /// Название сервера для списка (необязательно), например «Зал 1».
    name: String,
    windowed: bool,
    /// Клавиши в окне программы (стрелки, PageUp/PageDown, пробел, цифры + Enter) управляют показом.
    keyboard: bool,
    /// Монитор для показа: системное имя `\\.\DISPLAY2`; пусто — основной.
    monitor: String,
    /// Серверы, к которым подключались, — последний использованный первым.
    #[serde(deserialize_with = "null_as_empty")]
    servers: Vec<SavedServer>,
    /// Резервный сервер («адрес:порт» из списка) — на него можно переключиться одной кнопкой.
    reserve: String,
    /// Загружаемые файлы дублировать на резервный сервер.
    mirror: bool,
    /// Плеер (на сервере): играть по списку, по кругу, громкость 0–100.
    audio_list: bool,
    audio_repeat: bool,
    audio_volume: u8,
    /// Выбранный фон сцены — имя исходного файла (для показа); пусто — стандартный.
    background: String,
    /// Запись (на этом компьютере): видео- и звуковое устройство («-» — без него, пусто — подобрать),
    /// формат видео (подпись, пусто — лучший) и папка (пусто — «Записи» в рабочей папке).
    rec_video: String,
    rec_audio: String,
    rec_mode: String,
    rec_dir: String,
}

/// `"servers": null` (файл правили руками или другой программой) читаем как пустой список,
/// иначе из-за одного поля пропали бы все настройки.
fn null_as_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<SavedServer>, D::Error> {
    Ok(Option::<Vec<SavedServer>>::deserialize(d)?.unwrap_or_default())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ip: "127.0.0.1".into(),
            port: "14999".into(),
            password: "default_password".into(),
            name: String::new(),
            windowed: false,
            keyboard: true,
            monitor: String::new(),
            servers: Vec::new(),
            reserve: String::new(),
            mirror: true,
            audio_list: true,
            audio_repeat: false,
            audio_volume: 100,
            background: String::new(),
            rec_video: String::new(),
            rec_audio: String::new(),
            rec_mode: String::new(),
            rec_dir: String::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(default)]
struct SavedServer {
    name: String,
    ip: String,
    port: String,
    password: String,
}

impl SavedServer {
    /// Сервер узнаём по адресу и порту.
    fn key(&self) -> String {
        format!("{}:{}", self.ip.trim(), self.port.trim())
    }

    fn label(&self) -> String {
        if self.name.trim().is_empty() { self.key() } else { format!("{} — {}", self.name.trim(), self.key()) }
    }
}

/// Вкладки главного окна.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    List,
    Player,
    Record,
    Net,
}

/// Вид элемента списка «картинка» (открывается как презентация из одного слайда).
const KIND_IMAGE: &str = "КАРТИНКА";

/// Папка в рабочей папке для слайдов PPTX в картинках (превью).
const SLIDES_DIR: &str = "slides";

/// Элемент списка в list.json.
#[derive(Serialize, Deserialize)]
struct SavedItem {
    name: String,
    src: PathBuf,
}

/// Автопереподключение: попыток и пауза между ними (всего около 30 с).
const RECONNECT_TRIES: u32 = 15;
const RECONNECT_EVERY: Duration = Duration::from_secs(2);

/// Что открыть на резервном сервере после переключения.
struct Resume {
    name: String,
    page: usize,
    /// Уже отправлено «открыть» (номер элемента на резерве).
    opened: Option<u64>,
    since: Instant,
}

/// Сколько серверов помнить.
const MAX_SERVERS: usize = 20;

/// Добавить в `ours` серверы из `other`, которых там нет (кроме забытых в этом сеансе).
/// Наш порядок сохраняется: свои — первыми.
fn merge_servers(ours: &mut Vec<SavedServer>, other: Vec<SavedServer>, forgotten: &HashSet<String>) {
    for srv in other {
        let key = srv.key();
        if !forgotten.contains(&key) && !ours.iter().any(|s| s.key() == key) {
            ours.push(srv);
        }
    }
    ours.truncate(MAX_SERVERS);
}

/// Положение и размер главного окна: по центру рабочей области (экран без панели задач)
/// и не выше неё. В логических точках — как их ждёт egui.
fn initial_geometry() -> Option<(egui::Pos2, egui::Vec2)> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};
        let mut r = RECT::default();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut r as *mut RECT as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)).ok()?;
        // До создания окна процесс может ещё не учитывать масштаб — тогда Windows уже отдаёт
        // логические единицы и системный DPI = 96; в обоих случаях деление даёт точки egui.
        let k = windows::Win32::UI::HiDpi::GetDpiForSystem().max(96) as f32 / 96.0;
        let (x, y) = (r.left as f32 / k, r.top as f32 / k);
        let (w, h) = ((r.right - r.left) as f32 / k, (r.bottom - r.top) as f32 / k);
        const FRAME: f32 = 45.0; // заголовок и рамки окна
        let size = egui::vec2(750f32.min(w - 20.0), 620f32.min(h - FRAME - 10.0)).max(egui::vec2(640.0, 420.0));
        let pos = egui::pos2(x + (w - size.x) / 2.0, y + (h - size.y - FRAME) / 2.0).max(egui::pos2(x, y));
        Some((pos, size))
    }
    #[cfg(not(windows))]
    None
}

/// Дескриптор главного окна — своего, а не найденного по заголовку (у двух копий программы
/// на одном компьютере заголовки одинаковые). 0 — не Windows или не удалось.
fn main_hwnd(cc: &eframe::CreationContext<'_>) -> isize {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
}

/// Значок окон (тот же, что вшит в exe). Декодируется один раз.
fn app_icon() -> Arc<egui::IconData> {
    static ICON: std::sync::OnceLock<Arc<egui::IconData>> = std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        let img = image::load_from_memory(include_bytes!("../assets/icon_256.png")).map(|i| i.to_rgba8());
        Arc::new(img.map_or_else(|_| egui::IconData::default(), |i| egui::IconData { width: i.width(), height: i.height(), rgba: i.into_raw() }))
    })
    .clone()
}

fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
}

const CONFIG_FILE: &str = "presentator_settings.json";

/// Настройки — в %APPDATA%\presenter\, как у обычных программ.
fn config_path() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(app_dir);
    base.join("presenter").join(CONFIG_FILE)
}

enum Status {
    Preparing,
    Ready,
    Error(String),
}

struct Item {
    id: u64,
    name: String,
    kind: String,
    src: PathBuf,
    pdf: Option<PathBuf>,
    pages: usize,
    status: Status,
    /// Папка, куда мост к PowerPoint сохраняет слайды PPTX картинками (для превью).
    slides: Option<PathBuf>,
}

enum ShowKind {
    /// PDF рисуем сами в своём окне.
    Pdf(PathBuf),
    /// PPTX показывает PowerPoint; мы им управляем.
    Ppt(PptShow),
    /// Видео — встроенный проигрыватель поверх сцены.
    Video(VideoShow),
    /// Картинка — рисуем сами на сцене, как страницу PDF (анимированная GIF движется).
    Image(Option<background::Background>),
}

struct VideoShow {
    player: video::Video,
    state: VideoState,
    pos: f64,
    dur: f64,
}

/// Показ PowerPoint запускается за сценой и выходит на экран, только когда готов.
struct PptShow {
    phase: PptPhase,
    /// Окно слайд-шоу (из ответа `ready`).
    hwnd: isize,
}

#[derive(Clone, Copy, PartialEq)]
enum PptPhase {
    /// PowerPoint запускается; сцена поверх всех окон и скрывает запуск.
    Loading,
    /// Слайд-шоу запущено, даём ему отрисоваться.
    Ready(Instant),
    /// Сцена опущена под обычные окна; следом выводим слайд-шоу вперёд.
    Lowered(Instant),
    /// Слайд-шоу на экране, сцена лежит под ним.
    Shown,
}

impl Show {
    fn is_ppt(&self) -> bool {
        matches!(self.kind, ShowKind::Ppt(_))
    }

    fn video(&self) -> Option<&VideoShow> {
        match &self.kind {
            ShowKind::Video(v) => Some(v),
            _ => None,
        }
    }

    /// Сцена должна быть поверх всех окон всегда, кроме времени, когда на экране PowerPoint.
    fn stage_on_top(&self) -> bool {
        match &self.kind {
            ShowKind::Ppt(p) => !matches!(p.phase, PptPhase::Lowered(_) | PptPhase::Shown),
            // Идущее видео — наверху, сцена прямо под ним (через «поверх всех» видео не поднять).
            ShowKind::Video(v) => !matches!(v.state, VideoState::Playing | VideoState::Paused),
            ShowKind::Pdf(_) | ShowKind::Image(_) => true,
        }
    }

    fn video_on_screen(&self) -> bool {
        self.video().is_some_and(|v| matches!(v.state, VideoState::Playing | VideoState::Paused))
    }
}

struct Show {
    id: u64,
    page: usize,
    pages: usize,
    kind: ShowKind,
    gen: u64,
    opened: Instant,
}

struct App {
    cfg: Config,
    ctx: egui::Context,
    work_dir: PathBuf,

    // Локальное состояние (оно же состояние сервера)
    items: Vec<Item>,
    next_id: u64,
    show: Option<Show>,
    show_gen: u64,
    dirty: bool,
    /// Постоянный мост к PowerPoint (один на всё время работы).
    ppt_host: ppt::Host,
    /// Набираемый с клавиатуры номер слайда (цифры, затем Enter) и время последней цифры.
    goto_buf: String,
    goto_at: Option<Instant>,
    /// Поле «перейти к слайду» в главном окне.
    goto_field: String,
    /// Серверы, удалённые из списка в этом сеансе, — при сохранении не возвращаем их с диска.
    forgotten: HashSet<String>,
    /// Что последний раз записано в list.json — пишем только при изменениях.
    saved_list: String,

    // Экран сцены без показа (фон, часы, таймер) и таймер
    screen: Screen,
    timer: screen::Timer,
    /// Поля «минуты» и «секунды» таймера в главном окне.
    timer_min: String,
    timer_sec: String,
    /// Когда пришло последнее состояние сервера — от него клиент досчитывает идущий таймер.
    remote_at: Instant,

    // Превью: миниатюры текущего и следующего слайда (у клиента — присланные сервером)
    previews: HashMap<(u64, usize), egui::TextureHandle>,
    /// Те же миниатюры в JPEG — для рассылки клиентам (на сервере).
    thumb_jpg: HashMap<(u64, usize), Arc<Vec<u8>>>,
    /// Миниатюры PPTX, которые сейчас читаются с диска, и те, что не удались.
    thumb_busy: HashSet<(u64, usize)>,
    thumb_failed: HashSet<(u64, usize)>,

    // Состояние сервера (показатели)
    /// Счётчик кадров интерфейса: по нему сборщик понимает, что программа не зависла.
    ui_frames: Arc<AtomicU64>,
    /// Сборщик показателей этого компьютера — работает, пока запущен сервер.
    collector: Option<health::Collector>,
    local_health: Option<health::HealthInfo>,
    /// Показатели сервера, к которому подключены, задержка связи и когда сервер последний раз отвечал.
    remote_health: Option<health::HealthInfo>,
    rtt: Option<u32>,
    last_seen: Option<Instant>,
    /// Сервер отвечает на пинг (новая версия) — только тогда тревожим из-за молчания.
    remote_live: bool,
    /// Когда пришли последние показатели сервера.
    health_at: Option<Instant>,
    /// Связь с сервером оборвалась не по нашей команде — причина (для красной полосы).
    lost: Option<String>,
    /// Автозапуск сервера со сторожем при входе в Windows (состояние читается из реестра при запуске).
    autostart: bool,
    /// Дублирование загружаемых файлов на резервный сервер: загрузчик (и для какого сервера) и его события.
    mirror: Option<(String, net::Uploader)>,
    mirror_tx: Sender<CliEvent>,
    mirror_rx: Receiver<CliEvent>,
    /// После переключения на резерв — открыть там ту же презентацию на том же слайде.
    resume: Option<Resume>,
    /// Что последним было на экране сервера (имя файла, слайд) — переживает обрыв связи.
    last_show: Option<(String, usize)>,
    /// Файлы из ключей `--add` — добавить (загрузить) после запуска / подключения.
    pending_add: Vec<PathBuf>,
    /// Автопереподключение после обрыва: номер попытки и когда делать следующую.
    reconnect: Option<(u32, Instant)>,
    health_open: bool,
    help_open: bool,
    /// Вкладка «Запись» (запись идёт на этом компьютере).
    rec: rec_panel::RecUi,

    // Сцена — полноэкранное окно с фоном, на котором идут показы
    stage: bool,
    /// Что уже отправлено окну сцены: (поверх всех окон, на весь экран).
    stage_sent: Option<(bool, bool)>,
    bg: Option<background::Background>,
    /// Фон загружен (или грузится) для этого появления сцены; номер загрузки.
    bg_checked: bool,
    bg_gen: u64,
    /// На прошлом кадре фон был виден — анимацию двигаем только тогда.
    bg_shown: bool,
    esc_at: Option<Instant>,
    /// Дескриптор главного окна (Windows) и свернули ли мы его на время сцены.
    main_hwnd: isize,
    main_minimized: bool,
    /// Сцена была на экране на прошлом кадре (сворачиваем только в момент её появления).
    stage_up: bool,

    // Рендер
    renderer: Renderer,
    out_tx: Sender<Out>,
    out_rx: Receiver<Out>,
    textures: HashMap<PageKey, egui::TextureHandle>,
    /// Страницы, которые не удалось отрендерить, — повторно не заказываем.
    failed: HashSet<PageKey>,
    last_tex: Option<egui::TextureHandle>,
    render_error: Option<String>,

    // Сервер
    server: Option<ServerHandle>,
    server_online: bool,
    srv_tx: Sender<SrvEvent>,
    srv_rx: Receiver<SrvEvent>,
    clients: usize,

    // Клиент
    client: Option<ClientHandle>,
    connected: bool,
    cli_tx: Sender<CliEvent>,
    cli_rx: Receiver<CliEvent>,
    remote: StateMsg,

    // Плеер (на сервере и в автономном режиме; у клиента — состояние сервера в `remote.audio`)
    player: player::Player,
    /// Что последний раз записано в audio_<порт>.json.
    saved_audio: String,
    /// Выбранный трек в списке плеера и позиция, которую сейчас тянут мышью.
    audio_selected: Option<u64>,
    audio_seek: Option<f32>,

    // Интерфейс
    tab: Tab,
    selected: Option<u64>,
    net_status: String,
    status: String,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Light);
        cc.egui_ctx.style_mut(|s| {
            s.spacing.button_padding = egui::vec2(12.0, 7.0);
            s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        });
        // Если в %APPDATA% настроек ещё нет — берём прежний файл рядом с exe (старые версии хранили их там).
        let cfg: Config = std::fs::read_to_string(config_path())
            .or_else(|_| std::fs::read_to_string(app_dir().join(CONFIG_FILE)))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();


        let (out_tx, out_rx) = channel();
        let ctx = cc.egui_ctx.clone();
        let renderer = Renderer::new(out_tx.clone(), move || ctx.request_repaint());
        let (srv_tx, srv_rx) = channel();
        let (mirror_tx, mirror_rx) = channel();
        let (cli_tx, cli_rx) = channel();

        // Мост к PowerPoint: события приходят с номером показа (он же поколение `gen`).
        let ppt_host = {
            let (tx, ctx) = (out_tx.clone(), cc.egui_ctx.clone());
            ppt::Host::new(move |gen, ev| {
                let _ = tx.send(Out::Ppt { gen, ev });
                ctx.request_repaint();
            })
        };
        let player = {
            let (tx, ctx) = (out_tx.clone(), cc.egui_ctx.clone());
            player::Player::new(cfg.audio_list, cfg.audio_repeat, cfg.audio_volume, move |ev| {
                let _ = tx.send(Out::Audio(ev));
                ctx.request_repaint();
            })
        };
        let mut app = Self {
            player,
            saved_audio: String::new(),
            audio_selected: None,
            audio_seek: None,
            cfg,
            ctx: cc.egui_ctx.clone(),
            work_dir: app_dir().join("presentator_files"),
            items: Vec::new(),
            next_id: 1,
            show: None,
            show_gen: 0,
            dirty: false,
            ppt_host,
            goto_buf: String::new(),
            goto_at: None,
            goto_field: String::new(),
            forgotten: HashSet::new(),
            saved_list: String::new(),
            screen: Screen::Background,
            timer: screen::Timer::default(),
            timer_min: String::new(),
            timer_sec: String::new(),
            remote_at: Instant::now(),
            previews: HashMap::new(),
            thumb_jpg: HashMap::new(),
            thumb_busy: HashSet::new(),
            thumb_failed: HashSet::new(),
            ui_frames: Arc::new(AtomicU64::new(0)),
            collector: None,
            local_health: None,
            remote_health: None,
            rtt: None,
            last_seen: None,
            remote_live: false,
            health_at: None,
            lost: None,
            reconnect: None,
            mirror: None,
            mirror_tx,
            mirror_rx,
            resume: None,
            last_show: None,
            pending_add: Vec::new(),
            autostart: watchdog::autostart_enabled(),
            health_open: false,
            help_open: false,
            rec: rec_panel::RecUi::new(),
            stage: false,
            stage_sent: None,
            bg: None,
            bg_checked: false,
            bg_gen: 0,
            bg_shown: false,
            esc_at: None,
            main_hwnd: main_hwnd(cc),
            main_minimized: false,
            stage_up: false,
            renderer,
            out_tx,
            out_rx,
            textures: HashMap::new(),
            failed: HashSet::new(),
            last_tex: None,
            render_error: None,
            server: None,
            server_online: false,
            srv_tx,
            srv_rx,
            clients: 0,
            client: None,
            connected: false,
            cli_tx,
            cli_rx,
            remote: StateMsg::default(),
            tab: if std::env::args().any(|a| a == "--net") {
                Tab::Net
            } else if std::env::args().any(|a| a == "--record") {
                Tab::Record
            } else {
                Tab::List
            },
            selected: None,
            net_status: String::new(),
            status: String::new(),
        };
        // Автозапуск: `presentator --server` или `presentator --connect`.
        let args: Vec<String> = std::env::args().collect();
        // `--add <файл>` (можно несколько): добавить в список; при `--connect` — загрузить на сервер.
        app.pending_add = args.windows(2).filter(|w| w[0] == "--add").map(|w| PathBuf::from(&w[1])).collect();
        if args.iter().any(|a| a == "--server") {
            app.toggle_server(&cc.egui_ctx); // файлы добавятся, когда сервер восстановит свой список
        } else if args.iter().any(|a| a == "--connect") {
            app.toggle_client(&cc.egui_ctx); // файлы уйдут на сервер после подключения
        } else {
            let add = std::mem::take(&mut app.pending_add);
            app.add_files(add, &cc.egui_ctx);
        }
        app
    }

    fn save_cfg(&self) {
        // Список серверов объединяем с тем, что сейчас на диске: две копии программы на одном
        // компьютере пишут один файл, и одна не должна стирать серверы, запомненные другой.
        let mut cfg = self.cfg.clone();
        let disk: Option<Config> = std::fs::read_to_string(config_path()).ok().and_then(|s| serde_json::from_str(&s).ok());
        merge_servers(&mut cfg.servers, disk.map(|c| c.servers).unwrap_or_default(), &self.forgotten);
        if let Ok(s) = serde_json::to_string_pretty(&cfg) {
            let path = config_path();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, s);
        }
    }

    // ------------------------------------------------------------ состояние

    fn local_state(&self) -> StateMsg {
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
        }
    }

    /// То, что показывает интерфейс: список сервера, если мы подключены клиентом, иначе свой.
    fn view(&self) -> StateMsg {
        if self.connected { self.remote.clone() } else { self.local_state() }
    }

    /// Единая точка управления: клиент отправляет команду на сервер, иначе выполняем у себя.
    fn act(&mut self, cmd: Cmd) {
        if self.connected {
            if let Some(c) = &self.client {
                c.send(CliReq::Cmd(cmd));
            }
        } else {
            self.apply(cmd);
        }
    }

    fn apply(&mut self, cmd: Cmd) {
        self.dirty = true;
        match cmd {
            Cmd::Open { id } => {
                let Some(it) = self.items.iter().find(|i| i.id == id) else { return };
                if !matches!(it.status, Status::Ready) {
                    return;
                }
                let (pages, pdf, src, name, is_video, slides) = (it.pages, it.pdf.clone(), it.src.clone(), it.name.clone(), it.kind == "ВИДЕО", it.slides.clone());
                let is_image = it.kind == KIND_IMAGE;
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
                        let player = video::Video::start(&src, self.cfg.windowed, rect, move |ev| {
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
                        ShowKind::Ppt(PptShow { phase: PptPhase::Loading, hwnd: 0 })
                    }
                };
                self.show = Some(Show { id, page: 0, pages, kind, gen: self.show_gen, opened: Instant::now() });
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
            Cmd::Next if self.show.as_ref().is_some_and(Show::is_ppt) => self.ppt_host.send("next"),
            Cmd::Prev if self.show.as_ref().is_some_and(Show::is_ppt) => self.ppt_host.send("prev"),
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

    fn go(&mut self, f: impl Fn(&Show) -> usize) {
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
                        let id = s.id;
                        self.trim_pages(id, p);
                    }
                }
                ShowKind::Video(_) | ShowKind::Image(_) => {}
            }
        }
    }

    /// Закрывает показ; сцена (если она есть) остаётся и показывает фон.
    fn close_show(&mut self) {
        if self.show.take().is_some_and(|s| s.is_ppt()) {
            // Сцена лежит прямо под слайд-шоу, поэтому при его закрытии сразу виден фон.
            self.ppt_host.close();
        }
        self.goto_buf.clear();
        self.thumb_failed.clear();
        self.reset_pages(true);
    }

    /// Вывод слайд-шоу на экран — по шагам, чтобы запуск не был виден, плюс сторож зависаний.
    fn tick_ppt(&mut self, ctx: &egui::Context) {
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
    }

    /// Сделать картинку фоном сцены (на сервере и в автономном режиме).
    fn set_background(&mut self, path: &Path, name: &str) {
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
    fn pick_background(&mut self) {
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
    fn tick_previews(&mut self, view: &StateMsg) {
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
    fn trim_pages(&mut self, id: u64, cur: usize) {
        self.textures.retain(|k, _| k.id == id && k.page + 2 >= cur && k.page <= cur + PREFETCH_AHEAD);
    }

    fn reset_pages(&mut self, drop_last: bool) {
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
    fn list_path(&self) -> PathBuf {
        self.work_dir.join(format!("list_{}.json", self.cfg.port.trim()))
    }

    /// Список плеера — рядом, в своём файле.
    fn audio_path(&self) -> PathBuf {
        self.work_dir.join(format!("audio_{}.json", self.cfg.port.trim()))
    }

    /// Сохранить список (порядок, имена, файлы) — он переживает перезапуск программы и сбой.
    fn save_list(&mut self) {
        if !self.server_online {
            return;
        }
        let list: Vec<SavedItem> = self.items.iter().map(|i| SavedItem { name: i.name.clone(), src: i.src.clone() }).collect();
        let audio: Vec<SavedItem> = self.player.tracks.iter().map(|t| SavedItem { name: t.name.clone(), src: t.src.clone() }).collect();
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
    fn load_list(&mut self, ctx: &egui::Context) {
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
                self.add_local(it.src, Some(it.name), ctx);
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
    fn delete_files(&self, it: &Item) {
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
    fn slides_dir(&self, src: &Path) -> PathBuf {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        src.hash(&mut h);
        if let Ok(m) = std::fs::metadata(src) {
            m.len().hash(&mut h);
            m.modified().ok().hash(&mut h);
        }
        self.work_dir.join(SLIDES_DIR).join(format!("{:016x}", h.finish()))
    }

    fn add_local(&mut self, path: PathBuf, name: Option<String>, ctx: &egui::Context) {
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
        self.items.push(Item { id, name, kind: kind.into(), src: path.clone(), pdf: None, pages: 0, status: Status::Preparing, slides });
        if kind == "PPTX" {
            // PowerPoint запускаем заранее (скрыто) — к показу он уже будет готов.
            self.ppt_host.prewarm();
        }
        let ctx = ctx.clone();
        render::prepare(id, path, self.out_tx.clone(), move || ctx.request_repaint());
        self.dirty = true;
    }

    /// Добавление файлов: клиент отправляет их на сервер, иначе добавляем к себе.
    fn add_files(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
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

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(out) = self.out_rx.try_recv() {
            match out {
                Out::Prepared { id, result } => {
                    if let Some(it) = self.items.iter_mut().find(|i| i.id == id) {
                        match result {
                            Ok((pdf, pages)) => {
                                it.pdf = pdf;
                                it.pages = pages;
                                it.status = Status::Ready;
                            }
                            Err(e) => it.status = Status::Error(e),
                        }
                        self.dirty = true;
                    }
                }
                Out::Ppt { gen, ev } => {
                    if self.show.as_ref().is_some_and(|s| s.gen == gen) {
                        // Позиция без изменений и сохранённые слайды рассылки не требуют.
                        self.dirty |= !matches!(ev, ppt::PptEvent::Pos(..) | ppt::PptEvent::Slide(_));
                        match ev {
                            ppt::PptEvent::Ready(total, hwnd) => {
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
                                    (s.page, s.pages) = (page, pages);
                                }
                            }
                            // Мост сохранил слайд картинкой — превью может его прочитать.
                            ppt::PptEvent::Slide(n) => {
                                if let Some(s) = &self.show {
                                    self.thumb_failed.remove(&(s.id, n.saturating_sub(1)));
                                }
                            }
                            ppt::PptEvent::Ended => self.close_show(),
                            ppt::PptEvent::Error(e) => {
                                self.close_show();
                                self.status = format!("PowerPoint: {e}");
                            }
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
                        // Доиграло — возвращаем фон.
                        video::VideoEvent::Ended => {
                            self.close_show();
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
                SrvEvent::Clients(n) => self.clients = n,
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
                    self.net_status = if reason.is_empty() { "Отключено".into() } else { reason };
                }
                CliEvent::State(s) => {
                    self.last_seen = Some(Instant::now());
                    self.remote_at = Instant::now();
                    if s.message != self.remote.message {
                        self.status = s.message.clone();
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

    fn port(&mut self) -> Option<u16> {
        match self.cfg.port.trim().parse::<u16>() {
            Ok(p) if p > 0 => Some(p),
            _ => {
                self.net_status = "Неверный порт".into();
                None
            }
        }
    }

    fn toggle_server(&mut self, ctx: &egui::Context) {
        if self.server.take().is_some() {
            self.collector = None;
            self.local_health = None;
            self.server_online = false;
            self.clients = 0;
            self.net_status = "Сервер остановлен".into();
            return;
        }
        let Some(port) = self.port() else { return };
        self.disconnect();
        self.save_cfg();
        self.net_status = "Запуск сервера…".into();
        let ctx = ctx.clone();
        self.server = Some(net::start_server(
            port,
            self.cfg.password.clone(),
            self.work_dir.clone(),
            self.srv_tx.clone(),
            move || ctx.request_repaint(),
        ));
    }

    fn disconnect(&mut self) {
        self.lost = None;
        self.reconnect = None;
        if self.client.take().is_some() {
            self.connected = false;
            self.remote = StateMsg::default();
            self.previews.clear();
        }
    }

    /// Запустить сборщик показателей этого компьютера (на сервере).
    fn start_collector(&mut self) {
        let Some(server) = &self.server else { return };
        let publish = server.health_publisher();
        let (tx, ctx) = (self.out_tx.clone(), self.ctx.clone());
        self.collector = Some(health::start(
            self.work_dir.clone(),
            self.ui_frames.clone(),
            self.ppt_host.lag_probe(),
            move |h| {
                publish(&h); // клиентам — напрямую из потока сбора, без перерисовки окон
                let _ = tx.send(Out::Health(h));
            },
            move || ctx.request_repaint(),
        ));
    }

    /// Оценка состояния сервера: у клиента — показатели сервера и связь, на сервере — свои.
    fn health_rows(&self) -> (health::Level, Vec<health::Row>) {
        let rows = if self.connected {
            // Старый сервер (без пинга) молчит между изменениями — молчание тогда не тревога.
            let silent = self.last_seen.filter(|_| self.remote_live).map(|t| t.elapsed());
            // Пинг отвечает, а показатели не приходят дольше 7 с — программа на сервере зависла целиком.
            let mut h = self.remote_health.clone();
            if let (Some(h), Some(at)) = (h.as_mut(), self.health_at) {
                if at.elapsed() > Duration::from_secs(7) {
                    h.ui_ok = false;
                }
            }
            if self.remote_live { health::assess(h.as_ref(), self.rtt, silent) } else { Vec::new() }
        } else {
            health::assess(self.local_health.as_ref(), None, None)
        };
        (health::overall(&rows), rows)
    }

    /// Загрузчик на резервный сервер (создаётся при первой надобности, заново — если резерв сменили).
    fn mirror_uploader(&mut self) -> Option<&net::Uploader> {
        let srv = self.reserve_server().filter(|_| self.cfg.mirror)?;
        if self.mirror.as_ref().is_none_or(|(k, _)| *k != srv.key()) {
            let port = srv.port.trim().parse().ok()?;
            let ctx = self.ctx.clone();
            let up = net::start_uploader(srv.ip.trim().to_string(), port, srv.password.clone(), self.mirror_tx.clone(), move || ctx.request_repaint());
            self.mirror = Some((srv.key(), up));
        }
        self.mirror.as_ref().map(|(_, u)| u)
    }

    /// Резервный сервер из списка, если он выбран и это не тот, к которому мы уже подключены.
    fn reserve_server(&self) -> Option<SavedServer> {
        let current = format!("{}:{}", self.cfg.ip.trim(), self.cfg.port.trim());
        self.cfg.servers.iter().find(|s| s.key() == self.cfg.reserve && s.key() != current).cloned()
    }

    /// Отключиться от текущего сервера и подключиться к резервному.
    fn switch_to_reserve(&mut self) {
        let Some(srv) = self.reserve_server() else { return };
        // Что было на экране основного сервера — откроем то же на резерве (по имени файла).
        // Список сервера при обрыве уже очищен — берём то, что запомнили.
        self.resume = self.last_show.clone().map(|(name, page)| Resume { name, page, opened: None, since: Instant::now() });
        self.mirror = None; // резерв станет основным
        self.disconnect();
        self.status = format!("Переключение на резервный сервер {}…", srv.label());
        (self.cfg.ip, self.cfg.port, self.cfg.password, self.cfg.name) = (srv.ip, srv.port, srv.password, srv.name);
        let ctx = self.ctx.clone();
        self.toggle_client(&ctx);
    }

    /// Продолжение показа на резерве: найти файл с тем же именем, открыть, перейти на тот же слайд.
    fn continue_resume(&mut self) {
        let Some(r) = &mut self.resume else { return };
        if r.since.elapsed() > Duration::from_secs(20) {
            self.status = format!("На резервном сервере не удалось открыть «{}»", r.name);
            self.resume = None;
            return;
        }
        match r.opened {
            None => {
                let Some(it) = self.remote.items.iter().find(|i| i.name == r.name) else { return };
                if !it.ready {
                    return;
                }
                let id = it.id;
                r.opened = Some(id);
                self.act(Cmd::Open { id });
            }
            Some(id) if self.remote.active == Some(id) => {
                let (page, name) = (r.page, r.name.clone());
                self.resume = None;
                if page > 0 && self.remote.video.is_none() {
                    self.act(Cmd::GoTo { page });
                }
                self.status = format!("На резервном сервере открыт «{name}», слайд {}", page + 1);
            }
            Some(_) => {}
        }
    }

    /// Красная полоса под верхней панелью, пока у сервера проблема.
    fn alarm_bar(&mut self, ctx: &egui::Context) {
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
    fn health_window(&mut self, ctx: &egui::Context) {
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

    /// Подтянуть в список серверы, которые запомнила другая копия программы после нашего запуска.
    fn merge_servers_from_disk(&mut self) {
        let disk: Option<Config> = std::fs::read_to_string(config_path()).ok().and_then(|s| serde_json::from_str(&s).ok());
        merge_servers(&mut self.cfg.servers, disk.map(|c| c.servers).unwrap_or_default(), &self.forgotten);
    }

    /// Запомнить текущий сервер (адрес, порт, пароль, название) первым в списке.
    fn remember_server(&mut self) {
        let srv = SavedServer {
            name: self.cfg.name.trim().to_string(),
            ip: self.cfg.ip.trim().to_string(),
            port: self.cfg.port.trim().to_string(),
            password: self.cfg.password.clone(),
        };
        let key = srv.key();
        self.forgotten.remove(&key);
        self.cfg.servers.retain(|s| s.key() != key);
        self.cfg.servers.insert(0, srv);
        self.cfg.servers.truncate(MAX_SERVERS);
        self.save_cfg();
    }

    fn toggle_client(&mut self, ctx: &egui::Context) {
        if self.client.is_some() {
            self.disconnect();
            self.net_status = "Отключено".into();
            return;
        }
        let Some(port) = self.port() else { return };
        if self.server.is_some() {
            self.net_status = "Сначала остановите сервер в этом окне".into();
            return;
        }
        self.save_cfg();
        self.net_status = "Подключение…".into();
        let ctx = ctx.clone();
        self.client = Some(net::start_client(
            self.cfg.ip.trim().to_string(),
            port,
            self.cfg.password.clone(),
            self.cli_tx.clone(),
            move || ctx.request_repaint(),
        ));
    }

    // ------------------------------------------------------------ интерфейс

    fn top_bar(&mut self, ui: &mut egui::Ui, view: &StateMsg) {
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
                    self.act(if player { Cmd::AudioClear } else { Cmd::Clear });
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

    fn bottom_bar(&mut self, ui: &mut egui::Ui, view: &StateMsg) {
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
                if let Some(id) = self.selected.take() {
                    self.act(Cmd::Remove { id });
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
                        ui.label(RichText::new(format!("Слайд {} из {}", view.page + 1, view.pages)).strong().size(15.0));
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
                ui.add(egui::Label::new(RichText::new(&text).size(11.5).color(Color32::from_gray(0x66))).truncate());
            });
        }));
        if !active && view.screen == Screen::Timer {
            self.timer_bar(ui, view);
        }
    }

    /// Управление таймером — пока на экране сервера таймер.
    fn timer_bar(&mut self, ui: &mut egui::Ui, view: &StateMsg) {
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
    fn timer_extra(&self) -> u64 {
        if self.connected { self.remote_at.elapsed().as_millis() as u64 } else { 0 }
    }

    /// Кнопки «Выбрать…» и «Стандартный» (во вкладке «Сеть» и в меню кнопки «Фон»).
    fn background_menu(&mut self, ui: &mut egui::Ui, current: &str) {
        if ui.button("Выбрать фон…").clicked() {
            ui.close_menu();
            self.pick_background();
        }
        if ui.add_enabled(!current.is_empty(), egui::Button::new("Стандартный фон")).clicked() {
            ui.close_menu();
            self.act(Cmd::BackgroundReset);
        }
    }

    /// Превью у оператора: миниатюры предыдущего, текущего и следующего слайда.
    fn preview_bar(&mut self, ui: &mut egui::Ui, view: &StateMsg) {
        let Some(id) = view.active else { return };
        const GAP: f32 = 12.0;
        // Три миниатюры 16:9 по ширине окна, но не выше 112 точек.
        let h = ((ui.available_width() - 2.0 * GAP) / 3.0 * 9.0 / 16.0).clamp(50.0, 112.0);
        let slot = |ui: &mut egui::Ui, title: String, tex: Option<&egui::TextureHandle>, empty: &str, current: bool| {
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
            });
        };
        ui.horizontal(|ui| {
            let page = view.page;
            match page.checked_sub(1) {
                Some(prev) => slot(ui, format!("Предыдущий — слайд {}", prev + 1), self.previews.get(&(id, prev)), "готовится…", false),
                None => slot(ui, "Предыдущий".into(), None, "начало презентации", false),
            }
            ui.add_space(GAP);
            slot(ui, format!("Сейчас — слайд {}", page + 1), self.previews.get(&(id, page)), "готовится…", true);
            ui.add_space(GAP);
            if page + 1 < view.pages {
                slot(ui, format!("Далее — слайд {}", page + 2), self.previews.get(&(id, page + 1)), "готовится…", false);
            } else {
                slot(ui, "Далее".into(), None, "конец презентации", false);
            }
        });
    }

    /// Окно справки: что где нажимать.
    fn help_window(&mut self, ctx: &egui::Context) {
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
    fn tab_row(&mut self, ui: &mut egui::Ui, view: &StateMsg) {
        ui.horizontal(|ui| {
            for (tab, label) in [(Tab::List, "Презентации"), (Tab::Player, "Плеер"), (Tab::Record, "Запись"), (Tab::Net, "Сеть")] {
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
            let playing = now_playing(&view.audio);
            let rec = self.rec_badge();
            if playing.is_some() || rec.is_some() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Идёт запись — видно с любой вкладки; щелчок открывает «Запись».
                    if let Some((text, color)) = rec {
                        if ui.add(egui::Label::new(RichText::new(text).color(color).strong()).sense(egui::Sense::click())).on_hover_text("Вкладка «Запись»").clicked() {
                            self.tab = Tab::Record;
                        }
                        // Кружок рисуем сами: в шрифте egui нет символа «●».
                        let (dot, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 6.0, color);
                        ui.add_space(10.0);
                    }
                    if let Some((text, color)) = playing {
                        if ui.add(cbutton("■", RED)).on_hover_text("Остановить музыку (плавно)").clicked() {
                            self.act(Cmd::AudioStop);
                        }
                        ui.add(egui::Label::new(RichText::new(text).color(color).strong()).truncate());
                    }
                });
            }
        });
    }

    /// Вкладка «Плеер»: режим и громкость, список треков, позиция и кнопки.
    fn player_panel(&mut self, ui: &mut egui::Ui, view: &StateMsg) {
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
    fn player_controls(&mut self, ui: &mut egui::Ui, a: &AudioInfo) {
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
                    if let Some(id) = self.audio_selected.take() {
                        self.act(Cmd::AudioRemove { id });
                    }
                }
            });
        });
        // Пока звучит — обновлять позицию (сервер присылает её раз в секунду).
        if active {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
    }

    fn list(&mut self, ui: &mut egui::Ui, view: &StateMsg) {
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
                        let info: String = if info.chars().count() > 60 { info.chars().take(60).chain("…".chars()).collect() } else { info };
                        p.text(rect.right_center() - egui::vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, info, egui::FontId::proportional(12.0), col);
                        let resp = if it.status.is_empty() { resp } else { resp.on_hover_text(&it.status) };
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

    fn network_panel(&mut self, ui: &mut egui::Ui) {
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
            // Монитор для показа — на сервере: основной или любой из подключённых.
            ui.add_space(8.0);
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
            ui.add_space(8.0);
            let current = self.view().background;
            ui.horizontal(|ui| {
                ui.label("Фон экрана показа:");
                ui.label(RichText::new(if current.is_empty() { "стандартный" } else { current.as_str() }).strong());
                self.background_menu(ui, &current);
            })
            .response
            .on_hover_text("Картинка (PNG, JPG, BMP) или анимация (GIF, анимированные WebP и PNG). Копируется в папку настроек сервера; прежний выбранный фон заменяется");

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
            })
            .response
            .on_hover_text("Если с основным сервером беда, на резервный можно переключиться одной кнопкой — в красной полосе или в окне показателей");
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
            ui.checkbox(&mut self.cfg.windowed, "Сцена и показ в окне, а не на весь экран (только для теста на одном компьютере)");
            let mut auto = self.autostart;
            let resp = ui.checkbox(&mut auto, "Запускать сервер при входе в Windows (с самовосстановлением после сбоя)").on_hover_text(
                r"Сервер стартует сам при входе в Windows. Сторож следит за ним: упал или завис — перезапустит, с тем же списком. Закрыли сервер сами — сторож тоже завершится. Журнал: %APPDATA%\presenter\watchdog.log",
            );
            if resp.changed() {
                match watchdog::set_autostart(auto) {
                    Ok(()) => self.autostart = auto,
                    Err(e) => self.net_status = format!("Не удалось изменить автозапуск: {e}"),
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

    fn keys(&mut self, ctx: &egui::Context, view: &StateMsg) {
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
    fn goto_typed(&mut self, digits: &str, enter: bool, view: &StateMsg) -> bool {
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
    fn goto_number(&mut self, n: &str, view: &StateMsg) {
        match n.parse::<usize>() {
            Ok(n) if n >= 1 && n <= view.pages => self.act(Cmd::GoTo { page: n - 1 }),
            _ if n.is_empty() => {}
            _ => self.status = format!("Нет слайда {n}: в презентации {} слайдов", view.pages),
        }
    }

    /// Сцена: полноэкранное окно сервера. Показывает фон, а поверх — PDF;
    /// PowerPoint выводит своё слайд-шоу поверх неё. Скрывается двойным Esc, когда на ней только фон.
    fn stage_window(&mut self, ctx: &egui::Context) {
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
            // Пока на экране сцена (фон или презентация), курсора не видно.
            ctx.set_cursor_icon(egui::CursorIcon::None);
            egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ctx, |ui| {
                let rect = ui.max_rect();
                let mut page_tex = None;
                if let Some((id, page, pages, _)) = &pdf {
                    let (id, page, pages) = (*id, *page, *pages);
                    let ppp = ctx.pixels_per_point();
                    let max_side = ctx.input(|i| i.max_texture_side).min(16384) as f32;
                    let w = (rect.width() * ppp).round().clamp(16.0, max_side) as u32;
                    let h = (rect.height() * ppp).round().clamp(16.0, max_side) as u32;
                    let key = PageKey { id, page, w, h, thumb: false };
                    let tex = textures.get(&key).cloned();
                    if tex.is_some() {
                        *last_tex = tex.clone();
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
                if page_tex.is_none() {
                    page_tex = img_tex.clone().filter(|_| hold.is_zero());
                }
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                match page_tex {
                    Some(t) => {
                        let ts = t.size_vec2();
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
                    wanted.push(PageKey { id, page: p, w: THUMB_W, h: THUMB_H, thumb: true });
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
    fn minimize_for_stage(&mut self, ctx: &egui::Context) {
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
    fn esc_press(&mut self) {
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

/// Справка: разделы и строки «что — как».
const HELP: &[(&str, &[(&str, &str)])] = &[
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
        ("Видео", "Готовится за фоном: «готово к запуску». «Старт» или пробел — сразу на экран показа. Доиграло — снова фон."),
        ("Закрыть показ", "Кнопка «Закрыть показ», Shift+0 или двойной Esc на экране показа. На экране показа остаётся фон."),
    ]),
    ("Сеть, состояние сервера, резерв", &[
        ("Индикатор вверху", "Зелёный — норма, жёлтый — внимание, красный — проблема. Щелчок — показатели сервера: процессор, память, видеокарта, мониторы, связь."),
        ("Красная полоса", "Проблема на сервере или пропала связь. Клиент сам переподключается (до 15 попыток), можно сразу перейти на резерв."),
        ("Резервный сервер", "«Сеть», затем «Резервный сервер». Файлы можно сразу дублировать на него. «Переключиться на резерв» откроет там тот же файл на том же слайде."),
        ("Сохранённые серверы", "Сервер запоминается после успешного подключения. «Забыть» убирает его из списка."),
    ]),
    ("Настройки сервера", &[
        ("Монитор для показа", "«Сеть», затем «Монитор для показа»: основной или любой из подключённых (на компьютере с одним монитором — он же). Отключили выбранный — показ идёт на основном."),
        ("Автозапуск", "Галочка «Запускать сервер при входе в Windows»: сервер стартует сам и перезапускается при сбое или зависании."),
        ("Фон экрана показа", "Вкладка «Сеть» или правый щелчок по кнопке «Фон»: «Выбрать фон…» — картинка (PNG, JPG, BMP) или анимация (GIF, анимированные WebP и PNG). С клиента она уходит на сервер. Новый фон заменяет прежний; «Стандартный фон» — вернуть встроенный. Видео фоном не ставится — сделайте из ролика GIF любым конвертером (лучше до 1280 точек по ширине и 10–20 секунд)."),
        ("Брандмауэр", "При первом запуске сервера Windows спросит разрешение — разрешите, иначе клиенты с других компьютеров не подключатся."),
        ("Порт и пароль", "По умолчанию 14999 и default_password; должны совпадать на сервере и клиенте."),
    ]),
    ("Запись мероприятия", &[
        ("Где пишется", "На том компьютере, где нажали «Начать запись» (обычно у оператора), — не на сервере. По сети ничего не передаётся."),
        ("Источники", "Вкладка «Запись»: видео — карта захвата HDMI или камера, звук — звук той же карты, пульт в линейный вход или микрофон. Карту захвата программа выбирает сама, если она подключена. «Без видео» — запись только звука."),
        ("Проверить сигнал", "Картинка и уровень звука без записи в файл — проверить перед началом. «Нет звука» — 10 секунд тишины."),
        ("Запись", "«Начать запись» / «Остановить запись». Пока идёт запись, справа от вкладок — красное «Запись» и время (видно с любой вкладки). Видео сжимает видеокарта, если умеет, процессор почти не занят."),
        ("Файлы", "Папка «Записи» рядом с программой (можно выбрать другую). Имя — дата и время начала. Во время записи файл — MKV (не пропадёт при сбое), после остановки — MP4 (только звук — M4A)."),
        ("Устройство занято", "Карту захвата одновременно может держать только одна программа: закройте OBS (или уберите в нём этот источник)."),
        ("Выход", "Закрыть программу во время записи — она спросит, остановить ли запись, и закроется, когда файл сохранится."),
    ]),
];

/// Миниатюра нужна: предыдущий слайд, текущий и два следующих (при листании назад не ждём).
fn thumb_near(page: usize, cur: usize) -> bool {
    page + 1 >= cur && page <= cur + 2
}

/// Цифры, набранные без Shift (Shift+цифра открывает презентацию по номеру), и нажат ли Enter.
fn typed_digits(ctx: &egui::Context) -> (String, bool) {
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
fn nav_keys(ctx: &egui::Context, view: &StateMsg) -> Vec<Cmd> {
    use egui::Key;
    let mut out = Vec::new();
    ctx.input(|i| {
        let active = view.active.is_some();
        if active {
            if i.key_pressed(Key::Space) {
                out.push(Cmd::Play);
            }
            if [Key::ArrowRight, Key::ArrowDown, Key::PageDown].iter().any(|k| i.key_pressed(*k)) {
                out.push(Cmd::Next);
            }
            if [Key::ArrowLeft, Key::ArrowUp, Key::PageUp, Key::Backspace].iter().any(|k| i.key_pressed(*k)) {
                out.push(Cmd::Prev);
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
fn audio_time(pos: u32, dur: u32) -> String {
    let t = |s: u32| format!("{:02}:{:02}", s / 60, s % 60);
    if dur > 0 { format!("{} / {}", t(pos), t(dur)) } else { t(pos) }
}

/// Что играет плеер — для строки вкладок (None — тишина).
fn now_playing(a: &AudioInfo) -> Option<(String, Color32)> {
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
fn video_label(v: &VideoInfo) -> (String, Color32) {
    let t = |s: u32| format!("{:02}:{:02}", s / 60, s % 60);
    match v.state {
        VideoState::Loading => ("видео загружается…".into(), GRAY),
        VideoState::Armed => ("готово к запуску — пробел или «Старт»".into(), GREEN),
        VideoState::Playing => (format!("▶ {} / {}", t(v.pos), t(v.dur)), GREEN),
        VideoState::Paused => (format!("пауза {} / {}", t(v.pos), t(v.dur)), BLUE),
    }
}

fn level_color(level: health::Level) -> Color32 {
    match level {
        health::Level::Ok => GREEN,
        health::Level::Warn => Color32::from_rgb(0xe0, 0x9a, 0x00),
        health::Level::Bad => RED,
    }
}

fn cbutton(text: &str, color: Color32) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).color(Color32::WHITE).strong()).fill(color).corner_radius(5.0)
}

fn btn(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    ui.add(cbutton(text, color))
}

fn indicator(ui: &mut egui::Ui, color: Color32, text: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 6.0, color);
    ui.label(RichText::new(text).size(11.0).color(GRAY));
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui_frames.fetch_add(1, Ordering::SeqCst);
        self.poll(ctx);
        self.poll_rec(ctx);
        if let Some((n, at)) = self.reconnect {
            if self.client.is_none() && Instant::now() >= at {
                self.reconnect = Some((n + 1, Instant::now() + Duration::from_secs(3600)));
                let ctx = ctx.clone();
                self.toggle_client(&ctx);
                self.net_status = format!("Переподключение… попытка {} из {RECONNECT_TRIES}", n + 1);
            } else if self.client.is_none() {
                ctx.request_repaint_after(at.saturating_duration_since(Instant::now()));
            }
        }
        if self.connected {
            // Раз в секунду пересчитываем, давно ли сервер отвечал.
            ctx.request_repaint_after(Duration::from_secs(1));
        }

        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() {
            self.add_files(dropped, ctx);
        }

        let view = self.view();
        self.keys(ctx, &view);
        self.tick_previews(&view);
        // Идущий таймер в главном окне — обновлять при смене секунды.
        if view.active.is_none() && view.screen == Screen::Timer && view.timer.running {
            ctx.request_repaint_after(screen::timer_tick(&view.timer, self.timer_extra()));
        }

        let margin = egui::Frame::new().fill(Color32::from_gray(0xfa)).inner_margin(6.0);
        egui::TopBottomPanel::top("top").frame(margin).show(ctx, |ui| self.top_bar(ui, &view));
        self.alarm_bar(ctx);
        self.health_window(ctx);
        self.help_window(ctx);
        self.rec_close_guard(ctx);
        egui::TopBottomPanel::bottom("bottom").frame(margin).show(ctx, |ui| self.bottom_bar(ui, &view));
        // Превью — над нижней панелью, пока открыта презентация (у видео превью нет).
        if self.tab == Tab::List && view.active.is_some() && view.video.is_none() {
            egui::TopBottomPanel::bottom("preview").frame(margin).show(ctx, |ui| self.preview_bar(ui, &view));
        }
        egui::CentralPanel::default().frame(margin).show(ctx, |ui| {
            self.tab_row(ui, &view);
            ui.add_space(4.0);
            match self.tab {
                Tab::List => self.list(ui, &view),
                Tab::Player => self.player_panel(ui, &view),
                Tab::Record => self.record_panel(ui),
                Tab::Net => self.network_panel(ui),
            }
        });

        self.tick_ppt(ctx);
        self.minimize_for_stage(ctx);
        ppt::grab_esc(self.show.as_ref().is_some_and(|s| matches!(s.kind, ShowKind::Ppt(_))));
        if !self.connected {
            self.stage_window(ctx);
        }

        if self.dirty {
            self.dirty = false;
            if let Some(s) = &self.server {
                s.publish(&self.local_state());
            }
            self.save_list();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Запись (или проверка сигнала) — дописать файл, не бросать FFmpeg.
        self.rec.finish();
        self.save_cfg();
        // Список и присланные файлы остаются — при следующем запуске список восстановится.
        self.save_list();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Сквозной тест против запущенного экземпляра `presentator --server`:
    /// TEST_FILES="a.pdf;b.pptx" cargo test e2e -- --ignored --nocapture
    #[test]
    #[ignore]
    fn e2e() {
        let files: Vec<PathBuf> = std::env::var("TEST_FILES").unwrap().split(';').map(PathBuf::from).collect();
        let (tx, rx) = channel();
        let wait = |what: &str, f: &dyn Fn(&CliEvent) -> bool| {
            let t = Instant::now();
            loop {
                let ev = rx.recv_timeout(Duration::from_secs(90).saturating_sub(t.elapsed())).unwrap_or_else(|_| panic!("timeout: {what}"));
                if let CliEvent::Closed(r) = &ev { panic!("closed: {r}"); }
                if let CliEvent::UploadErr { msg, .. } = &ev { panic!("upload: {msg}"); }
                if let CliEvent::State(s) = &ev { println!("state: {s:?}"); }
                if f(&ev) { return; }
            }
        };
        // Неверный пароль должен давать понятный отказ.
        let (btx, brx) = channel();
        let _bad = net::start_client("127.0.0.1".into(), 14999, "wrong".into(), btx, || {});
        match brx.recv_timeout(Duration::from_secs(10)).unwrap() {
            CliEvent::Closed(r) => assert_eq!(r, "Неверный пароль"),
            _ => panic!("ожидался отказ"),
        }

        let c = net::start_client("127.0.0.1".into(), 14999, "default_password".into(), tx, || {});
        wait("connect", &|e| matches!(e, CliEvent::Connected));
        // Список на сервере переживает перезапуск — начинаем с чистого.
        c.send(CliReq::Cmd(Cmd::Clear));
        wait("cleared", &|e| matches!(e, CliEvent::State(s) if s.items.is_empty()));
        for f in &files { c.send(CliReq::Upload(f.clone())); }
        let n = files.len();
        let ids = std::cell::RefCell::new(Vec::new());
        wait("ready", &|e| {
            let ok = matches!(e, CliEvent::State(s) if s.items.len() == n && s.items.iter().all(|i| i.ready));
            if let (true, CliEvent::State(s)) = (ok, e) { *ids.borrow_mut() = s.items.iter().map(|i| i.id).collect(); }
            ok
        });
        let ids = ids.into_inner();
        // TEST_OPEN / TEST_SWITCH — позиция в списке (1 — первый загруженный файл).
        let pos = |v: u64| ids[(v as usize).saturating_sub(1).min(ids.len() - 1)];
        let step: u64 = pos(std::env::var("TEST_OPEN").ok().and_then(|s| s.parse().ok()).unwrap_or(1));
        c.send(CliReq::Cmd(Cmd::Open { id: step }));
        wait("open", &|e| matches!(e, CliEvent::State(s) if s.active == Some(step) && s.page == 0));
        c.send(CliReq::Cmd(Cmd::Next));
        wait("next", &|e| matches!(e, CliEvent::State(s) if s.page == 1));
        c.send(CliReq::Cmd(Cmd::Prev));
        wait("prev", &|e| matches!(e, CliEvent::State(s) if s.page == 0));
        c.send(CliReq::Cmd(Cmd::Next));
        wait("next", &|e| matches!(e, CliEvent::State(s) if s.page == 1));
        let hold = Duration::from_secs(std::env::var("TEST_HOLD").ok().and_then(|s| s.parse().ok()).unwrap_or(0));
        std::thread::sleep(hold);
        // TEST_SWITCH=id — переключиться на другую презентацию, затем закрыть показ (на сцене останется фон).
        if let Some(next) = std::env::var("TEST_SWITCH").ok().and_then(|s| s.parse::<u64>().ok()).map(pos) {
            c.send(CliReq::Cmd(Cmd::Open { id: next }));
            wait("switch", &|e| matches!(e, CliEvent::State(s) if s.active == Some(next)));
            std::thread::sleep(hold);
            c.send(CliReq::Cmd(Cmd::Close));
            wait("close", &|e| matches!(e, CliEvent::State(s) if s.active.is_none()));
            std::thread::sleep(hold);
        }
    }

    fn srv(name: &str, port: &str) -> SavedServer {
        SavedServer { name: name.into(), ip: "10.0.0.1".into(), port: port.into(), password: "p".into() }
    }

    #[test]
    fn merge_servers_keeps_ours_first_and_skips_forgotten() {
        let mut ours = vec![srv("Зал 2", "2"), srv("Зал 1", "1")];
        let disk = vec![srv("Зал 1 (старое имя)", "1"), srv("Зал 3", "3"), srv("Зал 4", "4")];
        let forgotten: HashSet<String> = ["10.0.0.1:4".to_string()].into();
        merge_servers(&mut ours, disk, &forgotten);
        let names: Vec<&str> = ours.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Зал 2", "Зал 1", "Зал 3"]);
    }

    #[test]
    fn config_reads_null_servers_without_losing_other_settings() {
        let c: Config = serde_json::from_str(r#"{"ip":"10.1.2.3","port":"15000","servers":null}"#).unwrap();
        assert_eq!((c.ip.as_str(), c.port.as_str(), c.servers.len()), ("10.1.2.3", "15000", 0));
        // Старый файл без новых полей тоже читается.
        let c: Config = serde_json::from_str(r#"{"ip":"10.1.2.3","port":"14999","password":"x","windowed":false}"#).unwrap();
        assert!(c.servers.is_empty() && c.name.is_empty());
    }

    /// Переключение презентаций против запущенного `presentator --server`:
    /// TEST_FILES="a.pptx;b.pptx;c.pdf" TEST_SEQ="1,2,1,3,2" TEST_HOLD=4 cargo test --release e2e_switch -- --ignored --nocapture
    /// Печатает момент каждой команды (unix-мс), чтобы сопоставить со снимками экрана.
    #[test]
    #[ignore]
    fn e2e_switch() {
        let files: Vec<PathBuf> = std::env::var("TEST_FILES").unwrap().split(';').map(PathBuf::from).collect();
        let seq: Vec<u64> = std::env::var("TEST_SEQ").unwrap().split(',').map(|x| x.trim().parse().unwrap()).collect();
        let hold = Duration::from_secs_f64(std::env::var("TEST_HOLD").ok().and_then(|s| s.parse().ok()).unwrap_or(4.0));
        let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
        let (tx, rx) = channel();
        let wait = |what: &str, f: &dyn Fn(&StateMsg) -> bool| {
            let t = Instant::now();
            loop {
                match rx.recv_timeout(Duration::from_secs(60).saturating_sub(t.elapsed())).unwrap_or_else(|_| panic!("timeout: {what}")) {
                    CliEvent::Closed(r) => panic!("closed: {r}"),
                    CliEvent::State(s) => {
                        if !s.message.is_empty() {
                            println!("MSG {} {}", now(), s.message);
                        }
                        if f(&s) {
                            return;
                        }
                    }
                    _ => {}
                }
            }
        };
        let port = std::env::var("TEST_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(14999);
        let c = net::start_client("127.0.0.1".into(), port, "default_password".into(), tx, || {});
        c.send(CliReq::Cmd(Cmd::Clear)); // список на сервере переживает перезапуск — начинаем с чистого
        wait("cleared", &|s| s.items.is_empty());
        for f in &files {
            c.send(CliReq::Upload(f.clone()));
        }
        let n = files.len();
        let ids = std::cell::RefCell::new(Vec::new());
        wait("ready", &|s| {
            let ok = s.items.len() == n && s.items.iter().all(|i| i.ready);
            if ok {
                *ids.borrow_mut() = s.items.iter().map(|i| i.id).collect();
            }
            ok
        });
        // TEST_SEQ — позиции в списке (1 — первый файл), переводим во внутренние номера.
        let ids = ids.into_inner();
        let seq: Vec<(u64, u64)> = seq.iter().map(|&p| (p, ids[(p as usize).saturating_sub(1).min(ids.len() - 1)])).collect();
        // TEST_LAST_HOLD — сколько держать последнюю презентацию (для проверки после быстрых переключений).
        let last_hold = std::env::var("TEST_LAST_HOLD").ok().and_then(|s| s.parse().ok()).map(Duration::from_secs_f64);
        let count = seq.len();
        for (k, (position, id)) in seq.into_iter().enumerate() {
            println!("OPEN {position} {}", now());
            c.send(CliReq::Cmd(Cmd::Open { id }));
            wait("open", &|s| s.active == Some(id));
            // Пока держим показ — печатаем смену слайда (для проверки перехода по номеру).
            let hold = if k + 1 == count { last_hold.unwrap_or(hold) } else { hold };
            let end = Instant::now() + hold;
            let mut last = None;
            while let Ok(ev) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
                if let CliEvent::State(s) = ev {
                    if last != Some(s.page) {
                        println!("PAGE {} {}/{}", now(), s.page + 1, s.pages);
                        last = Some(s.page);
                    }
                }
            }
        }
        println!("CLOSE {}", now());
        c.send(CliReq::Cmd(Cmd::Close));
        wait("close", &|s| s.active.is_none());
        std::thread::sleep(Duration::from_secs(2));
    }

    /// Печатает список на запущенном `presentator --server` (для проверки, что он пережил перезапуск):
    /// cargo test --release e2e_list -- --ignored --nocapture
    #[test]
    #[ignore]
    fn e2e_list() {
        let (tx, rx) = channel();
        let port = std::env::var("TEST_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(14999);
        let _c = net::start_client("127.0.0.1".into(), port, "default_password".into(), tx, || {});
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            if let Ok(CliEvent::State(s)) = rx.recv_timeout(Duration::from_secs(10)) {
                for (n, i) in s.items.iter().enumerate() {
                    println!("ITEM {} {} | {} | готов: {} | {}", n + 1, i.name, i.kind, i.ready, i.status);
                }
                println!("ITEMS {}", s.items.len());
                for (n, t) in s.audio.tracks.iter().enumerate() {
                    println!("AUDIO {} {}", n + 1, t.name);
                }
                println!("AUDIO_TRACKS {} режим: по списку {}, повтор {}, громкость {}", s.audio.tracks.len(), s.audio.list, s.audio.repeat, s.audio.volume);
                let active = s.active.and_then(|id| s.items.iter().find(|i| i.id == id)).map(|i| i.name.clone());
                println!("ACTIVE {:?} слайд {} из {}", active, s.page + 1, s.pages);
                return;
            }
        }
        panic!("нет состояния от сервера");
    }

    /// Открыть элемент TEST_OPEN (позиция в списке) на слайде TEST_PAGE (с 1) на сервере TEST_PORT.
    /// cargo test --release e2e_cmd -- --ignored --nocapture
    #[test]
    #[ignore]
    fn e2e_cmd() {
        let env = |k: &str, d: usize| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        let (tx, rx) = channel();
        let c = net::start_client("127.0.0.1".into(), env("TEST_PORT", 14999) as u16, "default_password".into(), tx, || {});
        let (pos, page) = (env("TEST_OPEN", 1), env("TEST_PAGE", 1));
        let t = Instant::now();
        let mut sent = (false, false);
        while t.elapsed() < Duration::from_secs(20) {
            let Ok(CliEvent::State(s)) = rx.recv_timeout(Duration::from_secs(1)) else { continue };
            let Some(id) = s.items.get(pos - 1).map(|i| i.id) else { continue };
            if !sent.0 {
                c.send(CliReq::Cmd(Cmd::Open { id }));
                sent.0 = true;
            } else if s.active == Some(id) && !sent.1 {
                c.send(CliReq::Cmd(Cmd::GoTo { page: page - 1 }));
                sent.1 = true;
            } else if s.active == Some(id) && s.page == page - 1 {
                println!("OPENED {} слайд {}", s.items[pos - 1].name, page);
                return;
            }
        }
        panic!("не открылось");
    }

    /// Плеер против запущенного `presentator --server`:
    /// TEST_AUDIO="музыка 1.wav;музыка 2.wav;фанфары.wav" cargo test --release e2e_audio -- --ignored --nocapture
    /// Нужны три файла: длинный (≥ 6 с), второй (≥ 3 с) и короткий (вставка, ≤ 3 с).
    #[test]
    #[ignore]
    fn e2e_audio() {
        let files: Vec<PathBuf> = std::env::var("TEST_AUDIO").unwrap().split(';').map(PathBuf::from).collect();
        let (tx, rx) = channel();
        let t0 = Instant::now();
        let wait = |what: &str, secs: u64, f: &dyn Fn(&AudioInfo) -> bool| -> AudioInfo {
            let t = Instant::now();
            loop {
                match rx.recv_timeout(Duration::from_secs(secs).saturating_sub(t.elapsed())).unwrap_or_else(|_| panic!("timeout: {what}")) {
                    CliEvent::Closed(r) => panic!("closed: {r}"),
                    CliEvent::State(s) => {
                        let a = s.audio;
                        if f(&a) {
                            println!(
                                "{:>6} ms OK {what}: музыка {:?} {:?} {}/{} с, вставка {:?} {:?} {}/{} с",
                                t0.elapsed().as_millis(), a.current, a.state, a.pos, a.dur, a.insert, a.insert_state, a.insert_pos, a.insert_dur
                            );
                            return a;
                        }
                    }
                    _ => {}
                }
            }
        };
        let port = std::env::var("TEST_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(14999);
        let c = net::start_client("127.0.0.1".into(), port, "default_password".into(), tx, || {});
        c.send(CliReq::Cmd(Cmd::AudioClear));
        wait("cleared", 10, &|a| a.tracks.is_empty());
        for f in &files {
            c.send(CliReq::Upload(f.clone()));
        }
        let a = wait("uploaded", 30, &|a| a.tracks.len() == 3);
        let (m1, m2, fan) = (a.tracks[0].id, a.tracks[1].id, a.tracks[2].id);
        c.send(CliReq::Cmd(Cmd::AudioMode { list: true, repeat: false }));
        c.send(CliReq::Cmd(Cmd::AudioVolume { volume: 60 }));
        wait("mode list, volume 60", 10, &|a| a.list && !a.repeat && a.volume == 60);

        // Музыка, через 2 с — вставка: музыка на паузе, после вставки продолжается с того же места.
        c.send(CliReq::Cmd(Cmd::AudioPlay { id: m1 }));
        let a = wait("music 1 playing 2 s", 10, &|a| a.current == Some(m1) && a.state == AudioState::Playing && a.pos >= 2);
        c.send(CliReq::Cmd(Cmd::AudioInsert { id: fan }));
        let before = wait("insert playing, music paused", 10, &|a| a.insert == Some(fan) && a.state == AudioState::Paused).pos;
        assert!(before >= a.pos);
        wait("insert done, music resumed", 10, &|a| a.insert.is_none() && a.state == AudioState::Playing && a.current == Some(m1));
        let a = wait("music continues", 10, &|a| a.state == AudioState::Playing && a.pos > before);
        assert!(a.pos < before + 3, "музыка должна продолжиться с того же места, а не начаться заново");

        // По списку: после первого трека сам пошёл второй.
        wait("list: next track", 15, &|a| a.current == Some(m2) && a.state == AudioState::Playing);
        // Стоп — плавно (затухание), затем тишина.
        c.send(CliReq::Cmd(Cmd::AudioStop));
        wait("stopping (fade)", 5, &|a| a.state == AudioState::Stopping);
        let t = Instant::now();
        wait("stopped", 5, &|a| a.state == AudioState::Stopped);
        println!("затухание: {} мс", t.elapsed().as_millis());

        // «Стоп вставки»: вставка обрывается сразу, музыка продолжается с того же места.
        c.send(CliReq::Cmd(Cmd::AudioPlay { id: m1 }));
        wait("music 1 for end-insert", 10, &|a| a.current == Some(m1) && a.state == AudioState::Playing && a.pos >= 1);
        c.send(CliReq::Cmd(Cmd::AudioInsert { id: m2 }));
        let before = wait("long insert playing", 10, &|a| a.insert == Some(m2) && a.insert_pos >= 1).pos;
        c.send(CliReq::Cmd(Cmd::AudioEndInsert));
        let a = wait("insert ended by button, music resumed", 5, &|a| a.insert.is_none() && a.state == AudioState::Playing);
        assert!(a.pos <= before + 1, "музыка должна продолжиться с того же места");
        // По списку: после последнего трека — снова первый.
        c.send(CliReq::Cmd(Cmd::AudioPlay { id: fan }));
        wait("last track playing", 10, &|a| a.current == Some(fan) && a.state == AudioState::Playing);
        wait("list wraps to first", 10, &|a| a.current == Some(m1) && a.state == AudioState::Playing);
        // «Повтор» и в режиме «По списку» крутит текущий трек: короткий всё ещё играет после своей длительности.
        c.send(CliReq::Cmd(Cmd::AudioMode { list: true, repeat: true }));
        c.send(CliReq::Cmd(Cmd::AudioPlay { id: fan }));
        wait("list + repeat playing", 10, &|a| a.current == Some(fan) && a.state == AudioState::Playing);
        std::thread::sleep(Duration::from_millis(4500));
        wait("same track still playing after 4.5 s", 5, &|a| a.current == Some(fan) && a.state == AudioState::Playing);
        // Один трек без повтора: доиграл и остановился, трек остался выбранным.
        c.send(CliReq::Cmd(Cmd::AudioMode { list: false, repeat: false }));
        c.send(CliReq::Cmd(Cmd::AudioPlay { id: fan }));
        wait("one: playing", 10, &|a| a.state == AudioState::Playing);
        wait("one: stopped by itself", 10, &|a| a.state == AudioState::Stopped && a.current == Some(fan));
        // Повторный «Стоп» во время затухания — сразу.
        c.send(CliReq::Cmd(Cmd::AudioPlay { id: m1 }));
        wait("music 1 again", 10, &|a| a.state == AudioState::Playing && a.pos >= 1);
        c.send(CliReq::Cmd(Cmd::AudioStop));
        wait("stopping", 5, &|a| a.state == AudioState::Stopping);
        let t = Instant::now();
        c.send(CliReq::Cmd(Cmd::AudioStop));
        wait("stopped at once", 5, &|a| a.state == AudioState::Stopped);
        println!("второй «Стоп»: {} мс", t.elapsed().as_millis());
        c.send(CliReq::Cmd(Cmd::AudioMode { list: true, repeat: false }));
        c.send(CliReq::Cmd(Cmd::AudioVolume { volume: 100 }));
        wait("restore mode", 10, &|a| a.list && a.volume == 100);
    }

    /// Фон сцены с клиента: TEST_BG="картинка.gif" (загрузить как фон) или без неё — вернуть стандартный.
    /// TEST_PORT — порт сервера. cargo test --release e2e_background -- --ignored --nocapture
    #[test]
    #[ignore]
    fn e2e_background() {
        let port = std::env::var("TEST_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(14999);
        let (tx, rx) = channel();
        let c = net::start_client("127.0.0.1".into(), port, "default_password".into(), tx, || {});
        let t0 = Instant::now();
        let wait = |what: &str, f: &dyn Fn(&StateMsg) -> bool| loop {
            match rx.recv_timeout(Duration::from_secs(30)).unwrap_or_else(|_| panic!("timeout: {what}")) {
                CliEvent::Closed(r) => panic!("closed: {r}"),
                CliEvent::UploadErr { msg, .. } => panic!("upload: {msg}"),
                CliEvent::State(s) if f(&s) => {
                    println!("{:>6} ms OK {what}: фон «{}», сообщение «{}»", t0.elapsed().as_millis(), s.background, s.message);
                    return;
                }
                _ => {}
            }
        };
        match std::env::var("TEST_BG").ok().map(PathBuf::from) {
            Some(path) => {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                c.send(CliReq::UploadBackground(path));
                wait("background set", &|s| s.background == name);
            }
            None => {
                c.send(CliReq::Cmd(Cmd::BackgroundReset));
                wait("background reset", &|s| s.background.is_empty());
            }
        }
    }

    /// Отправить одну команду (JSON) на сервер TEST_PORT: TEST_CMD='{"c":"Clock"}'
    /// cargo test --release e2e_send -- --ignored --nocapture
    #[test]
    #[ignore]
    fn e2e_send() {
        let cmd: Cmd = serde_json::from_str(&std::env::var("TEST_CMD").unwrap()).expect("TEST_CMD — команда в JSON");
        let port = std::env::var("TEST_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(14999);
        let (tx, rx) = channel();
        let c = net::start_client("127.0.0.1".into(), port, "default_password".into(), tx, || {});
        while !matches!(rx.recv_timeout(Duration::from_secs(10)).expect("нет связи"), CliEvent::Connected) {}
        c.send(CliReq::Cmd(cmd));
        // Ждём состояние после команды, чтобы она точно ушла.
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(2) {
            if let Ok(CliEvent::State(s)) = rx.recv_timeout(Duration::from_millis(500)) {
                println!("screen={:?} active={:?} timer={:?}", s.screen, s.active, s.timer);
            }
        }
    }

    /// Команды во время загрузки большого файла: TEST_PDF (≥ 10 стр.), TEST_BIG (большой файл).
    /// cargo test --release e2e_upload_cmd -- --ignored --nocapture
    #[test]
    #[ignore]
    fn e2e_upload_cmd() {
        let pdf = PathBuf::from(std::env::var("TEST_PDF").unwrap());
        let big = PathBuf::from(std::env::var("TEST_BIG").unwrap());
        let (tx, rx) = channel();
        let c = net::start_client("127.0.0.1".into(), 14999, "default_password".into(), tx, || {});
        let next_state = |timeout: Duration| -> Option<StateMsg> {
            let t = Instant::now();
            while let Ok(ev) = rx.recv_timeout(timeout.saturating_sub(t.elapsed())) {
                match ev {
                    CliEvent::State(s) => return Some(s),
                    CliEvent::UploadOk(n) => println!("UPLOADED {n} at {} ms", t.elapsed().as_millis()),
                    _ => {}
                }
            }
            None
        };
        c.send(CliReq::Cmd(Cmd::Clear));
        while next_state(Duration::from_secs(10)).is_some_and(|s| !s.items.is_empty()) {}
        c.send(CliReq::Upload(pdf));
        let id = loop {
            let s = next_state(Duration::from_secs(30)).expect("нет состояния");
            if let Some(i) = s.items.first().filter(|i| i.ready) {
                break i.id;
            }
        };
        c.send(CliReq::Cmd(Cmd::Open { id }));
        let mut page = 0;
        while next_state(Duration::from_secs(10)).is_some_and(|s| s.active != Some(id)) {}
        let started = Instant::now();
        c.send(CliReq::Upload(big));
        let mut worst = 0u128;
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(300));
            let t = Instant::now();
            c.send(CliReq::Cmd(Cmd::Next));
            loop {
                let s = next_state(Duration::from_secs(60)).expect("нет ответа");
                if s.page != page {
                    page = s.page;
                    break;
                }
            }
            let ms = t.elapsed().as_millis();
            worst = worst.max(ms);
            println!("NEXT -> слайд {} за {ms} мс (с начала загрузки {} мс)", page + 1, started.elapsed().as_millis());
        }
        println!("WORST {worst} ms");
        // дождаться конца загрузки (большой файл)
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(120) {
            if let Ok(CliEvent::UploadOk(n)) = rx.recv_timeout(Duration::from_secs(120)) {
                println!("UPLOADED {n} through {} ms after start", started.elapsed().as_millis());
                break;
            }
        }
    }

    /// Превью и таймер против запущенного `presentator --server`:
    /// TEST_FILES="a.pptx;b.pdf" cargo test --release e2e_preview -- --ignored --nocapture
    /// Каждый файл: открыть → миниатюры слайдов 1 и 2 → вперёд → миниатюра слайда 3.
    /// Затем слайд 5 → миниатюры 4, 5, 6. Затем экран таймера: обратный отсчёт 1:35, старт, пауза; экран часов; «Фон».
    #[test]
    #[ignore]
    fn e2e_preview() {
        let files: Vec<PathBuf> = std::env::var("TEST_FILES").unwrap().split(';').map(PathBuf::from).collect();
        let (tx, rx) = channel();
        let t0 = Instant::now();
        let thumbs = std::cell::RefCell::new(HashSet::<(u64, usize)>::new());
        let wait = |what: &str, f: &dyn Fn(&StateMsg, &HashSet<(u64, usize)>) -> bool| -> StateMsg {
            let t = Instant::now();
            loop {
                match rx.recv_timeout(Duration::from_secs(90).saturating_sub(t.elapsed())).unwrap_or_else(|_| panic!("timeout: {what}")) {
                    CliEvent::Closed(r) => panic!("closed: {r}"),
                    CliEvent::Thumb { id, page, image } => {
                        println!("{:>6} ms THUMB id={id} слайд {} {}×{}", t0.elapsed().as_millis(), page + 1, image.size[0], image.size[1]);
                        thumbs.borrow_mut().insert((id, page));
                    }
                    CliEvent::State(s) => {
                        if f(&s, &thumbs.borrow()) {
                            println!("{:>6} ms OK {what}", t0.elapsed().as_millis());
                            return s;
                        }
                    }
                    _ => {}
                }
            }
        };
        let c = net::start_client("127.0.0.1".into(), 14999, "default_password".into(), tx, || {});
        c.send(CliReq::Cmd(Cmd::Clear));
        wait("cleared", &|s, _| s.items.is_empty());
        for f in &files {
            c.send(CliReq::Upload(f.clone()));
        }
        let n = files.len();
        let s = wait("ready", &|s, _| s.items.len() == n && s.items.iter().all(|i| i.ready));
        for it in s.items.clone() {
            c.send(CliReq::Cmd(Cmd::Open { id: it.id }));
            let s = wait("open", &|s, _| s.active == Some(it.id) && s.pages > 2);
            let mut last = s;
            // Состояние после миниатюр может не прийти — ждём миниатюры отдельно, опрашивая очередь.
            let deadline = Instant::now() + Duration::from_secs(60);
            while !(thumbs.borrow().contains(&(it.id, 0)) && thumbs.borrow().contains(&(it.id, 1))) {
                assert!(Instant::now() < deadline, "нет миниатюр слайдов 1–2 для {}", it.name);
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(CliEvent::Thumb { id, page, image }) => {
                        println!("{:>6} ms THUMB id={id} слайд {} {}×{}", t0.elapsed().as_millis(), page + 1, image.size[0], image.size[1]);
                        thumbs.borrow_mut().insert((id, page));
                    }
                    Ok(CliEvent::State(s)) => last = s,
                    _ => {}
                }
            }
            println!("{:>6} ms OK «{}»: миниатюры 1 и 2 (слайд {} из {})", t0.elapsed().as_millis(), it.name, last.page + 1, last.pages);
            c.send(CliReq::Cmd(Cmd::GoTo { page: 1 }));
            wait("goto 2", &|s, _| s.page == 1);
            let deadline = Instant::now() + Duration::from_secs(30);
            while !thumbs.borrow().contains(&(it.id, 2)) {
                assert!(Instant::now() < deadline, "нет миниатюры слайда 3 для {}", it.name);
                if let Ok(CliEvent::Thumb { id, page, .. }) = rx.recv_timeout(Duration::from_millis(500)) {
                    println!("{:>6} ms THUMB id={id} слайд {}", t0.elapsed().as_millis(), page + 1);
                    thumbs.borrow_mut().insert((id, page));
                }
            }
            println!("{:>6} ms OK «{}»: миниатюра 3 после перехода", t0.elapsed().as_millis(), it.name);
            // Переход на слайд 5: нужны предыдущий (4), текущий (5) и следующий (6) — их ещё не присылали.
            c.send(CliReq::Cmd(Cmd::GoTo { page: 4 }));
            wait("goto 5", &|s, _| s.page == 4);
            let deadline = Instant::now() + Duration::from_secs(30);
            while ![3, 4, 5].iter().all(|&p| thumbs.borrow().contains(&(it.id, p))) {
                assert!(Instant::now() < deadline, "нет миниатюр слайдов 4–6 для {}", it.name);
                if let Ok(CliEvent::Thumb { id, page, .. }) = rx.recv_timeout(Duration::from_millis(500)) {
                    println!("{:>6} ms THUMB id={id} слайд {}", t0.elapsed().as_millis(), page + 1);
                    thumbs.borrow_mut().insert((id, page));
                }
            }
            println!("{:>6} ms OK «{}»: миниатюры 4, 5, 6 (предыдущий, текущий, следующий)", t0.elapsed().as_millis(), it.name);
            std::thread::sleep(Duration::from_secs(std::env::var("TEST_HOLD").ok().and_then(|s| s.parse().ok()).unwrap_or(0)));
        }
        // Таймер и часы.
        c.send(CliReq::Cmd(Cmd::Timer));
        wait("timer screen", &|s, _| s.active.is_none() && s.screen == Screen::Timer);
        c.send(CliReq::Cmd(Cmd::TimerSet { countdown: true, secs: 95 }));
        c.send(CliReq::Cmd(Cmd::TimerReset));
        let s = wait("timer 1:35", &|s, _| s.timer.countdown && s.timer.total == 95 && s.timer.elapsed == 0);
        assert_eq!(screen::timer_text(&s.timer, 0).0, "01:35");
        c.send(CliReq::Cmd(Cmd::TimerRun { run: true }));
        wait("timer running", &|s, _| s.timer.running);
        std::thread::sleep(Duration::from_millis(2300));
        c.send(CliReq::Cmd(Cmd::TimerRun { run: false }));
        let s = wait("timer paused", &|s, _| !s.timer.running);
        println!("таймер на паузе: {} мс, на экране {}", s.timer.elapsed, screen::timer_text(&s.timer, 0).0);
        assert!((2200..3000).contains(&s.timer.elapsed));
        std::thread::sleep(Duration::from_secs(std::env::var("TEST_HOLD").ok().and_then(|s| s.parse().ok()).unwrap_or(0)));
        c.send(CliReq::Cmd(Cmd::Clock));
        wait("clock screen", &|s, _| s.screen == Screen::Clock);
        std::thread::sleep(Duration::from_secs(std::env::var("TEST_HOLD").ok().and_then(|s| s.parse().ok()).unwrap_or(0)));
        c.send(CliReq::Cmd(Cmd::Background));
        wait("background", &|s, _| s.screen == Screen::Background);
    }

    /// Видео против запущенного `presentator --server`:
    /// TEST_VIDEO="a.mp4" cargo test e2e_video -- --ignored --nocapture
    /// Загрузка → «готово к запуску» → (TEST_HOLD с) → Старт → ждём конца, на сцене снова фон.
    #[test]
    #[ignore]
    fn e2e_video() {
        let file = PathBuf::from(std::env::var("TEST_VIDEO").unwrap());
        let hold = Duration::from_secs(std::env::var("TEST_HOLD").ok().and_then(|s| s.parse().ok()).unwrap_or(0));
        let (tx, rx) = channel();
        let wait = |what: &str, f: &dyn Fn(&StateMsg) -> bool| {
            let t = Instant::now();
            loop {
                match rx.recv_timeout(Duration::from_secs(60).saturating_sub(t.elapsed())).unwrap_or_else(|_| panic!("timeout: {what}")) {
                    CliEvent::Closed(r) => panic!("closed: {r}"),
                    CliEvent::State(s) => {
                        println!("{:>6} ms {what}: active={:?} video={:?} msg={:?}", t.elapsed().as_millis(), s.active, s.video, s.message);
                        if f(&s) {
                            return;
                        }
                    }
                    _ => {}
                }
            }
        };
        let port = std::env::var("TEST_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(14999);
        let c = net::start_client("127.0.0.1".into(), port, "default_password".into(), tx, || {});
        c.send(CliReq::Cmd(Cmd::Clear)); // список на сервере переживает перезапуск — начинаем с чистого
        wait("cleared", &|s| s.items.is_empty());
        c.send(CliReq::Upload(file));
        let id = std::cell::Cell::new(0);
        wait("ready", &|s| {
            let ok = s.items.len() == 1 && s.items[0].ready;
            if ok {
                id.set(s.items[0].id);
            }
            ok
        });
        let id = id.get();
        c.send(CliReq::Cmd(Cmd::Open { id }));
        wait("armed", &|s| s.video.as_ref().is_some_and(|v| v.state == VideoState::Armed));
        std::thread::sleep(hold);
        c.send(CliReq::Cmd(Cmd::Play));
        wait("playing", &|s| s.video.as_ref().is_some_and(|v| v.state == VideoState::Playing));
        wait("ended", &|s| s.active.is_none());
    }
}
