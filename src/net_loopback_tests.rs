//! Интеграционные тесты настоящего WebSocket-сервера Presentator.
//! Каждый тест создаёт ТОЛЬКО локальный сервер 127.0.0.1:0 (случайный порт)
//! с отдельным временным каталогом. Никакого PowerPoint, реального GUI или
//! доступа к запущенному Presentator здесь нет.

use super::*;
use std::net::SocketAddr;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

type TestSocket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;
const WAIT: Duration = Duration::from_secs(5);
const TEST_PASSWORD: &str = "local_test_only";

struct Sandbox {
    addr: SocketAddr,
    dir: PathBuf,
    events: Receiver<SrvEvent>,
    state: watch::Sender<String>,
    thumbs: watch::Sender<Thumbs>,
    // Эти sender-ы нужно держать живыми, иначе watch::Receiver::changed() завершится.
    _health: watch::Sender<String>,
    accept_task: JoinHandle<()>,
}

impl Sandbox {
    async fn new() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("локальный TCP-порт");
        let addr = listener.local_addr().expect("адрес локального сервера");
        let dir = std::env::temp_dir().join(format!(
            "presentator_test_{}_{}", std::process::id(), unique_prefix()
        ));
        std::fs::create_dir_all(&dir).expect("временный каталог для загрузок");
        let (ev_tx, events) = mpsc::channel();
        let (state, state_rx) = watch::channel(String::new());
        let (health, health_rx) = watch::channel(String::new());
        let (thumbs, thumbs_rx) = watch::channel(Thumbs::default());
        let clients = Arc::new(AtomicUsize::new(0));
        let server_dir = dir.clone();
        let accept_task = tokio::spawn(async move {
            // Создаём по одной задаче на подключение, как в обычном сервере.
            while let Ok((stream, _)) = listener.accept().await {
                let (ev, clients, dir) = (ev_tx.clone(), clients.clone(), server_dir.clone());
                let (state, health, thumbs) =
                    (state_rx.clone(), health_rx.clone(), thumbs_rx.clone());
                tokio::spawn(async move {
                    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
                    serve_client(stream, TEST_PASSWORD.to_string(), dir, state, health, thumbs,
                        &ev, &wake, &clients).await;
                });
            }
        });
        Self { addr, dir, events, state, thumbs, _health: health, accept_task }
    }

    async fn connect_raw(&self) -> TestSocket {
        let url = format!("ws://127.0.0.1:{}/", self.addr.port());
        let (ws, _) = timeout(WAIT, tokio_tungstenite::connect_async(url))
            .await.expect("таймаут WebSocket").expect("подключение WebSocket");
        ws
    }

    async fn connect(&self, upload: bool) -> TestSocket {
        let mut ws = self.connect_raw().await;
        ws.send(js(&C2S::Hello {
            password: TEST_PASSWORD.into(), upload, version: "test-client".into(),
        })).await.expect("Hello");
        assert!(matches!(next_reply(&mut ws).await, S2C::Welcome), "авторизация тестового клиента");
        ws
    }

    async fn event(&self) -> SrvEvent {
        for _ in 0..500 {
            match self.events.try_recv() {
                Ok(ev) => return ev,
                Err(TryRecvError::Disconnected) => panic!("канал событий неожиданно закрыт"),
                Err(TryRecvError::Empty) => sleep(Duration::from_millis(10)).await,
            }
        }
        panic!("таймаут ожидания события локального сервера")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.accept_task.abort();
        // После завершения теста только наш уникальный каталог.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn next_message(ws: &mut TestSocket) -> Message {
    timeout(WAIT, ws.next()).await.expect("таймаут ответа сервера")
        .expect("соединение завершилось раньше ответа")
        .expect("ошибка чтения WebSocket")
}

async fn next_reply(ws: &mut TestSocket) -> S2C {
    let Message::Text(text) = next_message(ws).await else {
        panic!("ожидался текстовый ответ WebSocket");
    };
    serde_json::from_str(text.as_str()).expect("корректный JSON протокола")
}

fn expect_clients(ev: SrvEvent, expected: usize) {
    match ev {
        SrvEvent::Clients(n) => assert_eq!(n, expected),
        _ => panic!("ожидалось изменение числа клиентов"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_rejects_wrong_password_without_counting_client() {
    let server = Sandbox::new().await;
    let mut ws = server.connect_raw().await;
    ws.send(js(&C2S::Hello {
        password: "wrong".into(), upload: false, version: "test".into(),
    })).await.unwrap();
    match next_reply(&mut ws).await {
        S2C::Denied { reason } => assert!(reason.contains("пароль")),
        _ => panic!("неверный пароль должен отклоняться"),
    }
    assert!(server.events.try_recv().is_err(), "запрещённый клиент не должен считаться подключённым");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_rejects_command_before_authentication() {
    let server = Sandbox::new().await;
    let mut ws = server.connect_raw().await;
    ws.send(js(&C2S::Cmd { cmd: Cmd::Clear })).await.unwrap();
    assert!(matches!(next_reply(&mut ws).await, S2C::Denied { .. }));
    assert!(server.events.try_recv().is_err(), "команда до авторизации не должна исполняться");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_accepts_legacy_hello_and_forwards_commands() {
    let server = Sandbox::new().await;
    let mut ws = server.connect_raw().await;
    // Старый клиент не отправляет version и upload.
    ws.send(Message::text(format!(r#"{{"t":"Hello","password":"{}"}}"#, TEST_PASSWORD)))
        .await.unwrap();
    assert!(matches!(next_reply(&mut ws).await, S2C::Welcome));
    match server.event().await {
        SrvEvent::Hello { version } => assert_eq!(version, ""),
        _ => panic!("ожидался Hello от старого клиента"),
    }
    expect_clients(server.event().await, 1);
    ws.send(js(&C2S::Cmd { cmd: Cmd::GoTo { page: 7 } })).await.unwrap();
    match server.event().await {
        SrvEvent::Cmd(Cmd::GoTo { page }) => assert_eq!(page, 7),
        _ => panic!("сервер не передал команду GoTo"),
    }
    ws.send(js(&C2S::Ping { ts: u64::MAX - 42 })).await.unwrap();
    match next_reply(&mut ws).await {
        S2C::Pong { ts } => assert_eq!(ts, u64::MAX - 42),
        _ => panic!("сервер не ответил на Ping"),
    }
    ws.close(None).await.unwrap();
    expect_clients(server.event().await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_publishes_state_and_binary_thumbnails() {
    let server = Sandbox::new().await;
    let mut ws = server.connect(false).await;
    assert!(matches!(server.event().await, SrvEvent::Hello { .. }));
    expect_clients(server.event().await, 1);

    let mut state = StateMsg::default();
    state.active = Some(8);
    state.page = 2;
    state.pages = 11;
    state.ppt_focus = PptFocus::Lost { window: "Update".into() };
    server.state.send_replace(serde_json::to_string(&S2C::State { state }).unwrap());
    match next_reply(&mut ws).await {
        S2C::State { state } => {
            assert_eq!((state.active, state.page, state.pages), (Some(8), 2, 11));
            assert!(matches!(state.ppt_focus, PptFocus::Lost { .. }));
        }
        _ => panic!("сервер не отправил актуальное состояние"),
    }

    server.thumbs.send_replace(Arc::new(vec![(8, 2, Arc::new(b"jpegdata".to_vec()))]));
    let Message::Binary(frame) = next_message(&mut ws).await else {
        panic!("ожидалась бинарная миниатюра");
    };
    assert_eq!(parse_thumb(&frame), Some((8, 2, &b"jpegdata"[..])));
    ws.close(None).await.unwrap();
    expect_clients(server.event().await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_upload_only_sanitizes_name_and_preserves_bytes() {
    let server = Sandbox::new().await;
    let mut ws = server.connect(true).await;
    let body = b"PDF_TEST_123";
    ws.send(js(&C2S::UploadBegin {
        name: "deck:2026?.pdf".into(), size: body.len() as u64, background: false,
    })).await.unwrap();
    ws.send(Message::binary(body.to_vec())).await.unwrap();
    ws.send(js(&C2S::UploadEnd)).await.unwrap();
    match next_reply(&mut ws).await {
        S2C::UploadOk { name } => assert_eq!(name, "deck_2026_.pdf"),
        _ => panic!("сервер не подтвердил целую загрузку"),
    }
    match server.event().await {
        SrvEvent::File { name, path } => {
            assert_eq!(name, "deck_2026_.pdf");
            assert!(path.starts_with(&server.dir));
            assert_eq!(std::fs::read(path).unwrap(), body.to_vec());
        }
        _ => panic!("ожидалось событие загруженного файла"),
    }
    assert!(server.events.try_recv().is_err(), "загрузчик не должен считаться управляющим клиентом");
    ws.close(None).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_rejects_short_upload_and_deletes_partial_file() {
    let server = Sandbox::new().await;
    let mut ws = server.connect(true).await;
    ws.send(js(&C2S::UploadBegin {
        name: "short.pdf".into(), size: 7, background: false,
    })).await.unwrap();
    ws.send(Message::binary(b"123".to_vec())).await.unwrap();
    ws.send(js(&C2S::UploadEnd)).await.unwrap();
    match next_reply(&mut ws).await {
        S2C::UploadErr { name, msg } => {
            assert_eq!(name, "short.pdf");
            assert!(msg.contains("3") && msg.contains("7"));
        }
        _ => panic!("сервер обязан отклонить укороченный файл"),
    }
    assert_eq!(std::fs::read_dir(&server.dir).unwrap().count(), 0);
    assert!(server.events.try_recv().is_err(), "неполный файл не должен попадать в список");
    ws.close(None).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_background_upload_uses_background_event() {
    let server = Sandbox::new().await;
    let mut ws = server.connect(true).await;
    ws.send(js(&C2S::UploadBegin {
        name: "wallpaper.png".into(), size: 3, background: true,
    })).await.unwrap();
    ws.send(Message::binary(b"abc".to_vec())).await.unwrap();
    ws.send(js(&C2S::UploadEnd)).await.unwrap();
    assert!(matches!(next_reply(&mut ws).await, S2C::UploadOk { .. }));
    match server.event().await {
        SrvEvent::Background { name, path } => {
            assert_eq!(name, "wallpaper.png");
            assert_eq!(std::fs::read(path).unwrap(), b"abc".to_vec());
        }
        _ => panic!("фоновое изображение должно иметь отдельный тип события"),
    }
    ws.close(None).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_reconnect_updates_client_count() {
    let server = Sandbox::new().await;
    for attempt in 0..3 {
        let mut ws = server.connect(false).await;
        assert!(matches!(server.event().await, SrvEvent::Hello { .. }));
        expect_clients(server.event().await, 1);
        ws.send(js(&C2S::Ping { ts: attempt })).await.unwrap();
        assert!(matches!(next_reply(&mut ws).await, S2C::Pong { ts } if ts == attempt));
        ws.close(None).await.unwrap();
        expect_clients(server.event().await, 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_invalid_json_does_not_disconnect_authorized_client() {
    let server = Sandbox::new().await;
    let mut ws = server.connect(false).await;
    assert!(matches!(server.event().await, SrvEvent::Hello { .. }));
    expect_clients(server.event().await, 1);
    ws.send(Message::text("not json")).await.unwrap();
    ws.send(js(&C2S::Ping { ts: 1984 })).await.unwrap();
    assert!(matches!(next_reply(&mut ws).await, S2C::Pong { ts: 1984 }));
    assert!(server.events.try_recv().is_err(), "некорректный JSON не должен создавать команду");
    ws.close(None).await.unwrap();
    expect_clients(server.event().await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_real_client_sends_command_and_receives_state() {
    let server = Sandbox::new().await;
    let (tx, rx) = mpsc::channel();
    // В отличие от остальных сценариев здесь используется настоящий start_client,
    // но он подключается только к изолированному серверу этого теста.
    let client = start_client("127.0.0.1".into(), server.addr.port(),
        TEST_PASSWORD.into(), tx, || {});
    let mut connected = false;
    for _ in 0..500 {
        match rx.try_recv() {
            Ok(CliEvent::Connected) => { connected = true; break; }
            Ok(CliEvent::Closed(reason)) => panic!("клиент не подключился: {reason}"),
            Ok(_) | Err(TryRecvError::Empty) => sleep(Duration::from_millis(10)).await,
            Err(TryRecvError::Disconnected) => panic!("канал клиента закрыт"),
        }
    }
    assert!(connected, "таймаут подключения настоящего клиента");
    assert!(matches!(server.event().await, SrvEvent::Hello { .. }));
    expect_clients(server.event().await, 1);

    client.send(CliReq::Cmd(Cmd::RestorePptFocus));
    assert!(matches!(server.event().await, SrvEvent::Cmd(Cmd::RestorePptFocus)));
    let mut state = StateMsg::default();
    state.pages = 4;
    state.ppt_focus = PptFocus::Lost { window: "Office".into() };
    server.state.send_replace(serde_json::to_string(&S2C::State { state }).unwrap());
    let mut received = false;
    for _ in 0..500 {
        match rx.try_recv() {
            Ok(CliEvent::State(s)) => {
                assert_eq!(s.pages, 4);
                assert!(matches!(s.ppt_focus, PptFocus::Lost { .. }));
                received = true;
                break;
            }
            Ok(CliEvent::Closed(reason)) => panic!("соединение оборвалось: {reason}"),
            Ok(_) | Err(TryRecvError::Empty) => sleep(Duration::from_millis(10)).await,
            Err(TryRecvError::Disconnected) => panic!("канал клиента закрыт"),
        }
    }
    assert!(received, "клиент не получил состояние сервера");
    drop(client);
    expect_clients(server.event().await, 0);
}

// ---------------------------------------------------------------- этап 3: отказы, одновременные клиенты и публикация

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_new_client_gets_prepublished_state_and_thumbnails() {
    let server = Sandbox::new().await;
    let mut state = StateMsg::default();
    state.active = Some(515);
    state.page = 3;
    state.pages = 9;
    server.state.send_replace(serde_json::to_string(&S2C::State { state }).unwrap());
    server.thumbs.send_replace(Arc::new(vec![(515, 3, Arc::new(b"slide".to_vec()))]));

    let mut ws = server.connect(false).await;
    assert!(matches!(server.event().await, SrvEvent::Hello { .. }));
    expect_clients(server.event().await, 1);
    match next_reply(&mut ws).await {
        S2C::State { state } => assert_eq!((state.active, state.page, state.pages), (Some(515), 3, 9)),
        _ => panic!("new client must receive the current state"),
    }
    match next_message(&mut ws).await {
        Message::Binary(frame) => assert_eq!(parse_thumb(&frame), Some((515, 3, &b"slide"[..]))),
        _ => panic!("new client must receive the current thumbnail"),
    }
    ws.close(None).await.unwrap();
    expect_clients(server.event().await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_three_clients_receive_independent_replies() {
    let server = Sandbox::new().await;
    let (mut a, mut b, mut c) = tokio::join!(
        server.connect(false), server.connect(false), server.connect(false)
    );
    let mut counts = Vec::new();
    let mut hellos = 0;
    for _ in 0..6 {
        match server.event().await {
            SrvEvent::Hello { .. } => hellos += 1,
            SrvEvent::Clients(n) => counts.push(n),
            _ => panic!("unexpected server event during connection"),
        }
    }
    counts.sort_unstable();
    assert_eq!(hellos, 3);
    assert_eq!(counts, [1, 2, 3]);

    for (ws, stamp) in [(&mut a, 11u64), (&mut b, 22), (&mut c, 33)] {
        ws.send(js(&C2S::Ping { ts: stamp })).await.unwrap();
        assert!(matches!(next_reply(ws).await, S2C::Pong { ts } if ts == stamp));
    }
    let (a_result, b_result, c_result) = tokio::join!(a.close(None), b.close(None), c.close(None));
    a_result.unwrap();
    b_result.unwrap();
    c_result.unwrap();
    let mut disconnects = Vec::new();
    for _ in 0..3 {
        match server.event().await {
            SrvEvent::Clients(n) => disconnects.push(n),
            _ => panic!("unexpected event during disconnection"),
        }
    }
    disconnects.sort_unstable();
    assert_eq!(disconnects, [0, 1, 2]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_aborted_upload_removes_partial_file() {
    let server = Sandbox::new().await;
    let mut ws = server.connect(true).await;
    ws.send(js(&C2S::UploadBegin {
        name: "interrupted.pdf".into(), size: 100, background: false,
    })).await.unwrap();
    ws.send(Message::binary(b"half".to_vec())).await.unwrap();

    // Wait until the partial file is actually created; otherwise an empty
    // directory could make this test falsely pass before the server processes it.
    let mut observed_partial = false;
    for _ in 0..500 {
        if std::fs::read_dir(&server.dir).unwrap().next().is_some() {
            observed_partial = true;
            break;
        }
        sleep(Duration::from_millis(10)).await;
    }
    assert!(observed_partial, "server did not start receiving the upload");
    ws.close(None).await.unwrap();
    let mut removed = false;
    for _ in 0..500 {
        if std::fs::read_dir(&server.dir).unwrap().next().is_none() {
            removed = true;
            break;
        }
        sleep(Duration::from_millis(10)).await;
    }
    assert!(removed, "partial file must be deleted when upload is interrupted");
    assert!(server.events.try_recv().is_err(), "incomplete upload must not emit File");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_multichunk_upload_preserves_full_content() {
    let server = Sandbox::new().await;
    let mut ws = server.connect(true).await;
    let bytes: Vec<u8> = (0..(CHUNK * 2 + 17)).map(|i| (i % 251) as u8).collect();
    ws.send(js(&C2S::UploadBegin {
        name: "chunks.pdf".into(), size: bytes.len() as u64, background: false,
    })).await.unwrap();
    for chunk in bytes.chunks(CHUNK) {
        ws.send(Message::binary(chunk.to_vec())).await.unwrap();
    }
    ws.send(js(&C2S::UploadEnd)).await.unwrap();
    assert!(matches!(next_reply(&mut ws).await, S2C::UploadOk { name } if name == "chunks.pdf"));
    match server.event().await {
        SrvEvent::File { path, name } => {
            assert_eq!(name, "chunks.pdf");
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
        _ => panic!("complete upload must emit File"),
    }
    ws.close(None).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_empty_upload_is_accepted_without_fake_bytes() {
    let server = Sandbox::new().await;
    let mut ws = server.connect(true).await;
    ws.send(js(&C2S::UploadBegin {
        name: "empty.pdf".into(), size: 0, background: false,
    })).await.unwrap();
    ws.send(js(&C2S::UploadEnd)).await.unwrap();
    assert!(matches!(next_reply(&mut ws).await, S2C::UploadOk { name } if name == "empty.pdf"));
    match server.event().await {
        SrvEvent::File { path, .. } => assert_eq!(std::fs::metadata(path).unwrap().len(), 0),
        _ => panic!("empty upload must generate a file event"),
    }
    ws.close(None).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_bad_commands_and_unsolicited_binary_are_ignored() {
    let server = Sandbox::new().await;
    let mut ws = server.connect(false).await;
    assert!(matches!(server.event().await, SrvEvent::Hello { .. }));
    expect_clients(server.event().await, 1);
    ws.send(Message::text("[]")).await.unwrap();
    ws.send(Message::text(r#"{"t":"Cmd","cmd":{"c":"Missing"}}"#)).await.unwrap();
    ws.send(Message::binary(b"garbage".to_vec())).await.unwrap();
    ws.send(js(&C2S::Ping { ts: 23 })).await.unwrap();
    assert!(matches!(next_reply(&mut ws).await, S2C::Pong { ts: 23 }));
    assert!(server.events.try_recv().is_err(), "invalid traffic must not emit a command");
    ws.close(None).await.unwrap();
    expect_clients(server.event().await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_concurrent_uploads_use_unique_paths() {
    let server = Sandbox::new().await;
    let (mut a, mut b) = tokio::join!(server.connect(true), server.connect(true));
    for (ws, data) in [(&mut a, &b"first"[..]), (&mut b, &b"second"[..])] {
        ws.send(js(&C2S::UploadBegin {
            name: "same-name.pdf".into(), size: data.len() as u64, background: false,
        })).await.unwrap();
        ws.send(Message::binary(data.to_vec())).await.unwrap();
        ws.send(js(&C2S::UploadEnd)).await.unwrap();
    }
    assert!(matches!(next_reply(&mut a).await, S2C::UploadOk { .. }));
    assert!(matches!(next_reply(&mut b).await, S2C::UploadOk { .. }));
    let mut paths = Vec::new();
    let mut contents = Vec::new();
    for _ in 0..2 {
        match server.event().await {
            SrvEvent::File { path, name } => {
                assert_eq!(name, "same-name.pdf");
                contents.push(std::fs::read(&path).unwrap());
                paths.push(path);
            }
            _ => panic!("each uploaded file must create one event"),
        }
    }
    assert_ne!(paths[0], paths[1], "equal original filenames must not overwrite each other");
    contents.sort();
    assert_eq!(contents, [b"first".to_vec(), b"second".to_vec()]);
    a.close(None).await.unwrap();
    b.close(None).await.unwrap();
}

fn test_publisher() -> (
    ServerHandle,
    watch::Receiver<String>,
    watch::Receiver<Thumbs>,
    watch::Receiver<String>,
) {
    let (state_tx, state_rx) = watch::channel(String::new());
    let (health_tx, health_rx) = watch::channel(String::new());
    let (thumbs_tx, thumbs_rx) = watch::channel(Thumbs::default());
    let (stop_tx, _stop_rx) = oneshot::channel::<()>();
    (
        ServerHandle { state_tx, health_tx: Arc::new(health_tx), thumbs_tx, _stop: stop_tx },
        state_rx,
        thumbs_rx,
        health_rx,
    )
}

#[test]
fn publisher_does_not_notify_on_identical_state() {
    let (publisher, mut receiver, _, _) = test_publisher();
    let mut state = StateMsg::default();
    state.pages = 3;
    publisher.publish(&state);
    assert!(receiver.has_changed().unwrap());
    let first = receiver.borrow_and_update().clone();
    assert!(matches!(serde_json::from_str::<S2C>(&first).unwrap(), S2C::State { state } if state.pages == 3));
    publisher.publish(&state);
    assert!(!receiver.has_changed().unwrap(), "no repeated notification for same state");
    state.pages = 4;
    publisher.publish(&state);
    assert!(receiver.has_changed().unwrap());
}

#[test]
fn publisher_thumbnails_notify_only_when_keys_change() {
    let (publisher, _, mut receiver, _) = test_publisher();
    publisher.publish_thumbs(vec![(4, 1, Arc::new(b"jpg1".to_vec()))]);
    assert!(receiver.has_changed().unwrap());
    assert_eq!(receiver.borrow_and_update().len(), 1);
    publisher.publish_thumbs(vec![(4, 1, Arc::new(b"jpg2".to_vec()))]);
    assert!(!receiver.has_changed().unwrap(), "same slide keys should not re-publish thumbnails");
    publisher.publish_thumbs(vec![(4, 2, Arc::new(b"jpg3".to_vec()))]);
    assert!(receiver.has_changed().unwrap());
}

#[test]
fn publisher_health_updates_are_serialized() {
    let (publisher, _, _, mut receiver) = test_publisher();
    let publish = publisher.health_publisher();
    let mut health = HealthInfo::default();
    health.ui_ok = true;
    health.cpu = 18.5;
    publish(&health);
    assert!(receiver.has_changed().unwrap());
    let json = receiver.borrow_and_update().clone();
    match serde_json::from_str::<S2C>(&json).unwrap() {
        S2C::Health { health: received } => assert_eq!(received, health),
        _ => panic!("health publisher must send a Health message"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_real_client_reports_server_disconnect() {
    // Dedicated fake peer disconnects cleanly after accepting an actual start_client.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (disconnect_tx, disconnect_rx) = oneshot::channel::<()>();
    let peer = tokio::spawn(async move {
        let (socket, _) = timeout(WAIT, listener.accept()).await.unwrap().unwrap();
        let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
        let hello = timeout(WAIT, ws.next()).await.unwrap().unwrap().unwrap();
        assert!(matches!(hello, Message::Text(_)));
        ws.send(js(&S2C::Welcome)).await.unwrap();
        disconnect_rx.await.unwrap();
        ws.close(None).await.unwrap();
    });
    let (tx, rx) = mpsc::channel();
    let client = start_client("127.0.0.1".into(), port, TEST_PASSWORD.into(), tx, || {});
    let mut connected = false;
    for _ in 0..500 {
        match rx.try_recv() {
            Ok(CliEvent::Connected) => { connected = true; break; }
            Ok(CliEvent::Closed(reason)) => panic!("client prematurely disconnected: {reason}"),
            Ok(_) | Err(TryRecvError::Empty) => sleep(Duration::from_millis(10)).await,
            Err(TryRecvError::Disconnected) => panic!("client event channel closed"),
        }
    }
    assert!(connected, "real client must connect to the local fake peer");
    disconnect_tx.send(()).unwrap();
    timeout(WAIT, peer).await.unwrap().unwrap();
    let mut disconnected = false;
    for _ in 0..500 {
        match rx.try_recv() {
            Ok(CliEvent::Closed(reason)) => {
                assert!(reason.contains("потеряно"), "unexpected disconnect reason: {reason}");
                disconnected = true;
                break;
            }
            Ok(_) | Err(TryRecvError::Empty) => sleep(Duration::from_millis(10)).await,
            Err(TryRecvError::Disconnected) => panic!("client event channel closed"),
        }
    }
    assert!(disconnected, "client must report loss of the server connection");
    drop(client);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_real_uploader_reports_missing_file() {
    let server = Sandbox::new().await;
    let (tx, rx) = mpsc::channel();
    let uploader = start_uploader("127.0.0.1".into(), server.addr.port(),
        TEST_PASSWORD.into(), tx, || {});
    let missing = server.dir.join("file_does_not_exist.pdf");
    uploader.send(missing);
    let mut failed = false;
    for _ in 0..500 {
        match rx.try_recv() {
            Ok(CliEvent::UploadErr { name, msg }) => {
                assert_eq!(name, "file_does_not_exist.pdf");
                assert!(!msg.is_empty());
                failed = true;
                break;
            }
            Ok(_) | Err(TryRecvError::Empty) => sleep(Duration::from_millis(10)).await,
            Err(TryRecvError::Disconnected) => panic!("uploader event channel closed"),
        }
    }
    assert!(failed, "real uploader must report the missing local file");
    assert!(server.events.try_recv().is_err(), "failed upload must not create server File event");
    drop(uploader);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_real_uploader_sends_file_and_progress() {
    let server = Sandbox::new().await;
    let source = server.dir.join("source_fixture.bin");
    let payload = b"client-uploader-fixture";
    std::fs::write(&source, payload).unwrap();
    let (tx, rx) = mpsc::channel();
    let uploader = start_uploader("127.0.0.1".into(), server.addr.port(),
        TEST_PASSWORD.into(), tx, || {});
    uploader.send(source);
    let mut progress = false;
    let mut uploaded = false;
    for _ in 0..500 {
        match rx.try_recv() {
            Ok(CliEvent::Progress { sent, total, .. }) => {
                assert_eq!((sent, total), (payload.len() as u64, payload.len() as u64));
                progress = true;
            }
            Ok(CliEvent::UploadOk(name)) => {
                assert_eq!(name, "source_fixture.bin");
                uploaded = true;
                break;
            }
            Ok(CliEvent::UploadErr { msg, .. }) => panic!("unexpected upload error: {msg}"),
            Ok(_) | Err(TryRecvError::Empty) => sleep(Duration::from_millis(10)).await,
            Err(TryRecvError::Disconnected) => panic!("uploader event channel closed"),
        }
    }
    assert!(progress && uploaded, "uploader must emit progress and final confirmation");
    match server.event().await {
        SrvEvent::File { path, name } => {
            assert_eq!(name, "source_fixture.bin");
            assert_eq!(std::fs::read(path).unwrap(), payload);
        }
        _ => panic!("real uploader must create server File event"),
    }
    drop(uploader);
}

/// Optional, limited load test. Not run by ordinary `cargo test`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "run explicitly with cargo test stress_loopback_ -- --ignored --nocapture"]
async fn stress_loopback_repeated_connect_and_ping() {
    let server = Sandbox::new().await;
    for count in 0..50u64 {
        let mut ws = server.connect(false).await;
        assert!(matches!(server.event().await, SrvEvent::Hello { .. }));
        expect_clients(server.event().await, 1);
        ws.send(js(&C2S::Ping { ts: count })).await.unwrap();
        assert!(matches!(next_reply(&mut ws).await, S2C::Pong { ts } if ts == count));
        ws.close(None).await.unwrap();
        expect_clients(server.event().await, 0);
    }
}

/// TODO security: do not allow a temporary file to grow beyond the declared
/// byte count, even before UploadEnd. This is a known limitation and the test
/// is ignored until the corresponding production-side fix is approved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "known limitation: file may grow beyond declared size before UploadEnd"]
async fn known_issue_upload_must_not_exceed_declared_size_on_disk() {
    let server = Sandbox::new().await;
    let mut ws = server.connect(true).await;
    ws.send(js(&C2S::UploadBegin {
        name: "extra.pdf".into(), size: 2, background: false,
    })).await.unwrap();
    ws.send(Message::binary(b"too-many-bytes".to_vec())).await.unwrap();
    let mut size_on_disk = None;
    for _ in 0..500 {
        if let Some(entry) = std::fs::read_dir(&server.dir).unwrap().next() {
            let entry = entry.unwrap();
            let size = std::fs::metadata(entry.path()).unwrap().len();
            if size > 0 { size_on_disk = Some(size); break; }
        }
        sleep(Duration::from_millis(10)).await;
    }
    assert!(size_on_disk.is_some_and(|size| size <= 2),
        "temporary upload must not grow beyond declared size before UploadEnd");
}
