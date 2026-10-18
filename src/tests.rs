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
fn upsert_server_adds_updates_and_trims() {
    let srv = |ip: &str, name: &str| SavedServer { name: name.into(), ip: ip.into(), port: "14999".into(), password: "p".into() };
    let mut list = vec![srv("10.0.0.1", "Зал 1")];
    upsert_server(&mut list, srv("10.0.0.2", "Зал 2"));
    assert_eq!(list.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Зал 1", "Зал 2"]);
    // Тот же адрес — обновление на месте, без дубля.
    upsert_server(&mut list, srv("10.0.0.1", "Главный"));
    assert_eq!(list.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Главный", "Зал 2"]);
    // Переполнение: новый остаётся, уходит самый давний (последний).
    let mut full: Vec<SavedServer> = (0..MAX_SERVERS).map(|i| srv(&format!("10.1.0.{i}"), "")).collect();
    upsert_server(&mut full, srv("10.9.9.9", "Новый"));
    assert_eq!(full.len(), MAX_SERVERS);
    assert_eq!(full.last().unwrap().name, "Новый");
    assert!(!full.iter().any(|s| s.ip == format!("10.1.0.{}", MAX_SERVERS - 1)));
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
    // Сначала 59 и дождаться: если прошлый прогон оборвался на громкости 60, «60» ничего бы не
    // изменило, и сервер не прислал бы нового состояния (обе команды за один кадр — тоже).
    c.send(CliReq::Cmd(Cmd::AudioVolume { volume: 59 }));
    wait("volume 59", 10, &|a| a.volume == 59);
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

/// Видео на слайде PowerPoint против запущенного сервера: TEST_PPTX — презентация, где на слайде 2
/// не меньше трёх медиа. TEST_PORT — порт. cargo test --release e2e_media -- --ignored --nocapture
#[test]
#[ignore]
fn e2e_media() {
    let port = std::env::var("TEST_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(14999);
    let (tx, rx) = channel();
    let c = net::start_client("127.0.0.1".into(), port, "default_password".into(), tx, || {});
    let t0 = Instant::now();
    let show = |m: &[MediaInfo]| m.iter().map(|x| format!("[{} «{}» {} {}/{} с]", x.id, x.name, x.state, x.pos, x.dur)).collect::<Vec<_>>().join(" ");
    let wait = |what: &str, f: &dyn Fn(&StateMsg) -> bool| -> StateMsg {
        let t = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_secs(40).saturating_sub(t.elapsed())).unwrap_or_else(|_| panic!("timeout: {what}")) {
                CliEvent::Closed(r) => panic!("closed: {r}"),
                CliEvent::State(s) if f(&s) => {
                    println!("{:>6} ms OK {what}: слайд {} {}", t0.elapsed().as_millis(), s.page + 1, show(&s.media));
                    return s;
                }
                CliEvent::State(s) if std::env::var_os("TEST_VERBOSE").is_some() => {
                    println!("{:>6} ms    ({what}) слайд {} {}", t0.elapsed().as_millis(), s.page + 1, show(&s.media));
                }
                _ => {}
            }
        }
    };
    c.send(CliReq::Cmd(Cmd::Clear));
    wait("cleared", &|s| s.items.is_empty());
    c.send(CliReq::Upload(PathBuf::from(std::env::var("TEST_PPTX").unwrap())));
    let id = wait("ready", &|s| s.items.len() == 1 && s.items[0].ready).items[0].id;
    c.send(CliReq::Cmd(Cmd::Open { id }));
    wait("open", &|s| s.active == Some(id) && s.pages >= 2);
    let s = wait("slide 1: no media", &|s| s.active == Some(id) && s.page == 0);
    assert!(s.media.is_empty());
    c.send(CliReq::Cmd(Cmd::GoTo { page: 1 }));
    let s = wait("slide 2: media found", &|s| s.page == 1 && s.media.len() >= 3);
    for (i, m) in s.media.iter().enumerate() {
        println!("   {} — id {} «{}» {} рамка {:.2},{:.2}–{:.2},{:.2}", i + 1, m.id, m.name, if m.video { "видео" } else { "звук" }, m.rect[0], m.rect[1], m.rect[2], m.rect[3]);
    }
    let ids: Vec<i32> = s.media.iter().map(|m| m.id).collect();
    let state = |s: &StateMsg, id: i32| s.media.iter().find(|m| m.id == id).map_or(9, |m| m.state);
    let pos = |s: &StateMsg, id: i32| s.media.iter().find(|m| m.id == id).map_or(0, |m| m.pos);
    let play = |id: i32| c.send(CliReq::Cmd(Cmd::PptMedia { id, action: MediaAction::Play }));
    play(ids[1]);
    wait("video 2 plays", &|s| state(s, ids[1]) == 0 && pos(s, ids[1]) >= 1);
    c.send(CliReq::Cmd(Cmd::PptMedia { id: ids[1], action: MediaAction::Pause }));
    let p = pos(&wait("video 2 paused", &|s| state(s, ids[1]) == 1), ids[1]);
    play(ids[2]);
    wait("video 3 plays (video 2 still paused)", &|s| state(s, ids[2]) == 0 && pos(s, ids[2]) >= 1 && state(s, ids[1]) == 1 && pos(s, ids[1]) == p);
    play(ids[0]);
    wait("videos 1 and 3 play", &|s| state(s, ids[0]) == 0 && state(s, ids[2]) == 0);
    for &m in &ids {
        c.send(CliReq::Cmd(Cmd::PptMedia { id: m, action: MediaAction::Stop }));
    }
    wait("all stopped", &|s| s.media.iter().all(|m| m.state >= 2));
    c.send(CliReq::Cmd(Cmd::GoTo { page: 0 }));
    wait("back to slide 1: list empty", &|s| s.page == 0 && s.media.is_empty());
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
            println!("screen={:?} active={:?} timer={:?} audio: list={} repeat={} volume={} tracks={}", s.screen, s.active, s.timer, s.audio.list, s.audio.repeat, s.audio.volume, s.audio.tracks.len());
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
    // Доиграло — остаётся открытым и снова готово; повторный старт — без загрузки.
    wait("rearmed", &|s| s.active == Some(id) && s.video.as_ref().is_some_and(|v| v.state == VideoState::Armed));
    std::thread::sleep(hold);
    c.send(CliReq::Cmd(Cmd::Play));
    wait("playing again", &|s| s.video.as_ref().is_some_and(|v| v.state == VideoState::Playing));
    wait("rearmed again", &|s| s.active == Some(id) && s.video.as_ref().is_some_and(|v| v.state == VideoState::Armed));
    c.send(CliReq::Cmd(Cmd::Close));
    wait("closed", &|s| s.active.is_none());
}
