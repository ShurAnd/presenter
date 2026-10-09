#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod net;
mod ppt;
mod render;
#[cfg(windows)]
mod mpv;
mod video;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use net::{CliEvent, CliReq, ClientHandle, Cmd, ItemInfo, ServerHandle, SrvEvent, StateMsg, VideoInfo, VideoState};
use render::{Out, PageKey, Renderer, VIDEO_EXTS};
use serde::{Deserialize, Serialize};

const BLUE: Color32 = Color32::from_rgb(0x5b, 0xc0, 0xde);
const GREEN: Color32 = Color32::from_rgb(0x28, 0xa7, 0x45);
const RED: Color32 = Color32::from_rgb(0xd9, 0x53, 0x4f);
const GRAY: Color32 = Color32::from_rgb(0x88, 0x88, 0x88);
const OFF: Color32 = Color32::from_rgb(0x6c, 0x75, 0x7d);
const TEAL: Color32 = Color32::from_rgb(0x17, 0xa2, 0xb8);
/// Фон сцены, если картинки фона нет.
const STAGE_BG: Color32 = Color32::from_rgb(0x1f, 0x6f, 0xc8);
/// Файлы фона рядом с exe — если есть, заменяют вшитую картинку. Берётся первый найденный.
const BG_FILES: [&str; 3] = ["background.png", "background.jpg", "background.jpeg"];
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
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("Presentator (Rust, тестовая сборка v{})", env!("CARGO_PKG_VERSION")))
            .with_inner_size([750.0, 620.0])
            .with_min_inner_size([640.0, 420.0])
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native("presentator", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Config {
    ip: String,
    port: String,
    password: String,
    windowed: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { ip: "127.0.0.1".into(), port: "14999".into(), password: "default_password".into(), windowed: false }
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
}

enum ShowKind {
    /// PDF рисуем сами в своём окне.
    Pdf(PathBuf),
    /// PPTX показывает PowerPoint; мы им управляем.
    Ppt(PptShow),
    /// Видео — встроенный проигрыватель поверх сцены.
    Video(VideoShow),
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
        !matches!(&self.kind, ShowKind::Ppt(p) if matches!(p.phase, PptPhase::Lowered(_) | PptPhase::Shown))
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

    // Сцена — полноэкранное окно с фоном, на котором идут показы
    stage: bool,
    /// Что уже отправлено окну сцены: (поверх всех окон, на весь экран).
    stage_sent: Option<(bool, bool)>,
    bg: Option<egui::TextureHandle>,
    bg_checked: bool,
    esc_at: Option<Instant>,

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

    // Интерфейс
    net_panel: bool,
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
        let (cli_tx, cli_rx) = channel();

        // Мост к PowerPoint: события приходят с номером показа (он же поколение `gen`).
        let ppt_host = {
            let (tx, ctx) = (out_tx.clone(), cc.egui_ctx.clone());
            ppt::Host::new(move |gen, ev| {
                let _ = tx.send(Out::Ppt { gen, ev });
                ctx.request_repaint();
            })
        };
        let mut app = Self {
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
            stage: false,
            stage_sent: None,
            bg: None,
            bg_checked: false,
            esc_at: None,
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
            net_panel: std::env::args().any(|a| a == "--net"),
            selected: None,
            net_status: String::new(),
            status: String::new(),
        };
        // Автозапуск: `presentator --server` или `presentator --connect`.
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a == "--server") {
            app.toggle_server(&cc.egui_ctx);
        } else if args.iter().any(|a| a == "--connect") {
            app.toggle_client(&cc.egui_ctx);
        }
        app
    }

    fn save_cfg(&self) {
        if let Ok(s) = serde_json::to_string_pretty(&self.cfg) {
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
                let (pages, pdf, src, name, is_video) = (it.pages, it.pdf.clone(), it.src.clone(), it.name.clone(), it.kind == "ВИДЕО");
                // Прежний показ закрываем: на экране остаётся сцена с фоном,
                // новый показ появится на ней, только когда будет готов.
                self.close_show();
                self.stage = true;
                self.show_gen += 1;
                let gen = self.show_gen;
                let kind = match pdf {
                    Some(pdf) => ShowKind::Pdf(pdf),
                    // Видео сразу загружается за фоном и ждёт пробела или «Старт».
                    None if is_video => {
                        let (tx, ctx) = (self.out_tx.clone(), self.ctx.clone());
                        let player = video::Video::start(&src, self.cfg.windowed, move |ev| {
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
                        if let Err(e) = self.ppt_host.open(gen, &src, self.cfg.windowed) {
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
            Cmd::Background => {
                self.close_show();
                self.stage = true;
            }
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
                ShowKind::Video(_) => {}
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
                    self.ppt_host.reveal(ps.hwnd);
                    ps.phase = PptPhase::Shown;
                } else {
                    ctx.request_repaint_after(Duration::from_millis(30));
                }
            }
            PptPhase::Shown => {}
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

    /// Удаляем только то, что сами создали в рабочей папке (загруженное и сконвертированное).
    fn delete_files(&self, it: &Item) {
        for p in [Some(&it.src), it.pdf.as_ref()].into_iter().flatten() {
            if p.starts_with(&self.work_dir) {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    fn add_local(&mut self, path: PathBuf, name: Option<String>, ctx: &egui::Context) {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let kind = match ext.as_str() {
            "pdf" => "PDF",
            "pptx" | "ppt" | "ppsx" | "pps" => "PPTX",
            e if VIDEO_EXTS.contains(&e) => "ВИДЕО",
            _ => {
                self.status = format!("Формат .{ext} не поддерживается (нужен PDF, PPTX или видео)");
                return;
            }
        };
        let id = self.next_id;
        self.next_id += 1;
        let name = name.unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        self.items.push(Item { id, name, kind: kind.into(), src: path.clone(), pdf: None, pages: 0, status: Status::Preparing });
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
                    c.send(CliReq::Upload(p));
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
                        self.dirty = true;
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
                                    s.page = cur.saturating_sub(1);
                                    s.pages = total.max(1);
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
                        video::VideoEvent::Ended => self.close_show(),
                        video::VideoEvent::Error(e) => {
                            self.close_show();
                            self.status = format!("Видео: {e}");
                        }
                    }
                    self.dirty = true;
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
            }
        }

        while let Ok(ev) = self.srv_rx.try_recv() {
            match ev {
                SrvEvent::Started => {
                    self.server_online = true;
                    self.stage = true; // сервер стоит у видеостены — сразу выводим фон
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
                CliEvent::Connected => {
                    self.connected = true;
                    self.close_show();
                    self.net_status = format!("Подключено к {}:{}", self.cfg.ip, self.cfg.port);
                    self.status.clear();
                }
                CliEvent::Closed(reason) => {
                    self.connected = false;
                    self.client = None;
                    self.remote = StateMsg::default();
                    self.net_status = if reason.is_empty() { "Отключено".into() } else { reason };
                }
                CliEvent::State(s) => {
                    if s.message != self.remote.message {
                        self.status = s.message.clone();
                    }
                    self.remote = s;
                }
                CliEvent::Progress { name, sent, total } => {
                    let pct = if total > 0 { sent * 100 / total } else { 100 };
                    self.status = format!("Отправка «{name}»: {pct}%");
                }
                CliEvent::UploadOk(name) => self.status = format!("Файл «{name}» передан на сервер"),
                CliEvent::UploadErr { name, msg } => self.status = format!("Не удалось передать «{name}»: {msg}"),
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
        if self.client.take().is_some() {
            self.connected = false;
            self.remote = StateMsg::default();
        }
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
            let label = if self.net_panel { "📄 Список" } else { "🌐 Сеть" };
            if btn(ui, label, BLUE).clicked() {
                self.net_panel = !self.net_panel;
            }
            if btn(ui, "➕ Добавить", BLUE).clicked() {
                if let Some(files) = rfd::FileDialog::new()
                    .add_filter("Презентации и видео", &[&["pdf", "pptx", "ppt", "ppsx", "pps"][..], &VIDEO_EXTS[..]].concat())
                    .pick_files()
                {
                    let ctx = ui.ctx().clone();
                    self.add_files(files, &ctx);
                }
            }
            ui.add_space(4.0);
            let srv = if self.server_online { format!("Сервер: онлайн ({})", self.clients) } else { "Сервер: офлайн".into() };
            indicator(ui, if self.server_online { GREEN } else { OFF }, &srv);
            let cli = if self.connected { "Клиент: подключен" } else { "Клиент: не подключен" };
            indicator(ui, if self.connected { TEAL } else { OFF }, cli);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if btn(ui, "🗑 Очистить", RED).clicked() {
                    self.act(Cmd::Clear);
                }
                let sel = self.selected.filter(|id| view.items.iter().any(|i| i.id == *id));
                if ui.add_enabled(sel.is_some(), cbutton("⬇ Вниз", GRAY)).clicked() {
                    self.act(Cmd::Move { id: sel.unwrap(), up: false });
                }
                if ui.add_enabled(sel.is_some(), cbutton("⬆ Вверх", GRAY)).clicked() {
                    self.act(Cmd::Move { id: sel.unwrap(), up: true });
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
            if btn(ui, "Фон", GRAY).on_hover_text("Закрыть показ и вывести на экран сервера фон").clicked() {
                self.act(Cmd::Background);
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
                        // Номер, набранный с клавиатуры, — подсказка оператору (на стене его не видно).
                        ui.label(RichText::new(format!("К слайду {} — Enter", self.goto_buf)).strong().size(15.0).color(GREEN));
                    } else {
                        ui.label(RichText::new(format!("Слайд {} из {}", view.page + 1, view.pages)).strong().size(15.0));
                    }
                } else {
                    ui.label(RichText::new("Показ не запущен").color(GRAY));
                }
            });
        });
        let hint = if self.connected {
            "Вы управляете сервером. Пробел — следующий шаг или старт/пауза видео, стрелки — слайды или перемотка видео."
        } else {
            "Двойной щелчок открывает файл. Пробел — следующий шаг или старт/пауза видео. Двойной Esc на экране показа закрывает его, на фоне — убирает фон."
        };
        let text = if self.status.is_empty() { hint } else { self.status.as_str() }.to_owned();
        let status = |ui: &mut egui::Ui| {
            ui.add(egui::Label::new(RichText::new(&text).size(11.5).color(Color32::from_gray(0x66))).truncate());
        };
        if !(active && view.video.is_none()) {
            status(ui);
            return;
        }
        // Слева строка состояния, справа переход к слайду по номеру (Enter в поле тоже работает).
        egui::Sides::new().show(
            ui,
            status,
            |ui| {
                let go = btn(ui, "Перейти", BLUE).on_hover_text("Перейти к слайду с этим номером");
                let field = ui.add(egui::TextEdit::singleline(&mut self.goto_field).hint_text("№").desired_width(44.0));
                ui.label("Слайд №");
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                self.goto_field.retain(|c| c.is_ascii_digit());
                if go.clicked() || enter {
                    let n = std::mem::take(&mut self.goto_field);
                    self.goto_number(&n, view);
                }
            },
        );
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
                        } else if it.pages == 0 {
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
            ui.add_space(8.0);
            ui.checkbox(&mut self.cfg.windowed, "Сцена и показ в окне, а не на весь экран (только для теста на одном компьютере)");
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "💡 На компьютере с видеостеной нажмите «Запустить сервер».\n\
                     На управляющем компьютере введите IP сервера и нажмите «Подключиться».\n\
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
        for cmd in nav_keys(ctx, view) {
            self.act(cmd);
        }
        let (digits, enter) = typed_digits(ctx);
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
            self.bg_checked = true;
            self.bg = load_background(ctx);
        }
        let pdf = match &self.show {
            Some(s) => match &s.kind {
                ShowKind::Pdf(pdf) => Some((s.id, s.page, s.pages, pdf.clone())),
                ShowKind::Ppt(_) | ShowKind::Video(_) => None,
            },
            None => None,
        };
        // Сразу после открытия какое-то время виден только фон, даже если страница уже готова.
        let hold = self.show.as_ref().map_or(Duration::ZERO, |s| SWITCH_BG.saturating_sub(s.opened.elapsed()));
        if !hold.is_zero() {
            ctx.request_repaint_after(hold);
        }
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
        // Флаги builder'а у нового окна eframe на Windows не применяются — шлём команды явно.
        let sent = self.stage_sent;
        let (textures, last_tex, error, bg, failed) = (&self.textures, &mut self.last_tex, &self.render_error, &self.bg, &self.failed);

        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("stage"), builder, |ctx, _| {
            if sent.is_none_or(|(_, fs)| fs == windowed) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(windowed));
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
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
                    let key = PageKey { id, page, w, h };
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
                        }
                        None => {
                            ui.painter().rect_filled(rect, 0.0, STAGE_BG);
                        }
                    },
                }
                if let (Some(e), Some(_)) = (error, &pdf) {
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, e, egui::FontId::proportional(20.0), Color32::LIGHT_RED);
                }
            });
        });
        // Уровень окна сцены дублируем напрямую через Windows: при смене, а пока PowerPoint
        // готовит показ за сценой — каждый кадр (его окно не должно вылезти поверх фона).
        let ppt_loading = self.show.as_ref().is_some_and(|s| matches!(&s.kind, ShowKind::Ppt(p) if matches!(p.phase, PptPhase::Loading | PptPhase::Ready(_))));
        if !windowed && (sent.is_none_or(|(t, _)| t != top) || ppt_loading) {
            ppt::stage_topmost(top);
        }
        if ppt_loading {
            ctx.request_repaint_after(Duration::from_millis(15));
        }
        self.stage_sent = Some((top, !windowed));

        if let Some((_, _, _, pdf)) = pdf {
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
        }
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
        } else {
            self.stage = false;
        }
    }
}

/// Картинка фона: background.png / .jpg рядом с exe, иначе вшитая assets/background.png. Не удалось — None, сцена будет залита цветом.
fn load_background(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    // Файл рядом с exe заменяет вшитую картинку (можно поменять фон без пересборки).
    let file = BG_FILES.iter().map(|n| app_dir().join(n)).find(|p| p.is_file()).and_then(|p| image::open(p).ok());
    let img = file.or_else(|| image::load_from_memory(include_bytes!("../assets/background.png")).ok())?;
    let max = ctx.input(|i| i.max_texture_side).min(8192) as u32;
    let img = if img.width() > max || img.height() > max { img.resize(max, max, image::imageops::FilterType::Triangle) } else { img };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ctx.load_texture("background", egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()), egui::TextureOptions::LINEAR))
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
        self.poll(ctx);

        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() {
            self.add_files(dropped, ctx);
        }

        let view = self.view();
        self.keys(ctx, &view);

        let margin = egui::Frame::new().fill(Color32::from_gray(0xfa)).inner_margin(6.0);
        egui::TopBottomPanel::top("top").frame(margin).show(ctx, |ui| self.top_bar(ui, &view));
        egui::TopBottomPanel::bottom("bottom").frame(margin).show(ctx, |ui| self.bottom_bar(ui, &view));
        egui::CentralPanel::default().frame(margin).show(ctx, |ui| {
            if self.net_panel {
                self.network_panel(ui);
            } else {
                self.list(ui, &view);
            }
        });

        self.tick_ppt(ctx);
        ppt::grab_esc(self.show.as_ref().is_some_and(|s| matches!(s.kind, ShowKind::Ppt(_))));
        if !self.connected {
            self.stage_window(ctx);
        }

        if self.dirty {
            self.dirty = false;
            if let Some(s) = &self.server {
                s.publish(&self.local_state());
            }
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_cfg();
        // Убираем за собой загруженные и сконвертированные файлы.
        for it in &self.items {
            self.delete_files(it);
        }
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
        for f in &files { c.send(CliReq::Upload(f.clone())); }
        let n = files.len();
        wait("ready", &|e| matches!(e, CliEvent::State(s) if s.items.len() == n && s.items.iter().all(|i| i.ready)));
        let step: u64 = std::env::var("TEST_OPEN").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
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
        if let Some(next) = std::env::var("TEST_SWITCH").ok().and_then(|s| s.parse::<u64>().ok()) {
            c.send(CliReq::Cmd(Cmd::Open { id: next }));
            wait("switch", &|e| matches!(e, CliEvent::State(s) if s.active == Some(next)));
            std::thread::sleep(hold);
            c.send(CliReq::Cmd(Cmd::Close));
            wait("close", &|e| matches!(e, CliEvent::State(s) if s.active.is_none()));
            std::thread::sleep(hold);
        }
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
        let c = net::start_client("127.0.0.1".into(), 14999, "default_password".into(), tx, || {});
        for f in &files {
            c.send(CliReq::Upload(f.clone()));
        }
        let n = files.len();
        wait("ready", &|s| s.items.len() == n && s.items.iter().all(|i| i.ready));
        // TEST_LAST_HOLD — сколько держать последнюю презентацию (для проверки после быстрых переключений).
        let last_hold = std::env::var("TEST_LAST_HOLD").ok().and_then(|s| s.parse().ok()).map(Duration::from_secs_f64);
        let count = seq.len();
        for (k, id) in seq.into_iter().enumerate() {
            println!("OPEN {id} {}", now());
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
        let c = net::start_client("127.0.0.1".into(), 14999, "default_password".into(), tx, || {});
        c.send(CliReq::Upload(file));
        wait("ready", &|s| s.items.len() == 1 && s.items[0].ready);
        let id = 1;
        c.send(CliReq::Cmd(Cmd::Open { id }));
        wait("armed", &|s| s.video.as_ref().is_some_and(|v| v.state == VideoState::Armed));
        std::thread::sleep(hold);
        c.send(CliReq::Cmd(Cmd::Play));
        wait("playing", &|s| s.video.as_ref().is_some_and(|v| v.state == VideoState::Playing));
        wait("ended", &|s| s.active.is_none());
    }
}
