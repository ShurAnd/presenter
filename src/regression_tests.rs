//! Быстрые регрессионные тесты Presentator.
//!
//! Только чистая логика: без GUI, PowerPoint, сети, аудиоустройств,
//! пользовательских документов и изменений конфигурации на диске.

use super::*;

#[test]
fn config_defaults_match_portable_client_expectations() {
    let c = Config::default();
    assert_eq!(c.ip, "127.0.0.1");
    assert_eq!(c.port, "14999");
    assert_eq!(c.pdf_engine, "pdfium");
    assert_eq!(c.audio_volume, 100);
    assert!(c.keyboard && c.mirror && c.audio_list);
    assert!(c.servers.is_empty());
}

#[test]
fn config_empty_json_keeps_defaults() {
    let c: Config = serde_json::from_str("{}").unwrap();
    assert_eq!(c.port, Config::default().port);
    assert_eq!(c.pdf_engine, "pdfium");
    assert!(c.servers.is_empty());
}

#[test]
fn config_roundtrip_preserves_pdf_engine_and_reserve() {
    let mut c = Config::default();
    c.pdf_engine = "hayro".into();
    c.reserve = "192.168.1.30:15000".into();
    c.audio_volume = 27;
    c.servers.push(SavedServer {
        name: "Резервный зал".into(),
        ip: "192.168.1.30".into(),
        port: "15000".into(),
        password: "p".into(),
    });
    let copy: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
    assert_eq!(copy.pdf_engine, "hayro");
    assert_eq!(copy.reserve, "192.168.1.30:15000");
    assert_eq!(copy.audio_volume, 27);
    assert_eq!(copy.servers[0].name, "Резервный зал");
}

#[test]
fn config_ignores_unknown_fields_from_newer_version() {
    let c: Config = serde_json::from_str(r#"{"port":"15000","future_feature":{"x":1}}"#).unwrap();
    assert_eq!(c.port, "15000");
    assert_eq!(c.pdf_engine, "pdfium");
}

#[test]
fn saved_server_key_and_label_trim_whitespace() {
    let s = SavedServer {
        name: "  Основная сцена  ".into(),
        ip: " 192.168.0.4 ".into(),
        port: " 14999 ".into(),
        password: String::new(),
    };
    assert_eq!(s.key(), "192.168.0.4:14999");
    assert_eq!(s.label(), "Основная сцена — 192.168.0.4:14999");
    let s = SavedServer { name: "  ".into(), ..s };
    assert_eq!(s.label(), "192.168.0.4:14999");
}

#[test]
fn upsert_server_updates_same_address_without_changing_order() {
    let make = |ip: &str, name: &str| SavedServer {
        name: name.into(), ip: ip.into(), port: "14999".into(), password: String::new(),
    };
    let mut list = vec![make("1.1.1.1", "Первый"), make("2.2.2.2", "Второй")];
    upsert_server(&mut list, make("1.1.1.1", "Обновлён"));
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name, "Обновлён");
    assert_eq!(list[1].name, "Второй");
}

#[test]
fn merge_servers_skips_duplicates_and_respects_limit() {
    let make = |n: usize| SavedServer {
        name: n.to_string(), ip: format!("10.0.0.{n}"), port: "14999".into(), password: String::new(),
    };
    let mut ours = vec![make(1), make(2)];
    let disk: Vec<SavedServer> = (1..=30).map(make).collect();
    let forgotten: HashSet<String> = [make(3).key()].into_iter().collect();
    merge_servers(&mut ours, disk, &forgotten);
    assert_eq!(ours.len(), MAX_SERVERS);
    assert_eq!(ours[0].name, "1");
    assert_eq!(ours[1].name, "2");
    assert!(!ours.iter().any(|s| s.name == "3"));
    assert_eq!(ours.iter().filter(|s| s.name == "1").count(), 1);
}

#[test]
fn state_msg_reads_old_minimal_protocol() {
    let old = r#"{"items":[],"active":null,"page":0,"pages":0}"#;
    let msg: StateMsg = serde_json::from_str(old).unwrap();
    assert!(msg.items.is_empty());
    assert_eq!(msg.screen, Screen::Background);
    assert_eq!(msg.ppt_focus, PptFocus::Unknown);
    assert!(msg.version.is_empty());
    assert!(msg.media.is_empty());
}

#[test]
fn state_msg_roundtrip_preserves_media_timer_and_pdf_view() {
    let mut msg = StateMsg::default();
    msg.active = Some(99);
    msg.page = 3;
    msg.pages = 8;
    msg.zoom = 2.5;
    msg.view = [0.1, 0.2, 0.7, 0.8];
    msg.screen = Screen::Timer;
    msg.timer = net::TimerInfo { countdown: true, total: 95, elapsed: 1100, running: true };
    msg.media = vec![MediaInfo {
        id: 14, name: "clip.mp4".into(), video: true, state: 0,
        pos: 3, dur: 20, rect: [0.1, 0.1, 0.8, 0.8],
    }];
    msg.ppt_focus = PptFocus::Lost { window: "Microsoft Office".into() };
    let copy: StateMsg = serde_json::from_str(&serde_json::to_string(&msg).unwrap()).unwrap();
    assert_eq!(copy.active, Some(99));
    assert_eq!((copy.page, copy.pages), (3, 8));
    assert_eq!(copy.view, [0.1, 0.2, 0.7, 0.8]);
    assert_eq!(copy.screen, Screen::Timer);
    assert_eq!(copy.timer.total, 95);
    assert_eq!(copy.media[0].id, 14);
    assert_eq!(copy.ppt_focus, msg.ppt_focus);
}

#[test]
fn protocol_commands_roundtrip_through_json() {
    let commands = [
        Cmd::Open { id: 123 }, Cmd::Next, Cmd::Prev, Cmd::GoTo { page: 7 },
        Cmd::Close, Cmd::Clear, Cmd::Zoom { step: -1 },
        Cmd::Pan { dx: 1, dy: -1 }, Cmd::TimerSet { countdown: true, secs: 90 },
        Cmd::AudioVolume { volume: 42 }, Cmd::VideoRepeat { id: 9, on: true },
        Cmd::RestorePptFocus,
    ];
    for command in commands {
        let encoded = serde_json::to_string(&command).unwrap();
        let decoded: Cmd = serde_json::from_str(&encoded).unwrap();
        assert_eq!(serde_json::to_value(command).unwrap(), serde_json::to_value(decoded).unwrap());
    }
}

#[test]
fn protocol_rejects_unknown_command() {
    assert!(serde_json::from_str::<Cmd>(r#"{"c":"UnexpectedCommand"}"#).is_err());
}

#[test]
fn pdf_preference_falls_back_to_pdfium_for_unknown_values() {
    assert_eq!(render::PdfEngine::from_config("hayro"), render::PdfEngine::Hayro);
    assert_eq!(render::PdfEngine::from_config("HAYRO"), render::PdfEngine::Hayro);
    assert_eq!(render::PdfEngine::from_config("unexpected"), render::PdfEngine::Pdfium);
    assert_eq!(render::PdfEngine::Pdfium.label(), "PDFium");
}

#[test]
fn pdf_zoom_centered_uses_expected_fraction() {
    let view = render::viewport(16.0 / 9.0, (1920.0, 1080.0), 2.0, (0.5, 0.5));
    for (actual, expected) in view.iter().zip([0.25, 0.25, 0.75, 0.75]) {
        assert!((actual - expected).abs() < 0.0001, "{view:?}");
    }
}

#[test]
fn pdf_zoom_cannot_pan_outside_page() {
    for center in [(-5.0, -5.0), (5.0, 5.0), (0.5, 0.5)] {
        let view = render::viewport(4.0 / 3.0, (1920.0, 1080.0), 3.0, center);
        assert!(view[0] >= -0.0001 && view[1] >= -0.0001, "{view:?}");
        assert!(view[2] <= 1.0001 && view[3] <= 1.0001, "{view:?}");
        assert!(view[0] < view[2] && view[1] < view[3]);
    }
}

#[test]
fn pdf_view_key_clamps_and_rounds() {
    assert_eq!(render::view_key([-2.0, 0.1234, 0.9, 4.0]), [0, 1234, 9000, 10000]);
    assert_eq!(render::view_key([0.0, 0.0, 1.0, 1.0]), FULL_VIEW);
}

#[test]
fn pdf_page_keys_distinguish_page_size_and_view() {
    let base = PageKey { id: 1, page: 0, w: 1920, h: 1080, thumb: false, view: FULL_VIEW };
    let mut keys = HashSet::new();
    keys.insert(base);
    keys.insert(PageKey { page: 1, ..base });
    keys.insert(PageKey { w: 480, h: 270, thumb: true, ..base });
    keys.insert(PageKey { view: [0, 0, 5000, 5000], ..base });
    keys.insert(base);
    assert_eq!(keys.len(), 4);
}

#[test]
fn timer_duration_is_clamped_and_mode_switch_resets() {
    let mut timer = screen::Timer::default();
    timer.set(true, 0);
    assert_eq!(timer.info().total, 1);
    timer.set(true, u32::MAX);
    assert_eq!(timer.info().total, 99 * 3600 + 59 * 60 + 59);
    timer.set(false, 0);
    let info = timer.info();
    assert!(!info.countdown);
    assert_eq!(info.total, 1);
    assert_eq!(info.elapsed, 0);
    assert!(!info.running);
}

#[test]
fn timer_text_handles_minute_boundary_and_overtime() {
    let mut info = net::TimerInfo { countdown: true, total: 60, elapsed: 0, running: true };
    assert_eq!(screen::timer_text(&info, 1).0, "01:00");
    assert_eq!(screen::timer_text(&info, 1000).0, "00:59");
    info.elapsed = 60_000;
    assert_eq!(screen::timer_text(&info, 0).0, "00:00");
    assert_eq!(screen::timer_text(&info, 1_000).0, "+00:01");
    info.running = false;
    assert_eq!(screen::timer_text(&info, 10_000).0, "00:00");
}

#[test]
fn timer_repaint_tick_aligns_to_next_second() {
    let info = net::TimerInfo { countdown: false, total: 60, elapsed: 2250, running: true };
    assert_eq!(screen::timer_tick(&info, 0), Duration::from_millis(755));
    assert_eq!(screen::timer_tick(&info, 500), Duration::from_millis(255));
}

fn healthy_sample() -> health::HealthInfo {
    health::HealthInfo {
        ui_ok: true, cpu: 10.0, ram_total_mb: 16_000, ram_used_mb: 4000,
        disk_total_mb: 100_000, disk_free_mb: 40_000,
        monitors: vec!["1920×1080".into()], monitors_max: 1,
        ..Default::default()
    }
}

fn level(rows: &[health::Row], name: &str) -> health::Level {
    rows.iter().find(|r| r.name == name).unwrap().level
}

#[test]
fn health_good_sample_is_not_false_alarm() {
    let rows = health::assess(Some(&healthy_sample()), Some(25), Some(Duration::from_secs(1)));
    assert_eq!(health::overall(&rows), health::Level::Ok);
}

#[test]
fn health_network_latency_warning_and_failure_thresholds() {
    let healthy = healthy_sample();
    let at = |rtt, silence| health::assess(Some(&healthy), Some(rtt), Some(Duration::from_secs(silence)));
    assert_eq!(level(&at(300, 1), "Связь с сервером"), health::Level::Ok);
    assert_eq!(level(&at(301, 1), "Связь с сервером"), health::Level::Warn);
    assert_eq!(level(&at(1001, 1), "Связь с сервером"), health::Level::Bad);
    assert_eq!(level(&at(20, 7), "Связь с сервером"), health::Level::Bad);
}

#[test]
fn health_powerpoint_lag_warning_and_failure_thresholds() {
    let mut h = healthy_sample();
    for (ms, expected) in [(2999, health::Level::Ok), (3000, health::Level::Warn), (8000, health::Level::Bad)] {
        h.ppt_lag_ms = Some(ms);
        assert_eq!(level(&health::assess(Some(&h), None, None), "PowerPoint"), expected);
    }
}

#[test]
fn health_detects_disconnected_monitor_and_hung_ui() {
    let mut h = healthy_sample();
    h.monitors_max = 2;
    h.ui_ok = false;
    let rows = health::assess(Some(&h), None, None);
    assert_eq!(level(&rows, "Мониторы"), health::Level::Bad);
    assert_eq!(level(&rows, "Программа на сервере"), health::Level::Bad);
}

#[test]
fn player_playlist_reorder_and_remove_without_audio_device() {
    let mut p = player::Player::new(true, false, 40, |_| {});
    p.add(PathBuf::from("alpha.mp3"), "A".into());
    p.add(PathBuf::from("beta.mp3"), "B".into());
    p.add(PathBuf::from("gamma.mp3"), "C".into());
    let ids: Vec<u64> = p.info().tracks.iter().map(|t| t.id).collect();
    assert_eq!(ids.len(), 3);
    p.apply(Cmd::AudioMove { id: ids[2], up: true });
    assert_eq!(p.info().tracks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["A", "C", "B"]);
    let removed = p.apply(Cmd::AudioRemove { id: ids[0] });
    assert_eq!(removed, vec![PathBuf::from("alpha.mp3")]);
    assert_eq!(p.info().tracks.len(), 2);
}

#[test]
fn player_modes_volume_and_clear_do_not_require_audio_device() {
    let mut p = player::Player::new(true, false, 200, |_| {});
    assert_eq!(p.info().volume, 100);
    p.add(PathBuf::from("one.mp3"), "One".into());
    p.add(PathBuf::from("two.mp3"), "Two".into());
    p.apply(Cmd::AudioMode { list: false, repeat: true });
    p.apply(Cmd::AudioVolume { volume: 250 });
    let info = p.info();
    assert!(!info.list && info.repeat);
    assert_eq!(info.volume, 100);
    assert_eq!(p.apply(Cmd::AudioClear).len(), 2);
    assert!(p.info().tracks.is_empty());
}
