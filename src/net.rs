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
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "t")]
enum C2S {
    Hello { password: String },
    Cmd { cmd: Cmd },
    UploadBegin { name: String, size: u64 },
    UploadEnd,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "t")]
enum S2C {
    Welcome,
    Denied { reason: String },
    State { state: StateMsg },
    UploadOk { name: String },
    UploadErr { name: String, msg: String },
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
}

pub fn start_server(
    port: u16,
    password: String,
    dir: PathBuf,
    ev: Sender<SrvEvent>,
    wake: impl Fn() + Send + Sync + 'static,
) -> ServerHandle {
    let (state_tx, state_rx) = watch::channel(String::new());
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
                    let (password, dir, state_rx) = (password.clone(), dir.clone(), state_rx.clone());
                    tokio::spawn(async move {
                        serve_client(stream, password, dir, state_rx, &ev, &wake, &clients).await;
                    });
                }
            };
            tokio::select! { _ = accept => {}, _ = stop_rx => {} }
        });
        // Runtime уничтожается здесь — все соединения закрываются.
    });
    ServerHandle { state_tx, _stop: stop_tx }
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
    ev: &Sender<SrvEvent>,
    wake: &Arc<dyn Fn() + Send + Sync>,
    clients: &AtomicUsize,
) {
    let _ = stream.set_nodelay(true);
    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else { return };

    // Авторизация: первое сообщение обязано быть Hello с верным паролем.
    let hello = tokio::time::timeout(Duration::from_secs(5), ws.next()).await;
    let ok = match hello {
        Ok(Some(Ok(Message::Text(t)))) => {
            matches!(serde_json::from_str::<C2S>(t.as_str()), Ok(C2S::Hello { password: p }) if p == password)
        }
        _ => false,
    };
    if !ok {
        let _ = ws.send(js(&S2C::Denied { reason: "Неверный пароль".into() })).await;
        let _ = ws.close(None).await;
        return;
    }
    if ws.send(js(&S2C::Welcome)).await.is_err() {
        return;
    }
    let _ = ev.send(SrvEvent::Clients(clients.fetch_add(1, Ordering::SeqCst) + 1));
    wake();

    let first = state_rx.borrow_and_update().clone();
    if !first.is_empty() {
        let _ = ws.send(Message::text(first)).await;
    }

    // Текущая загрузка: (имя, путь, файл, ожидаемый размер, принято)
    let mut upload: Option<(String, PathBuf, tokio::fs::File, u64, u64)> = None;

    loop {
        tokio::select! {
            changed = state_rx.changed() => {
                if changed.is_err() { break; }
                let s = state_rx.borrow_and_update().clone();
                if ws.send(Message::text(s)).await.is_err() { break; }
            }
            msg = ws.next() => {
                let Some(Ok(msg)) = msg else { break };
                match msg {
                    Message::Text(t) => match serde_json::from_str::<C2S>(t.as_str()) {
                        Ok(C2S::Cmd { cmd }) => { let _ = ev.send(SrvEvent::Cmd(cmd)); wake(); }
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
    let _ = ev.send(SrvEvent::Clients(clients.fetch_sub(1, Ordering::SeqCst) - 1));
    wake();
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
    wake: &(impl Fn() + Send + Sync),
) -> String {
    let url = format!("ws://{host}:{port}");
    let conn = tokio::time::timeout(Duration::from_secs(5), tokio_tungstenite::connect_async(&url)).await;
    let mut ws = match conn {
        Ok(Ok((ws, _))) => ws,
        Ok(Err(e)) => return format!("Не удалось подключиться к {host}:{port} ({e})"),
        Err(_) => return format!("Сервер {host}:{port} не отвечает"),
    };
    if ws.send(js(&C2S::Hello { password })).await.is_err() {
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

    loop {
        tokio::select! {
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
                        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        if let Err(msg) = upload(&mut ws, &path, &name, ev, wake).await {
                            let _ = ev.send(CliEvent::UploadErr { name, msg });
                            wake();
                        }
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
                        _ => {}
                    },
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }
    "Соединение с сервером потеряно".into()
}

async fn upload<S>(
    ws: &mut tokio_tungstenite::WebSocketStream<S>,
    path: &PathBuf,
    name: &str,
    ev: &Sender<CliEvent>,
    wake: &(impl Fn() + Send + Sync),
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
