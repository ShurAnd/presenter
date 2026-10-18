#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod audio;
mod background;
mod commands;
mod config;
mod display;
mod health;
mod journal;
#[cfg(windows)]
mod mpv;
mod net;
mod network;
mod player;
mod ppt;
mod registry;
mod render;
mod screen;
mod stage;
mod ui;
mod ui_net;
mod ui_player;
mod ui_preview;
mod ui_settings;
mod video;
mod watchdog;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use net::{MediaAction, MediaInfo, AudioInfo, AudioState, CliEvent, CliReq, ClientHandle, Cmd, ItemInfo, Screen, ServerHandle, SrvEvent, StateMsg, VideoInfo, VideoState};
use render::{Out, PageKey, Renderer, AUDIO_EXTS, FULL_VIEW, THUMB_H, THUMB_W, VIDEO_EXTS, ZOOMS};
use serde::{Deserialize, Serialize};

// Общие помощники модулей интерфейса и настроек — видны во всех модулях программы.
use config::*;
use ui::*;
use ui_preview::*;

const BLUE: Color32 = Color32::from_rgb(0x5b, 0xc0, 0xde);
const GREEN: Color32 = Color32::from_rgb(0x28, 0xa7, 0x45);
const RED: Color32 = Color32::from_rgb(0xd9, 0x53, 0x4f);
const GRAY: Color32 = Color32::from_rgb(0x88, 0x88, 0x88);
const OFF: Color32 = Color32::from_rgb(0x6c, 0x75, 0x7d);
/// Вставка (фанфары) в плеере.
const INSERT: Color32 = Color32::from_rgb(0xe0, 0x8a, 0x00);
/// Рамка видимой части увеличенного слайда на миниатюре.
const ZOOM_FRAME: Color32 = Color32::from_rgb(0xf0, 0x8c, 0x00);
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
/// С «Показать курсор» курсор на сцене виден столько после последнего движения мыши.
const CURSOR_IDLE: Duration = Duration::from_secs(3);
const DOUBLE_ESC: Duration = Duration::from_millis(600);

fn main() -> eframe::Result {
    // `--watchdog`: окна нет — сторож запускает сервер (остальные ключи) и перезапускает его при сбое.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--watchdog") {
        watchdog::run(args.into_iter().filter(|a| a != "--watchdog").collect());
        return Ok(());
    }
    journal::init();
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

/// Вкладки главного окна.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    List,
    Player,
    Net,
    Settings,
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
    /// Видео: повтор по кругу.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    repeat: bool,
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

/// Пока работает сервер или на экране сцена — не давать Windows гасить экран и засыпать
/// (иначе посреди мероприятия экран показа мог погаснуть по таймеру энергосбережения).
fn keep_awake(on: bool) {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn SetThreadExecutionState(flags: u32) -> u32;
        }
        const ES_CONTINUOUS: u32 = 0x8000_0000;
        const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
        const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;
        unsafe {
            SetThreadExecutionState(if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED } else { ES_CONTINUOUS });
        }
    }
    #[cfg(not(windows))]
    let _ = on;
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
    /// Видео: по окончании начинать заново (значок повтора в строке списка).
    repeat: bool,
    /// PPTX: видео и звук, вставленные ссылкой на файлы, которых нет на этом компьютере
    /// («файл (слайд N)») — на экране их не будет.
    linked: Vec<String>,
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
    /// Увеличение: слайд крупной картинкой (номер слайда с нуля) — сцена показывает её поверх
    /// слайд-шоу; какой слайд сейчас готовится; была ли сцена с увеличением наверху.
    zoom_img: Option<(usize, egui::TextureHandle)>,
    zoom_busy: Option<usize>,
    zoom_up: bool,
    /// Видео и звук на слайде (номер слайда с единицы) — по сообщению моста.
    media: (usize, Vec<ppt::Media>),
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
            ShowKind::Ppt(p) => !matches!(p.phase, PptPhase::Lowered(_) | PptPhase::Shown) || self.ppt_zoom_on(),
            // Идущее видео — наверху, сцена прямо под ним (через «поверх всех» видео не поднять).
            ShowKind::Video(v) => !matches!(v.state, VideoState::Playing | VideoState::Paused),
            ShowKind::Pdf(_) | ShowKind::Image(_) => true,
        }
    }

    /// PowerPoint увеличен и крупная картинка этого слайда готова — сцена показывает её поверх слайд-шоу.
    fn ppt_zoom_on(&self) -> bool {
        match &self.kind {
            ShowKind::Ppt(p) => p.phase == PptPhase::Shown && self.zoom > 1.0 && p.zoom_img.as_ref().is_some_and(|(pg, _)| *pg == self.page),
            _ => false,
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
    /// Увеличение слайда (1 — весь слайд) и центр видимой части в долях страницы.
    zoom: f32,
    center: (f32, f32),
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
    /// Видео на слайде, над строкой которого мышь в списке, — его рамка на миниатюре ярче.
    media_hover: Option<usize>,
    /// «Показать курсор» на экране показа (после перезапуска — снова выключено) и когда мышь
    /// последний раз двигали над сценой (курсор виден 3 с после движения).
    show_cursor: bool,
    cursor_moved: Option<Instant>,
    /// Размер сцены в пикселях и пропорции (ширина / высота) текущего слайда — для увеличения.
    stage_px: (f32, f32),
    slide_aspect: f32,
    /// Окно «Резервный сервер»: вводим сервер, которого ещё нет в списке.
    reserve_form: Option<SavedServer>,

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
    /// Что последним записано в журнал из строки состояния и сетевого статуса (пишем только перемены),
    /// уровень состояния сервера, не даём ли системе уснуть.
    logged_status: String,
    logged_net: String,
    logged_level: Option<health::Level>,
    awake: bool,
    /// Ждёт подтверждения: текст вопроса, надпись на кнопке и команда (удалить, очистить).
    confirm: Option<(String, &'static str, Cmd)>,
    /// Состояние от сервера уже пришло (до этого версия сервера неизвестна); полосу о версии скрыли.
    got_state: bool,
    version_hidden: bool,
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
            media_hover: None,
            show_cursor: false,
            cursor_moved: None,
            stage_px: (1920.0, 1080.0),
            slide_aspect: 16.0 / 9.0,
            reserve_form: None,
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
            tab: if std::env::args().any(|a| a == "--net") { Tab::Net } else { Tab::List },
            logged_status: String::new(),
            logged_net: String::new(),
            logged_level: None,
            awake: false,
            confirm: None,
            got_state: false,
            version_hidden: false,
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

}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui_frames.fetch_add(1, Ordering::SeqCst);
        self.poll(ctx);
        self.journal_tick();
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
        self.version_bar(ctx);
        self.health_window(ctx);
        self.confirm_window(ctx);
        self.help_window(ctx);
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
                Tab::Net => self.network_panel(ui),
                Tab::Settings => self.settings_panel(ui),
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
        jlog!("Выход из программы");
        keep_awake(false);
        // Показ PowerPoint закрываем сами, пока процесс жив: иначе скрытый PowerPoint останется с файлом.
        self.ppt_host.shutdown();
        self.save_cfg();
        // Список и присланные файлы остаются — при следующем запуске список восстановится.
        self.save_list();
    }
}

#[cfg(test)]
mod tests;
