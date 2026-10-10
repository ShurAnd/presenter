//! Сетевой слой: типизированный протокол поверх WebSocket, сервер и клиент.
//! С интерфейсом общается только через каналы — никаких вызовов GUI из сетевых потоков.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc as tmpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;

use crate::health::HealthInfo;

pub const CHUNK: usize = 256 * 1024;

/// Команды управления. Одинаковы для локального управления и для сети.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "c")]
pub enum Cmd {
    Open { id: u64 },
    Next,
    Prev,
    GoTo { page: usize },
    Close,
    Remove { id: u64 },
    Move { id: u64, up: bool },
    Clear,
    /// Закрыть показ (если есть) и вывести на сервере сцену с фоном.
    Background,
    /// Пробел или «Старт»: видео — запуск, затем пауза/продолжить; презентация — следующий шаг.
    Play,
    /// Закрыть показ и вывести на сервере экран часов.
    Clock,
    /// Закрыть показ и вывести на сервере экран таймера.
    Timer,
    /// Режим таймера (обратный отсчёт на `secs` секунд или секундомер).
    TimerSet { countdown: bool, secs: u32 },
    /// Запустить или остановить таймер.
    TimerRun { run: bool },
    /// Таймер — на начало (и остановить).
    TimerReset,
    /// Плеер: играть трек (музыка; вставка, если играла, прекращается).
    AudioPlay { id: u64 },
    /// Плеер: сыграть трек один раз «вставкой» — музыка на это время на паузе, потом продолжается.
    AudioInsert { id: u64 },
    /// Плеер: пауза / продолжить; если ничего не играет — старт.
    AudioToggle,
    /// Плеер: стоп с плавным затуханием; повторный «Стоп» во время затухания — сразу.
    AudioStop,
    AudioNext,
    AudioPrev,
    /// Перемотка того, что сейчас звучит, на `secs` секунд от начала.
    AudioSeek { secs: u32 },
    /// Режим: `list` — играть по списку (иначе один трек), `repeat` — по кругу.
    AudioMode { list: bool, repeat: bool },
    /// Громкость, 0–100.
    AudioVolume { volume: u8 },
    AudioRemove { id: u64 },
    AudioMove { id: u64, up: bool },
    AudioClear,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub enum AudioState {
    #[default]
    Stopped,
    Playing,
    Paused,
    /// Идёт плавное затухание после «Стоп».
    Stopping,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct TrackInfo {
    pub id: u64,
    pub name: String,
}

/// Состояние плеера на сервере.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct AudioInfo {
    pub tracks: Vec<TrackInfo>,
    /// Трек «музыки» (выбран для игры), его состояние, позиция и длительность в секундах.
    pub current: Option<u64>,
    pub state: AudioState,
    pub pos: u32,
    pub dur: u32,
    /// Вставка, пока она звучит (музыка на это время на паузе).
    pub insert: Option<u64>,
    pub insert_state: AudioState,
    pub insert_pos: u32,
    pub insert_dur: u32,
    pub list: bool,
    pub repeat: bool,
    pub volume: u8,
}

impl Default for AudioInfo {
    fn default() -> Self {
        Self {
            tracks: Vec::new(),
            current: None,
            state: AudioState::Stopped,
            pos: 0,
            dur: 0,
            insert: None,
            insert_state: AudioState::Stopped,
            insert_pos: 0,
            insert_dur: 0,
            list: true,
            repeat: false,
            volume: 100,
        }
    }
}

/// Что на сцене сервера, когда показа нет.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub enum Screen {
    #[default]
    Background,
    Clock,
    Timer,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct TimerInfo {
    /// Обратный отсчёт; иначе секундомер.
    pub countdown: bool,
    /// Длительность обратного отсчёта, с.
    pub total: u32,
    /// Сколько прошло на момент отправки состояния, мс. Пока таймер идёт, клиент досчитывает сам.
    pub elapsed: u64,
    pub running: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum VideoState {
    /// Видео загружается.
    Loading,
    /// Загружено и стоит на паузе за фоном — запустится сразу по пробелу или «Старт».
    Armed,
    Playing,
    Paused,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct VideoInfo {
    pub state: VideoState,
    /// Позиция и длительность в секундах.
    pub pos: u32,
    pub dur: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ItemInfo {
    pub id: u64,
    pub name: String,
    pub kind: String,
    pub pages: usize,
    pub ready: bool,
    /// Пусто, если всё в порядке; иначе текст состояния или ошибки.
    pub status: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct StateMsg {
    pub items: Vec<ItemInfo>,
    pub active: Option<u64>,
    pub page: usize,
    pub pages: usize,
    /// Сообщение сервера для оператора (например, ошибка PowerPoint).
    #[serde(default)]
    pub message: String,
    /// Состояние видео, если сейчас открыто видео.
    #[serde(default)]
    pub video: Option<VideoInfo>,
    /// Экран сцены без показа: фон, часы или таймер.
    #[serde(default)]
    pub screen: Screen,
    #[serde(default)]
    pub timer: TimerInfo,
    #[serde(default)]
    pub audio: AudioInfo,
}

/// Миниатюры текущего и следующего слайда: (элемент, страница с нуля, JPEG).
pub type Thumbs = Arc<Vec<(u64, usize, Arc<Vec<u8>>)>>;

/// Миниатюра уходит клиенту бинарным кадром: id (8 байт LE), страница (4 байта LE), JPEG.
fn thumb_frame(id: u64, page: usize, jpg: &[u8]) -> Message {
    let mut b = Vec::with_capacity(12 + jpg.len());
    b.extend_from_slice(&id.to_le_bytes());
    b.extend_from_slice(&(page as u32).to_le_bytes());
    b.extend_from_slice(jpg);
    Message::binary(b)
}

fn parse_thumb(b: &[u8]) -> Option<(u64, usize, &[u8])> {
    let id = u64::from_le_bytes(b.get(..8)?.try_into().ok()?);
    let page = u32::from_le_bytes(b.get(8..12)?.try_into().ok()?) as usize;
    Some((id, page, &b[12..]))
}

/// Картинка (JPEG, PNG) → изображение для текстуры egui.
pub fn decode_image(data: &[u8]) -> Option<eframe::egui::ColorImage> {
    let img = image::load_from_memory(data).ok()?.to_rgba8();
    Some(eframe::egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "t")]
enum C2S {
    /// `upload` — соединение только для загрузки файлов: не считается клиентом, состояние по нему не шлём.
    Hello {
        password: String,
        #[serde(default)]
        upload: bool,
    },
    Cmd { cmd: Cmd },
    UploadBegin { name: String, size: u64 },
    UploadEnd,
    /// Замер задержки: сервер сразу отвечает `Pong` с тем же `ts`.
    Ping { ts: u64 },
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "t")]
enum S2C {
    Welcome,
    Denied { reason: String },
    State { state: StateMsg },
    UploadOk { name: String },
    UploadErr { name: String, msg: String },
    Pong { ts: u64 },
    /// Показатели компьютера-сервера (раз в 2 с).
    Health { health: HealthInfo },
}

fn js<T: Serialize>(v: &T) -> Message {
    Message::text(serde_json::to_string(v).unwrap_or_default())
}

// ---------------------------------------------------------------- сервер

pub enum SrvEvent {
    Started,
    Failed(String),
    Clients(usize),
    Cmd(Cmd),
    File { name: String, path: PathBuf },
}

pub struct ServerHandle {
    state_tx: watch::Sender<String>,
    health_tx: Arc<watch::Sender<String>>,
    thumbs_tx: watch::Sender<Thumbs>,
    _stop: oneshot::Sender<()>,
}

impl ServerHandle {
    pub fn publish(&self, state: &StateMsg) {
        let s = serde_json::to_string(&S2C::State { state: state.clone() }).unwrap_or_default();
        // Рассылаем только реальные изменения.
        self.state_tx.send_if_modified(|cur| {
            let changed = *cur != s;
            if changed {
                *cur = s;
            }
            changed
        });
    }

    /// Миниатюры, нужные клиентам сейчас (текущий и следующий слайд). Рассылка — только при смене набора.
    pub fn publish_thumbs(&self, thumbs: Vec<(u64, usize, Arc<Vec<u8>>)>) {
        self.thumbs_tx.send_if_modified(|cur| {
            let same = cur.len() == thumbs.len() && cur.iter().zip(&thumbs).all(|(a, b)| (a.0, a.1) == (b.0, b.1));
            if !same {
                *cur = Arc::new(thumbs);
            }
            !same
        });
    }

    /// Функция рассылки показателей клиентам. Её вызывает поток сбора напрямую, а не интерфейс:
    /// если интерфейс сервера зависнет, клиенты всё равно увидят показатели и «программа не отвечает».
    pub fn health_publisher(&self) -> impl Fn(&HealthInfo) + Send + 'static {
        let tx = self.health_tx.clone();
        move |health: &HealthInfo| {
            let s = serde_json::to_string(&S2C::Health { health: health.clone() }).unwrap_or_default();
            tx.send_replace(s);
        }
    }
}

pub fn start_server(
    port: u16,
    password: String,
    dir: PathBuf,
    ev: Sender<SrvEvent>,
    wake: impl Fn() + Send + Sync + 'static,
) -> ServerHandle {
    let (state_tx, state_rx) = watch::channel(String::new());
    let (health_tx, health_rx) = watch::channel(String::new());
    let (thumbs_tx, thumbs_rx) = watch::channel(Thumbs::default());
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = ev.send(SrvEvent::Failed(e.to_string()));
                wake();
                return;
            }
        };
        rt.block_on(async move {
            let listener = match TcpListener::bind(("0.0.0.0", port)).await {
                Ok(l) => l,
                Err(e) => {
                    let _ = ev.send(SrvEvent::Failed(format!("порт {port}: {e}")));
                    wake();
                    return;
                }
            };
            let _ = ev.send(SrvEvent::Started);
            wake();
            let clients = Arc::new(AtomicUsize::new(0));
            let accept = async {
                loop {
                    let Ok((stream, _)) = listener.accept().await else { continue };
                    let (ev, wake, clients) = (ev.clone(), wake.clone(), clients.clone());
                    let (password, dir, state_rx, health_rx) = (password.clone(), dir.clone(), state_rx.clone(), health_rx.clone());
                    let thumbs_rx = thumbs_rx.clone();
                    tokio::spawn(async move {
                        serve_client(stream, password, dir, state_rx, health_rx, thumbs_rx, &ev, &wake, &clients).await;
                    });
                }
            };
            tokio::select! { _ = accept => {}, _ = stop_rx => {} }
        });
        // Runtime уничтожается здесь — все соединения закрываются.
    });
    ServerHandle { state_tx, health_tx: Arc::new(health_tx), thumbs_tx, _stop: stop_tx }
}

fn safe_name(name: &str) -> String {
    let n: String = name
        .chars()
        .map(|c| if c.is_control() || "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let n = n.trim().trim_matches('.').to_string();
    if n.is_empty() { "file".into() } else { n }
}

pub fn unique_prefix() -> String {
    static N: AtomicUsize = AtomicUsize::new(0);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{ms}_{}", N.fetch_add(1, Ordering::Relaxed))
}

async fn serve_client(
    stream: TcpStream,
    password: String,
    dir: PathBuf,
    mut state_rx: watch::Receiver<String>,
    mut health_rx: watch::Receiver<String>,
    mut thumbs_rx: watch::Receiver<Thumbs>,
    ev: &Sender<SrvEvent>,
    wake: &Arc<dyn Fn() + Send + Sync>,
    clients: &AtomicUsize,
) {
    let _ = stream.set_nodelay(true);
    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else { return };

    // Авторизация: первое сообщение обязано быть Hello с верным паролем.
    let hello = tokio::time::timeout(Duration::from_secs(5), ws.next()).await;
    let (ok, upload_only) = match hello {
        Ok(Some(Ok(Message::Text(t)))) => match serde_json::from_str::<C2S>(t.as_str()) {
            Ok(C2S::Hello { password: p, upload }) => (p == password, upload),
            _ => (false, false),
        },
        _ => (false, false),
    };
    if !ok {
        let _ = ws.send(js(&S2C::Denied { reason: "Неверный пароль".into() })).await;
        let _ = ws.close(None).await;
        return;
    }
    if ws.send(js(&S2C::Welcome)).await.is_err() {
        return;
    }
    if !upload_only {
        let _ = ev.send(SrvEvent::Clients(clients.fetch_add(1, Ordering::SeqCst) + 1));
        wake();
        let first = state_rx.borrow_and_update().clone();
        if !first.is_empty() {
            let _ = ws.send(Message::text(first)).await;
        }
        thumbs_rx.mark_changed(); // новому клиенту — миниатюры, которые уже есть
    }

    // Текущая загрузка: (имя, путь, файл, ожидаемый размер, принято)
    let mut upload: Option<(String, PathBuf, tokio::fs::File, u64, u64)> = None;
    // Миниатюры из прошлой рассылки — их клиенту уже отправили.
    let mut sent_thumbs: Vec<(u64, usize)> = Vec::new();

    loop {
        tokio::select! {
            changed = thumbs_rx.changed(), if !upload_only => {
                if changed.is_err() { break; }
                let set = thumbs_rx.borrow_and_update().clone();
                let mut ok = true;
                for (id, page, jpg) in set.iter().filter(|t| !sent_thumbs.contains(&(t.0, t.1))) {
                    if ws.send(thumb_frame(*id, *page, jpg)).await.is_err() { ok = false; break; }
                }
                if !ok { break; }
                sent_thumbs = set.iter().map(|t| (t.0, t.1)).collect();
            }
            changed = state_rx.changed(), if !upload_only => {
                if changed.is_err() { break; }
                let s = state_rx.borrow_and_update().clone();
                if ws.send(Message::text(s)).await.is_err() { break; }
            }
            changed = health_rx.changed(), if !upload_only => {
                if changed.is_err() { break; }
                let s = health_rx.borrow_and_update().clone();
                if !s.is_empty() && ws.send(Message::text(s)).await.is_err() { break; }
            }
            msg = ws.next() => {
                let Some(Ok(msg)) = msg else { break };
                match msg {
                    Message::Text(t) => match serde_json::from_str::<C2S>(t.as_str()) {
                        Ok(C2S::Cmd { cmd }) => { let _ = ev.send(SrvEvent::Cmd(cmd)); wake(); }
                        Ok(C2S::Ping { ts }) => {
                            if ws.send(js(&S2C::Pong { ts })).await.is_err() { break; }
                        }
                        Ok(C2S::UploadBegin { name, size }) => {
                            let name = safe_name(&name);
                            let path = dir.join(format!("{}_{}", unique_prefix(), name));
                            let _ = tokio::fs::create_dir_all(&dir).await;
                            match tokio::fs::File::create(&path).await {
                                Ok(f) => upload = Some((name, path, f, size, 0)),
                                Err(e) => {
                                    let _ = ws.send(js(&S2C::UploadErr { name, msg: e.to_string() })).await;
                                }
                            }
                        }
                        Ok(C2S::UploadEnd) => {
                            if let Some((name, path, mut f, size, got)) = upload.take() {
                                let _ = f.flush().await;
                                drop(f);
                                if got == size {
                                    let _ = ev.send(SrvEvent::File { name: name.clone(), path });
                                    wake();
                                    let _ = ws.send(js(&S2C::UploadOk { name })).await;
                                } else {
                                    let _ = tokio::fs::remove_file(&path).await;
                                    let msg = format!("получено {got} из {size} байт");
                                    let _ = ws.send(js(&S2C::UploadErr { name, msg })).await;
                                }
                            }
                        }
                        _ => {}
                    },
                    Message::Binary(b) => {
                        if let Some((_, _, f, _, got)) = upload.as_mut() {
                            if f.write_all(&b).await.is_ok() { *got += b.len() as u64; }
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }
    // Недокачанный файл не оставляем.
    if let Some((_, path, f, _, _)) = upload.take() {
        drop(f);
        let _ = tokio::fs::remove_file(&path).await;
    }
    if !upload_only {
        let _ = ev.send(SrvEvent::Clients(clients.fetch_sub(1, Ordering::SeqCst) - 1));
        wake();
    }
}

// ---------------------------------------------------------------- клиент

pub enum CliEvent {
    Connected,
    /// Соединение не установлено или разорвано; текст причины.
    Closed(String),
    State(StateMsg),
    Progress { name: String, sent: u64, total: u64 },
    UploadOk(String),
    UploadErr { name: String, msg: String },
    /// Задержка связи (пинг туда и обратно), мс.
    Rtt(u32),
    Health(HealthInfo),
    /// Миниатюра слайда (уже раскодирована).
    Thumb { id: u64, page: usize, image: eframe::egui::ColorImage },
}

pub enum CliReq {
    Cmd(Cmd),
    Upload(PathBuf),
}

/// Пока жив этот объект — живо соединение.
pub struct ClientHandle {
    tx: tmpsc::UnboundedSender<CliReq>,
}

impl ClientHandle {
    pub fn send(&self, req: CliReq) {
        let _ = self.tx.send(req);
    }
}

pub fn start_client(
    host: String,
    port: u16,
    password: String,
    ev: Sender<CliEvent>,
    wake: impl Fn() + Send + Sync + 'static,
) -> ClientHandle {
    let (tx, rx) = tmpsc::unbounded_channel();
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = ev.send(CliEvent::Closed(e.to_string()));
                wake();
                return;
            }
        };
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let reason = rt.block_on(client_task(host, port, password, rx, &ev, &wake));
        let _ = ev.send(CliEvent::Closed(reason));
        wake();
    });
    ClientHandle { tx }
}

async fn client_task(
    host: String,
    port: u16,
    password: String,
    mut rx: tmpsc::UnboundedReceiver<CliReq>,
    ev: &Sender<CliEvent>,
    wake: &Arc<dyn Fn() + Send + Sync>,
) -> String {
    let url = format!("ws://{host}:{port}");
    // Загрузки — отдельной задачей по своему соединению: команды не ждут, пока едет большой файл.
    let (up_tx, up_rx) = tmpsc::unbounded_channel::<PathBuf>();
    let uploader = tokio::spawn(upload_worker(url.clone(), password.clone(), up_rx, ev.clone(), wake.clone()));
    let _stop_uploader = AbortOnDrop(uploader);
    let conn = tokio::time::timeout(Duration::from_secs(5), tokio_tungstenite::connect_async(&url)).await;
    let mut ws = match conn {
        Ok(Ok((ws, _))) => ws,
        Ok(Err(e)) => return format!("Не удалось подключиться к {host}:{port} ({e})"),
        Err(_) => return format!("Сервер {host}:{port} не отвечает"),
    };
    if ws.send(js(&C2S::Hello { password, upload: false })).await.is_err() {
        return "Соединение разорвано".into();
    }
    match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
        Ok(Some(Ok(Message::Text(t)))) => match serde_json::from_str::<S2C>(t.as_str()) {
            Ok(S2C::Welcome) => {}
            Ok(S2C::Denied { reason }) => return reason,
            _ => return "Неожиданный ответ сервера".into(),
        },
        _ => return "Сервер не ответил на авторизацию".into(),
    }
    let _ = ev.send(CliEvent::Connected);
    wake();

    // Пинг раз в 2 с: задержка связи и признак того, что сервер жив.
    let started = std::time::Instant::now();
    let mut ping = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = ping.tick() => {
                let t = started.elapsed().as_millis() as u64;
                if ws.send(js(&C2S::Ping { ts: t })).await.is_err() { break; }
            }
            req = rx.recv() => {
                let Some(req) = req else {
                    let _ = ws.close(None).await;
                    return String::new(); // отключились сами
                };
                match req {
                    CliReq::Cmd(cmd) => {
                        if ws.send(js(&C2S::Cmd { cmd })).await.is_err() { break; }
                    }
                    CliReq::Upload(path) => {
                        let _ = up_tx.send(path);
                    }
                }
            }
            msg = ws.next() => {
                let Some(Ok(msg)) = msg else { break };
                match msg {
                    Message::Text(t) => match serde_json::from_str::<S2C>(t.as_str()) {
                        Ok(S2C::State { state }) => { let _ = ev.send(CliEvent::State(state)); wake(); }
                        Ok(S2C::UploadOk { name }) => { let _ = ev.send(CliEvent::UploadOk(name)); wake(); }
                        Ok(S2C::UploadErr { name, msg }) => { let _ = ev.send(CliEvent::UploadErr { name, msg }); wake(); }
                        Ok(S2C::Pong { ts }) => {
                            let ms = (started.elapsed().as_millis() as u64).saturating_sub(ts);
                            let _ = ev.send(CliEvent::Rtt(ms.min(u32::MAX as u64) as u32));
                            wake();
                        }
                        Ok(S2C::Health { health }) => { let _ = ev.send(CliEvent::Health(health)); wake(); }
                        _ => {}
                    },
                    Message::Binary(b) => {
                        if let Some((id, page, image)) = parse_thumb(&b).and_then(|(id, page, jpg)| Some((id, page, decode_image(jpg)?))) {
                            let _ = ev.send(CliEvent::Thumb { id, page, image });
                            wake();
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }
    "Соединение с сервером потеряно".into()
}

/// Загрузчик файлов на сервер без управляющего соединения — для дублирования на резервный сервер.
/// Пока жив — живёт; события — те же `CliEvent::Progress / UploadOk / UploadErr`.
pub struct Uploader {
    tx: tmpsc::UnboundedSender<PathBuf>,
}

impl Uploader {
    pub fn send(&self, path: PathBuf) {
        let _ = self.tx.send(path);
    }
}

pub fn start_uploader(host: String, port: u16, password: String, ev: Sender<CliEvent>, wake: impl Fn() + Send + Sync + 'static) -> Uploader {
    let (tx, rx) = tmpsc::unbounded_channel();
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        rt.block_on(upload_worker(format!("ws://{host}:{port}"), password, rx, ev, wake));
    });
    Uploader { tx }
}

/// Останавливает задачу, когда клиент отключается.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Загрузки файлов по отдельному соединению (помечено `upload`): по очереди, соединение
/// открывается при первой загрузке и переоткрывается, если оборвалось.
async fn upload_worker(
    url: String,
    password: String,
    mut rx: tmpsc::UnboundedReceiver<PathBuf>,
    ev: Sender<CliEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    let mut conn: Option<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>> = None;
    while let Some(path) = rx.recv().await {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if conn.is_none() {
            conn = open_upload_conn(&url, &password).await.ok();
        }
        let result = match conn.as_mut() {
            Some(ws) => upload(ws, &path, &name, &ev, &*wake).await,
            None => Err("не удалось открыть соединение для передачи файла".into()),
        };
        let result = match (result, conn.as_mut()) {
            // Ответ сервера о приёме файла (по этому соединению ничего другого не приходит).
            (Ok(()), Some(ws)) => match tokio::time::timeout(Duration::from_secs(120), wait_upload_reply(ws)).await {
                Ok(Ok(Ok(()))) => Ok(()),
                Ok(Ok(Err(msg))) => Err(msg),
                Ok(Err(e)) => Err(e),
                Err(_) => Err("сервер не подтвердил приём файла".into()),
            },
            (r, _) => r,
        };
        match result {
            Ok(()) => {
                let _ = ev.send(CliEvent::UploadOk(name));
            }
            Err(msg) => {
                conn = None; // при следующей загрузке — новое соединение
                let _ = ev.send(CliEvent::UploadErr { name, msg });
            }
        }
        wake();
    }
}

async fn open_upload_conn(
    url: &str,
    password: &str,
) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>, String> {
    let (mut ws, _) = tokio::time::timeout(Duration::from_secs(5), tokio_tungstenite::connect_async(url))
        .await
        .map_err(|_| "сервер не отвечает".to_string())?
        .map_err(|e| e.to_string())?;
    ws.send(js(&C2S::Hello { password: password.to_string(), upload: true })).await.map_err(|e| e.to_string())?;
    match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
        Ok(Some(Ok(Message::Text(t)))) => match serde_json::from_str::<S2C>(t.as_str()) {
            Ok(S2C::Welcome) => Ok(ws),
            Ok(S2C::Denied { reason }) => Err(reason),
            _ => Err("неожиданный ответ сервера".into()),
        },
        _ => Err("сервер не ответил на авторизацию".into()),
    }
}

/// Ждём UploadOk / UploadErr; состояние и показатели (если сервер старой версии их шлёт) пропускаем.
async fn wait_upload_reply<S>(ws: &mut tokio_tungstenite::WebSocketStream<S>) -> Result<Result<(), String>, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    while let Some(msg) = ws.next().await {
        if let Ok(Message::Text(t)) = msg {
            match serde_json::from_str::<S2C>(t.as_str()) {
                Ok(S2C::UploadOk { .. }) => return Ok(Ok(())),
                Ok(S2C::UploadErr { msg, .. }) => return Ok(Err(msg)),
                _ => {}
            }
        } else if msg.is_err() {
            break;
        }
    }
    Err("соединение для передачи файла оборвалось".into())
}

async fn upload<S>(
    ws: &mut tokio_tungstenite::WebSocketStream<S>,
    path: &PathBuf,
    name: &str,
    ev: &Sender<CliEvent>,
    wake: &(dyn Fn() + Send + Sync),
) -> Result<(), String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut f = tokio::fs::File::open(path).await.map_err(|e| e.to_string())?;
    let total = f.metadata().await.map_err(|e| e.to_string())?.len();
    let e2s = |e: tokio_tungstenite::tungstenite::Error| e.to_string();
    ws.send(js(&C2S::UploadBegin { name: name.to_string(), size: total })).await.map_err(e2s)?;
    let mut buf = vec![0u8; CHUNK];
    let mut sent = 0u64;
    loop {
        let n = f.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        ws.send(Message::binary(buf[..n].to_vec())).await.map_err(e2s)?;
        sent += n as u64;
        let _ = ev.send(CliEvent::Progress { name: name.to_string(), sent, total });
        wake();
    }
    ws.send(js(&C2S::UploadEnd)).await.map_err(e2s)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_frame_roundtrip() {
        let Message::Binary(b) = thumb_frame(0x0102_0304_0506_0708, 41, b"jpeg") else { panic!("ожидался бинарный кадр") };
        assert_eq!(parse_thumb(&b), Some((0x0102_0304_0506_0708, 41, &b"jpeg"[..])));
        assert_eq!(parse_thumb(&b[..11]), None);
    }
}
