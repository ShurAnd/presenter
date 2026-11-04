//! Stage 4: tests of the real App command dispatcher, with no PowerPoint or real files.
//! PDF entries are synthetic; no GUI window, external server or PowerPoint is used.
//! Production command logging may still append to the usual application log.
use super::*;

fn test_app() -> App {
    let cfg = Config { pdf_engine: "hayro".into(), ..Config::default() };
    let ctx = egui::Context::default();
    let (out_tx, out_rx) = channel();
    let renderer = Renderer::new(out_tx.clone(), || {}, &cfg.pdf_engine);
    let (srv_tx, srv_rx) = channel();
    let (mirror_tx, mirror_rx) = channel();
    let (cli_tx, cli_rx) = channel();
    let ppt_host = ppt::Host::new(|_, _| {});
    let player = player::Player::new(cfg.audio_list, cfg.audio_repeat, cfg.audio_volume, |_| {});
    App {
            player,
            saved_audio: String::new(),
            audio_selected: None,
            audio_seek: None,
            cfg,
            ctx: ctx.clone(),
            work_dir: std::env::temp_dir().join("presentator_stage4_tests_no_io"),
            items: Vec::new(),
            next_id: 1,
            show: None,
            show_gen: 0,
            dirty: false,
            ppt_host,
            ppt_focus: PptFocus::Unknown,
            ppt_focus_polled: Instant::now(),
            ppt_focus_candidate: None,
            ppt_focus_restore_deadline: None,
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
            autostart: false,
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
            main_hwnd: 0,
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
    }
}

fn pdf_item(id: u64, pages: usize) -> Item {
    Item {
        id, name: format!("deck_{id}.pdf"), kind: "PDF".into(),
        src: PathBuf::from(format!("not_created_{id}.pdf")),
        pdf: Some(PathBuf::from(format!("not_created_{id}.pdf"))),
        pages, status: Status::Ready, slides: None, repeat: false, linked: vec![],
    }
}

fn open_pdf(app: &mut App, id: u64, pages: usize) {
    app.items.push(pdf_item(id, pages));
    app.apply(Cmd::Open { id });
    assert_eq!(app.show.as_ref().map(|s| s.id), Some(id));
}

#[test]
fn command_open_pdf_initializes_stage_and_replaces_show() {
    let mut a = test_app();
    open_pdf(&mut a, 11, 5);
    assert!(a.stage);
    assert_eq!(a.screen, Screen::Background);
    assert_eq!(a.show.as_ref().unwrap().page, 0);
    assert_eq!(a.show.as_ref().unwrap().zoom, 1.0);
    let first_generation = a.show_gen;
    open_pdf(&mut a, 22, 3);
    assert_eq!(a.show.as_ref().unwrap().id, 22);
    assert!(a.show_gen > first_generation);
    assert_eq!(a.local_state().active, Some(22));
    a.apply(Cmd::Close);
    assert!(a.show.is_none());
    assert_eq!(a.local_state().active, None);
    assert!(a.stage); // Closing must not tear down the stage itself.
}

#[test]
fn opening_pdf_after_powerpoint_reissues_stage_focus_and_clears_stale_focus_state() {
    let mut a = test_app();
    a.show = Some(Show {
        id: 900, page: 0, pages: 4,
        kind: ShowKind::Ppt(PptShow {
            phase: PptPhase::Shown, hwnd: 123, zoom_img: None,
            zoom_busy: None, zoom_up: false, media: (0, Vec::new()),
        }),
        gen: 1, opened: Instant::now(), zoom: 1.0, center: (0.5, 0.5),
    });
    a.stage_sent = Some((false, true));
    a.ppt_focus = PptFocus::Lost { window: "PowerPoint".into() };
    a.ppt_focus_candidate = Some(123);
    a.ppt_focus_restore_deadline = Some(Instant::now() + Duration::from_secs(2));
    a.items.push(pdf_item(901, 3));

    a.apply(Cmd::Open { id: 901 });

    assert!(matches!(a.show.as_ref().map(|s| &s.kind), Some(ShowKind::Pdf(_))));
    assert_eq!(a.stage_sent, None, "the existing stage viewport must issue Focus again");
    assert_eq!(a.ppt_focus, PptFocus::Unknown, "old PowerPoint warning must not leak into PDF state");
    assert_eq!(a.ppt_focus_candidate, None);
    assert_eq!(a.ppt_focus_restore_deadline, None);
}

#[test]
fn command_rejects_unknown_or_not_ready_items_without_interrupting_show() {
    let mut a = test_app();
    open_pdf(&mut a, 1, 4);
    let generation = a.show_gen;
    a.items.push(Item { status: Status::Preparing, ..pdf_item(2, 10) });
    a.items.push(Item { status: Status::Error("invalid PDF".into()), ..pdf_item(3, 10) });
    for id in [999, 2, 3] {
        a.apply(Cmd::Open { id });
        assert_eq!(a.show.as_ref().unwrap().id, 1);
        assert_eq!(a.show_gen, generation);
    }
}

#[test]
fn command_pdf_navigation_clamps_boundaries_and_resets_zoom() {
    let mut a = test_app();
    open_pdf(&mut a, 1, 3);
    a.apply(Cmd::Prev);
    assert_eq!(a.show.as_ref().unwrap().page, 0);
    a.apply(Cmd::Next);
    assert_eq!(a.show.as_ref().unwrap().page, 1);
    a.apply(Cmd::Zoom { step: 1 });
    assert!(a.show.as_ref().unwrap().zoom > 1.0);
    a.apply(Cmd::Next);
    assert_eq!(a.show.as_ref().unwrap().page, 2);
    assert_eq!(a.show.as_ref().unwrap().zoom, 1.0);
    a.apply(Cmd::Next);
    assert_eq!(a.show.as_ref().unwrap().page, 2);
    a.apply(Cmd::GoTo { page: usize::MAX });
    assert_eq!(a.show.as_ref().unwrap().page, 2);
    a.apply(Cmd::GoTo { page: 0 });
    assert_eq!(a.show.as_ref().unwrap().page, 0);
}

#[test]
fn command_zoom_and_pan_never_expose_area_outside_pdf() {
    let mut a = test_app();
    open_pdf(&mut a, 1, 1);
    for _ in 0..20 { a.apply(Cmd::Zoom { step: 1 }); }
    assert_eq!(a.show.as_ref().unwrap().zoom, 4.0);
    for _ in 0..25 { a.apply(Cmd::Pan { dx: 1, dy: -1 }); }
    let view = a.slide_view();
    assert!(view[0] >= 0.0 && view[1] >= 0.0, "{view:?}");
    assert!(view[2] <= 1.0 && view[3] <= 1.0, "{view:?}");
    a.apply(Cmd::Zoom { step: 0 });
    assert_eq!(a.show.as_ref().unwrap().zoom, 1.0);
    assert_eq!(a.show.as_ref().unwrap().center, (0.5, 0.5));
    assert_eq!(a.slide_view(), [0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn command_background_clock_timer_close_show_but_keep_stage() {
    let mut a = test_app();
    open_pdf(&mut a, 10, 2);
    for (cmd, screen) in [(Cmd::Clock, Screen::Clock), (Cmd::Timer, Screen::Timer), (Cmd::Background, Screen::Background)] {
        a.apply(cmd);
        assert!(a.show.is_none());
        assert_eq!(a.screen, screen);
        assert!(a.stage);
        // Reopen a PDF to ensure the following mode switch genuinely closes it.
        a.apply(Cmd::Open { id: 10 });
        assert!(a.show.is_some());
    }
}

#[test]
fn command_open_warns_about_missing_linked_media_without_changing_pages() {
    let mut a = test_app();
    let mut it = pdf_item(1, 8);
    it.linked = vec!["video.mp4 (слайд 3)".into()];
    a.items.push(it);
    a.apply(Cmd::Open { id: 1 });
    assert_eq!(a.show.as_ref().unwrap().pages, 8);
    assert!(a.status.contains("video.mp4"));
    assert!(a.status.contains("Внимание"));
}

#[test]
fn command_local_state_marks_ready_preparing_and_failed_items() {
    let mut a = test_app();
    a.items.push(pdf_item(1, 3));
    a.items.push(Item { status: Status::Preparing, ..pdf_item(2, 5) });
    a.items.push(Item { status: Status::Error("bad PDF".into()), ..pdf_item(3, 2) });
    let state = a.local_state();
    assert_eq!(state.items.len(), 3);
    assert!(state.items[0].ready);
    assert!(!state.items[1].ready);
    assert!(state.items[1].status.contains("обработка"));
    assert!(!state.items[2].ready);
    assert!(state.items[2].status.contains("bad PDF"));
    assert!(state.active.is_none());
}

#[test]
fn command_switches_to_remote_state_when_connected() {
    let mut a = test_app();
    a.status = "local".into();
    a.remote.message = "remote".into();
    assert_eq!(a.view().message, "local");
    a.connected = true;
    assert_eq!(a.view().message, "remote");
    a.connected = false;
    assert_eq!(a.view().message, "local");
}

#[test]
fn command_ppt_media_is_visible_only_on_matching_slide() {
    let mut a = test_app();
    a.show = Some(Show {
        id: 15, page: 2, pages: 5,
        kind: ShowKind::Ppt(PptShow {
            phase: PptPhase::Shown, hwnd: 0, zoom_img: None,
            zoom_busy: None, zoom_up: false,
            media: (3, vec![ppt::Media {
                id: 17, name: "clip".into(), video: true, state: 3,
                pos: 0, dur: 90, rect: [0.1, 0.2, 0.7, 0.8],
            }]),
        }),
        gen: 1, opened: Instant::now(), zoom: 1.0, center: (0.5, 0.5),
    });
    assert_eq!(a.slide_media().len(), 1);
    assert_eq!(a.slide_media()[0].id, 17);
    a.show.as_mut().unwrap().page = 3;
    assert!(a.slide_media().is_empty());
}

#[test]
fn command_restore_focus_without_active_slideshow_is_safe() {
    let mut a = test_app();
    a.apply(Cmd::RestorePptFocus);
    assert!(a.status.contains("Фокус недоступен"));
    assert_eq!(a.ppt_focus, PptFocus::Unknown);
    assert!(a.ppt_focus_restore_deadline.is_none());
}

#[test]
fn command_restore_stage_focus_requests_focus_for_active_pdf_without_reopening_it() {
    let mut a = test_app();
    open_pdf(&mut a, 901, 4);
    a.stage_sent = Some((true, true));
    let generation = a.show_gen;
    a.apply(Cmd::RestoreStageFocus);
    assert!(matches!(&a.ppt_focus, PptFocus::Restoring { .. }));
    assert!(a.ppt_focus_restore_deadline.is_some());
    assert!(a.stage_sent.is_none(), "next stage frame must issue a native Focus command");
    assert_eq!(a.show_gen, generation);
    assert_eq!(a.show.as_ref().unwrap().id, 901);
}

#[test]
fn command_restore_stage_focus_accepts_video_without_restarting_playback() {
    let mut a = test_app();
    stage8_video(&mut a, 902);
    a.stage_sent = Some((true, true));
    let generation = a.show_gen;
    a.apply(Cmd::RestoreStageFocus);
    assert!(a.stage_sent.is_none());
    assert!(matches!(a.ppt_focus, PptFocus::Restoring { .. }));
    assert_eq!(a.show_gen, generation);
    assert_eq!(a.show.as_ref().unwrap().id, 902);
}

#[test]
fn command_ppt_navigation_keeps_position_until_bridge_reports_it() {
    let mut a = test_app();
    a.show = Some(Show {
        id: 8, page: 1, pages: 9,
        kind: ShowKind::Ppt(PptShow {
            phase: PptPhase::Shown, hwnd: 0, zoom_img: None,
            zoom_busy: None, zoom_up: false, media: (0, vec![]),
        }),
        gen: 1, opened: Instant::now(), zoom: 2.0, center: (0.7, 0.7),
    });
    a.apply(Cmd::Next);
    assert_eq!(a.show.as_ref().unwrap().page, 1);
    assert_eq!(a.show.as_ref().unwrap().zoom, 1.0);
    a.apply(Cmd::GoTo { page: 7 });
    assert_eq!(a.show.as_ref().unwrap().page, 1);
    a.apply(Cmd::Prev);
    assert_eq!(a.show.as_ref().unwrap().page, 1);
}

#[test]
fn command_stage_layering_follows_powerpoint_phase() {
    let mut a = test_app();
    a.show = Some(Show {
        id: 9, page: 0, pages: 4,
        kind: ShowKind::Ppt(PptShow {
            phase: PptPhase::Loading, hwnd: 0, zoom_img: None,
            zoom_busy: None, zoom_up: false, media: (0, vec![]),
        }),
        gen: 1, opened: Instant::now(), zoom: 1.0, center: (0.5, 0.5),
    });
    assert!(a.show.as_ref().unwrap().stage_on_top());
    if let ShowKind::Ppt(p) = &mut a.show.as_mut().unwrap().kind {
        p.phase = PptPhase::Shown;
    }
    assert!(!a.show.as_ref().unwrap().stage_on_top());
    if let ShowKind::Ppt(p) = &mut a.show.as_mut().unwrap().kind {
        p.phase = PptPhase::Lowered(Instant::now());
    }
    assert!(!a.show.as_ref().unwrap().stage_on_top());
}

#[test]
fn command_video_repeat_changes_only_named_item() {
    let mut a = test_app();
    a.items.push(pdf_item(4, 1));
    a.items.push(pdf_item(5, 1));
    a.apply(Cmd::VideoRepeat { id: 5, on: true });
    assert!(!a.items[0].repeat);
    assert!(a.items[1].repeat);
    a.apply(Cmd::VideoRepeat { id: 999, on: true });
    assert!(!a.items[0].repeat);
}

#[test]
fn command_linked_text_reports_all_missing_media() {
    let content = vec!["a.mp4".into(), "b.mp3".into()];
    let description = linked_text(&content);
    assert!(description.contains("2 видео"));
    assert!(description.contains("a.mp4, b.mp3"));
}

// Stage 5: the stage Esc state machine is exercised without creating an OS window.
#[test]
fn stage_single_escape_only_arms_double_escape() {
    let mut a = test_app();
    a.stage = true;
    a.esc_press();
    assert!(a.stage);
    assert!(a.esc_at.is_some());
    assert_eq!(a.screen, Screen::Background);
}

#[test]
fn stage_double_escape_hides_empty_background() {
    let mut a = test_app();
    a.stage = true;
    a.esc_press();
    a.esc_press();
    assert!(!a.stage);
    assert!(a.esc_at.is_none());
}

#[test]
fn stage_double_escape_returns_clock_to_background_first() {
    let mut a = test_app();
    a.stage = true;
    a.screen = Screen::Clock;
    a.esc_press();
    a.esc_press();
    assert!(a.stage);
    assert_eq!(a.screen, Screen::Background);
    assert!(a.dirty);
}

#[test]
fn stage_double_escape_returns_timer_to_background_first() {
    let mut a = test_app();
    a.stage = true;
    a.screen = Screen::Timer;
    a.esc_press();
    a.esc_press();
    assert!(a.stage);
    assert_eq!(a.screen, Screen::Background);
}

#[test]
fn stage_double_escape_closes_show_but_preserves_stage() {
    let mut a = test_app();
    open_pdf(&mut a, 120, 3);
    a.esc_press();
    a.esc_press();
    assert!(a.show.is_none());
    assert!(a.stage);
}

#[test]
fn stage_expired_first_escape_requires_new_double_press() {
    let mut a = test_app();
    a.stage = true;
    a.esc_at = Some(Instant::now() - DOUBLE_ESC - Duration::from_millis(100));
    a.esc_press();
    assert!(a.stage);
    assert!(a.esc_at.is_some());
    a.esc_press();
    assert!(!a.stage);
}

// Stage 6: network controller, failover-resume and safe stage state-machine tests.
// These reuse the synthetic App; no server, client socket, Office or stage window is started.
fn saved_server(name: &str, ip: &str, port: &str, password: &str) -> SavedServer {
    SavedServer {
        name: name.into(), ip: ip.into(), port: port.into(), password: password.into(),
    }
}

fn mock_connected_client() -> (App, tokio::sync::mpsc::UnboundedReceiver<CliReq>) {
    let mut a = test_app();
    let (client, rx) = net::mock_client_for_tests();
    a.client = Some(client);
    a.connected = true;
    (a, rx)
}

fn resume(name: &str, page: usize, opened: Option<u64>) -> Resume {
    Resume { name: name.into(), page, opened, since: Instant::now() }
}

fn healthy_snapshot() -> health::HealthInfo {
    health::HealthInfo {
        ui_ok: true,
        disk_free_mb: 32_000,
        disk_total_mb: 64_000,
        monitors: vec!["1920x1080".into()],
        monitors_max: 1,
        ..Default::default()
    }
}

#[test]
fn network_port_accepts_trimmed_numbers_and_maximum_port() {
    for (value, expected) in [(" 1 ", 1u16), (" 14999  ", 14999), ("65535", 65535)] {
        let mut a = test_app();
        a.cfg.port = value.into();
        a.net_status = "untouched".into();
        assert_eq!(a.port(), Some(expected));
        assert_eq!(a.net_status, "untouched");
    }
}

#[test]
fn network_port_rejects_zero_overflow_and_invalid_input() {
    for value in ["", " ", "0", "65536", "999999999999999999", "-2", "12.5", "abc"] {
        let mut a = test_app();
        a.cfg.port = value.into();
        assert_eq!(a.port(), None, "accepted invalid port {value:?}");
        assert_eq!(a.net_status, "Неверный порт");
        assert!(a.client.is_none());
    }
}

#[test]
fn network_reserve_requires_an_exact_selected_key() {
    let mut a = test_app();
    a.cfg.servers.push(saved_server("Backup", "10.0.0.2", "15000", "secret"));
    assert!(a.reserve_server().is_none());
    a.cfg.reserve = "10.0.0.3:15000".into();
    assert!(a.reserve_server().is_none());
}

#[test]
fn network_reserve_excludes_the_current_connection_even_with_whitespace() {
    let mut a = test_app();
    a.cfg.ip = " 10.0.0.1 ".into();
    a.cfg.port = " 14999 ".into();
    a.cfg.servers.push(saved_server("Current", " 10.0.0.1", "14999 ", "secret"));
    a.cfg.reserve = "10.0.0.1:14999".into();
    assert!(a.reserve_server().is_none());
}

#[test]
fn network_reserve_selects_other_host_and_preserves_credentials() {
    let mut a = test_app();
    a.cfg.servers.push(saved_server("Current", "127.0.0.1", "14999", "old"));
    a.cfg.servers.push(saved_server("Backup", "10.0.0.2", "15000", "keep this"));
    a.cfg.reserve = "10.0.0.2:15000".into();
    let s = a.reserve_server().expect("selected backup must exist");
    assert_eq!(s.ip, "10.0.0.2");
    assert_eq!(s.port, "15000");
    assert_eq!(s.password, "keep this");
    assert_eq!(s.name, "Backup");
}

#[test]
fn network_reserve_distinguishes_servers_with_identical_labels() {
    let mut a = test_app();
    a.cfg.servers.push(saved_server("Auditorium", "10.1.1.1", "15001", "one"));
    a.cfg.servers.push(saved_server("Auditorium", "10.1.1.2", "15001", "two"));
    a.cfg.reserve = "10.1.1.2:15001".into();
    assert_eq!(a.reserve_server().unwrap().password, "two");
}

#[test]
fn network_disconnect_clears_remote_state_and_pending_reconnect() {
    let (mut a, mut rx) = mock_connected_client();
    a.remote.active = Some(42);
    a.remote.message = "remote state".into();
    a.lost = Some("connection lost".into());
    a.reconnect = Some((2, Instant::now()));
    a.disconnect();
    assert!(a.client.is_none());
    assert!(!a.connected);
    assert!(a.remote.active.is_none());
    assert!(a.remote.items.is_empty());
    assert_eq!(a.remote.message, "");
    assert!(a.lost.is_none());
    assert!(a.reconnect.is_none());
    assert!(rx.try_recv().is_err());
}

#[test]
fn network_toggle_client_disconnects_existing_handle_without_dialing() {
    let (mut a, _rx) = mock_connected_client();
    a.cfg.port = "invalid".into();
    let ctx = a.ctx.clone();
    a.toggle_client(&ctx);
    assert!(a.client.is_none());
    assert!(!a.connected);
    assert_eq!(a.net_status, "Отключено");
}

#[test]
fn network_toggle_client_does_not_connect_with_invalid_port() {
    let mut a = test_app();
    a.cfg.port = "0".into();
    let ctx = a.ctx.clone();
    a.toggle_client(&ctx);
    assert!(a.client.is_none());
    assert_eq!(a.net_status, "Неверный порт");
}

#[test]
fn network_resume_waits_for_a_matching_remote_filename() {
    let (mut a, mut rx) = mock_connected_client();
    a.resume = Some(resume("Slides.pdf", 3, None));
    a.remote.items.push(ItemInfo { id: 1, name: "Other.pdf".into(), ready: true, ..Default::default() });
    a.continue_resume();
    assert_eq!(a.resume.as_ref().unwrap().opened, None);
    assert!(rx.try_recv().is_err());
    a.remote.items.push(ItemInfo { id: 2, name: "Slides.pdf".into(), ready: true, ..Default::default() });
    a.continue_resume();
    assert_eq!(a.resume.as_ref().unwrap().opened, Some(2));
    assert!(matches!(rx.try_recv(), Ok(CliReq::Cmd(Cmd::Open { id: 2 }))));
}

#[test]
fn network_resume_does_not_open_a_remote_file_still_preparing() {
    let (mut a, mut rx) = mock_connected_client();
    a.resume = Some(resume("Slides.pdf", 0, None));
    a.remote.items.push(ItemInfo { id: 18, name: "Slides.pdf".into(), ready: false, ..Default::default() });
    a.continue_resume();
    assert_eq!(a.resume.as_ref().unwrap().opened, None);
    assert!(rx.try_recv().is_err());
    a.remote.items[0].ready = true;
    a.continue_resume();
    assert!(matches!(rx.try_recv(), Ok(CliReq::Cmd(Cmd::Open { id: 18 }))));
    assert_eq!(a.resume.as_ref().unwrap().opened, Some(18));
}

#[test]
fn network_resume_open_command_is_issued_once_while_awaiting_activation() {
    let (mut a, mut rx) = mock_connected_client();
    a.resume = Some(resume("deck.pdf", 6, None));
    a.remote.items.push(ItemInfo { id: 31, name: "deck.pdf".into(), ready: true, ..Default::default() });
    a.continue_resume();
    assert!(matches!(rx.try_recv(), Ok(CliReq::Cmd(Cmd::Open { id: 31 }))));
    a.continue_resume();
    assert!(rx.try_recv().is_err(), "must not re-open a pending presentation");
}

#[test]
fn network_resume_ignores_unrelated_active_presentation() {
    let (mut a, mut rx) = mock_connected_client();
    a.resume = Some(resume("deck.pdf", 8, Some(31)));
    a.remote.active = Some(99);
    a.continue_resume();
    assert_eq!(a.resume.as_ref().unwrap().opened, Some(31));
    assert!(rx.try_recv().is_err());
}

#[test]
fn network_resume_restores_slide_only_after_expected_presentation_appears() {
    let (mut a, mut rx) = mock_connected_client();
    a.resume = Some(resume("deck.pdf", 5, Some(31)));
    a.remote.active = Some(31);
    a.continue_resume();
    assert!(a.resume.is_none());
    assert!(a.status.contains("deck.pdf"));
    assert!(a.status.contains("слайд 6"));
    assert!(matches!(rx.try_recv(), Ok(CliReq::Cmd(Cmd::GoTo { page: 5 }))));
    assert!(rx.try_recv().is_err());
}

#[test]
fn network_resume_does_not_seek_when_remote_active_item_is_video() {
    let (mut a, mut rx) = mock_connected_client();
    a.resume = Some(resume("clip.mp4", 77, Some(21)));
    a.remote.active = Some(21);
    a.remote.video = Some(VideoInfo { state: VideoState::Armed, pos: 0, dur: 120 });
    a.continue_resume();
    assert!(a.resume.is_none());
    assert!(rx.try_recv().is_err(), "video resume must not issue slide navigation");
}

#[test]
fn network_resume_at_first_slide_needs_no_navigation_command() {
    let (mut a, mut rx) = mock_connected_client();
    a.resume = Some(resume("deck.pdf", 0, Some(31)));
    a.remote.active = Some(31);
    a.continue_resume();
    assert!(a.resume.is_none());
    assert!(a.status.contains("слайд 1"));
    assert!(rx.try_recv().is_err());
}

#[test]
fn network_resume_expires_and_reports_the_missing_show() {
    let (mut a, mut rx) = mock_connected_client();
    let mut r = resume("Important deck.pdf", 17, None);
    r.since = Instant::now() - Duration::from_secs(21);
    a.resume = Some(r);
    a.continue_resume();
    assert!(a.resume.is_none());
    assert!(a.status.contains("Important deck.pdf"));
    assert!(a.status.contains("не удалось"));
    assert!(rx.try_recv().is_err());
}

#[test]
fn network_old_remote_without_live_pings_does_not_report_communication_alarm() {
    let mut a = test_app();
    a.connected = true;
    a.remote_live = false;
    a.last_seen = Some(Instant::now() - Duration::from_secs(90));
    let (level, rows) = a.health_rows();
    assert_eq!(level, health::Level::Ok);
    assert!(rows.is_empty());
}

#[test]
fn network_live_remote_connection_reports_stale_heartbeat() {
    let mut a = test_app();
    a.connected = true;
    a.remote_live = true;
    a.last_seen = Some(Instant::now() - Duration::from_secs(8));
    let (level, rows) = a.health_rows();
    assert_eq!(level, health::Level::Bad);
    assert!(rows.iter().any(|r| r.name == "Связь с сервером" && r.level == health::Level::Bad));
}

#[test]
fn network_remote_health_becomes_bad_when_metrics_stop_arriving() {
    let mut a = test_app();
    a.connected = true;
    a.remote_live = true;
    a.remote_health = Some(healthy_snapshot());
    a.health_at = Some(Instant::now() - Duration::from_secs(8));
    a.last_seen = Some(Instant::now());
    let (level, rows) = a.health_rows();
    assert_eq!(level, health::Level::Bad);
    assert!(rows.iter().any(|r| r.name == "Программа на сервере" && r.level == health::Level::Bad));
}

#[test]
fn network_disconnected_uses_local_health_without_fake_network_row() {
    let mut a = test_app();
    a.connected = false;
    a.local_health = Some(healthy_snapshot());
    a.remote_live = true;
    a.last_seen = Some(Instant::now() - Duration::from_secs(60));
    let (level, rows) = a.health_rows();
    assert_eq!(level, health::Level::Ok);
    assert!(rows.iter().any(|r| r.name == "Программа на сервере"));
    assert!(!rows.iter().any(|r| r.name == "Связь с сервером"));
}

#[test]
fn stage_hidden_resets_sent_window_state_without_loading_background() {
    let mut a = test_app();
    a.stage = false;
    a.stage_sent = Some((true, false));
    a.bg_checked = true;
    a.bg_gen = 9;
    let ctx = a.ctx.clone();
    a.stage_window(&ctx);
    assert!(a.stage_sent.is_none());
    assert!(!a.bg_checked);
    assert_eq!(a.bg_gen, 9);
}

#[test]
fn stage_minimize_releases_main_window_when_scene_is_gone() {
    let mut a = test_app();
    a.stage = false;
    a.main_minimized = true;
    a.stage_up = true;
    let ctx = a.ctx.clone();
    a.minimize_for_stage(&ctx);
    assert!(!a.main_minimized);
    assert!(!a.stage_up);
}

#[test]
fn stage_remote_client_does_not_minimize_local_main_window() {
    let mut a = test_app();
    a.stage = true;
    a.connected = true;
    let ctx = a.ctx.clone();
    a.minimize_for_stage(&ctx);
    assert!(!a.main_minimized);
    assert!(!a.stage_up);
}

#[test]
fn stage_windowed_mode_releases_previous_minimization() {
    let mut a = test_app();
    a.stage = true;
    a.cfg.windowed = true;
    a.main_minimized = true;
    let ctx = a.ctx.clone();
    a.minimize_for_stage(&ctx);
    assert!(!a.main_minimized);
    assert!(!a.stage_up);
}

#[test]
fn stage_double_escape_first_restores_background_then_hides_scene() {
    let mut a = test_app();
    a.stage = true;
    a.screen = Screen::Clock;
    a.esc_press();
    a.esc_press();
    assert_eq!(a.screen, Screen::Background);
    assert!(a.stage);
    assert!(a.esc_at.is_none());
    a.esc_press();
    a.esc_press();
    assert!(!a.stage);
    assert!(a.esc_at.is_none());
}

#[test]
fn stage_double_escape_closes_pdf_before_allowing_scene_to_hide() {
    let mut a = test_app();
    open_pdf(&mut a, 77, 3);
    a.esc_press();
    a.esc_press();
    assert!(a.show.is_none());
    assert!(a.stage);
    a.esc_press();
    a.esc_press();
    assert!(!a.stage);
}
// Stage 7: headless egui state/interaction coverage. Rendering the operator panels here
// never opens a native viewport, starts a server or launches PowerPoint/media engines.
// Each test uses the same synthetic App established by test_app().
fn stage7_frame(app: &mut App, mut draw: impl FnMut(&mut App, &mut egui::Ui, &StateMsg)) {
    let ctx = app.ctx.clone();
    configure_operator_style(&ctx);
    let state = app.view();
    let mut input = egui::RawInput::default();
    input.screen_rect = Some(egui::Rect::from_min_size(
        egui::Pos2::ZERO, egui::vec2(1400.0, 950.0),
    ));
    let _ = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            draw(app, ui, &state);
        });
    });
}

fn stage7_media(id: i32, state: u8, video: bool) -> MediaInfo {
    MediaInfo {
        id, name: format!("Медиа {id}"), video, state, pos: 12, dur: 125,
        rect: [0.15, 0.2, 0.7, 0.85],
    }
}

fn stage7_remote_pdf(page: usize, pages: usize) -> StateMsg {
    StateMsg {
        active: Some(10), page, pages,
        zoom: 1.0, view: [0.0, 0.0, 1.0, 1.0],
        items: vec![ItemInfo {
            id: 10, name: "Презентация.pdf".into(), kind: "PDF".into(),
            pages, ready: true, ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn ui_time_renders_unknown_duration_and_hours_without_truncating() {
    assert_eq!(audio_time(0, 0), "00:00");
    assert_eq!(audio_time(59, 0), "00:59");
    assert_eq!(audio_time(61, 125), "01:01 / 02:05");
    assert_eq!(audio_time(3601, 7200), "60:01 / 120:00");
}

#[test]
fn ui_now_playing_silent_and_paused_have_distinct_labels() {
    let mut a = AudioInfo::default();
    assert!(now_playing(&a).is_none());
    a.tracks.push(net::TrackInfo { id: 7, name: "Марш".into() });
    a.current = Some(7);
    a.state = AudioState::Paused;
    let (label, color) = now_playing(&a).unwrap();
    assert!(label.contains("Марш") && label.contains("паузе"));
    assert_eq!(color, BLUE);
}

#[test]
fn ui_now_playing_music_and_fade_have_expected_presentation() {
    let mut a = AudioInfo::default();
    a.current = Some(9);
    a.tracks.push(net::TrackInfo { id: 9, name: "Вальс".into() });
    a.pos = 81;
    a.dur = 245;
    a.state = AudioState::Playing;
    let (label, color) = now_playing(&a).unwrap();
    assert!(label.contains("Вальс") && label.contains("01:21 / 04:05"));
    assert_eq!(color, GREEN);
    a.state = AudioState::Stopping;
    let (label, color) = now_playing(&a).unwrap();
    assert!(label.contains("затихает"));
    assert_eq!(color, GRAY);
}

#[test]
fn ui_now_playing_insertion_overrides_music_only_while_active() {
    let mut a = AudioInfo::default();
    a.tracks = vec![
        net::TrackInfo { id: 1, name: "Фон".into() },
        net::TrackInfo { id: 2, name: "Фанфары".into() },
    ];
    a.current = Some(1);
    a.state = AudioState::Playing;
    a.insert = Some(2);
    a.insert_state = AudioState::Playing;
    a.insert_pos = 4;
    a.insert_dur = 20;
    let (label, color) = now_playing(&a).unwrap();
    assert!(label.contains("Вставка: Фанфары 00:04 / 00:20"));
    assert_eq!(color, INSERT);
    a.insert_state = AudioState::Stopped;
    assert!(now_playing(&a).unwrap().0.contains("Фон"));
}

#[test]
fn ui_now_playing_unknown_track_does_not_panic() {
    let mut a = AudioInfo::default();
    a.current = Some(999);
    a.state = AudioState::Playing;
    assert!(now_playing(&a).unwrap().0.starts_with("Музыка:"));
}

#[test]
fn ui_video_label_covers_all_playback_states() {
    for (state, prefix, color) in [
        (VideoState::Loading, "видео загружается", GRAY),
        (VideoState::Armed, "готово к запуску", GREEN),
        (VideoState::Playing, "▶ 01:01 / 02:05", GREEN),
        (VideoState::Paused, "пауза 01:01 / 02:05", BLUE),
    ] {
        let (label, actual) = video_label(&VideoInfo { state, pos: 61, dur: 125 });
        assert!(label.starts_with(prefix), "{label:?}");
        assert_eq!(actual, color);
    }
}

#[test]
fn ui_health_level_colors_distinguish_all_severities() {
    assert_eq!(level_color(health::Level::Ok), GREEN);
    assert_eq!(level_color(health::Level::Bad), RED);
    assert_ne!(level_color(health::Level::Warn), GREEN);
    assert_ne!(level_color(health::Level::Warn), RED);
}

#[test]
fn ui_versions_compare_numerically_not_lexicographically() {
    use std::cmp::Ordering as O;
    assert_eq!(version_cmp("2.10.0", "2.9.9"), O::Greater);
    assert_eq!(version_cmp("2.4.1", "2.4.1"), O::Equal);
    assert_eq!(version_cmp("", "1.0"), O::Less);
    assert_eq!(version_cmp("2.bad.1", "2.0.1"), O::Equal);
    assert_eq!(version_cmp("2.4", "2.4.0"), O::Less);
}

#[test]
fn ui_thumbnail_window_includes_previous_current_two_ahead() {
    assert!(!thumb_near(0, 2));
    assert!(thumb_near(1, 2));
    assert!(thumb_near(2, 2));
    assert!(thumb_near(3, 2));
    assert!(thumb_near(4, 2));
    assert!(!thumb_near(5, 2));
}

#[test]
fn ui_thumbnail_window_handles_page_zero() {
    assert!(thumb_near(0, 0));
    assert!(thumb_near(1, 0));
    assert!(thumb_near(2, 0));
    assert!(!thumb_near(3, 0));
}

#[test]
fn ui_goto_number_uses_one_based_slide_numbers() {
    let mut a = test_app();
    open_pdf(&mut a, 10, 8);
    let view = a.view();
    a.goto_number("3", &view);
    assert_eq!(a.show.as_ref().unwrap().page, 2);
    a.goto_number("8", &view);
    assert_eq!(a.show.as_ref().unwrap().page, 7);
    a.goto_number("1", &view);
    assert_eq!(a.show.as_ref().unwrap().page, 0);
}

#[test]
fn ui_goto_number_rejects_zero_oversized_and_nonnumeric_input() {
    let mut a = test_app();
    open_pdf(&mut a, 10, 5);
    let view = a.view();
    for invalid in ["0", "6", "-1", "xyz", "999999999999999999999"] {
        a.goto_number(invalid, &view);
        assert!(a.status.contains("Нет слайда"));
        assert_eq!(a.show.as_ref().unwrap().page, 0);
    }
    a.status.clear();
    a.goto_number("", &view);
    assert!(a.status.is_empty());
}

#[test]
fn ui_goto_typed_buffers_digits_until_enter_and_clears_them() {
    let mut a = test_app();
    open_pdf(&mut a, 10, 30);
    let view = a.view();
    assert!(!a.goto_typed("1", false, &view));
    assert!(!a.goto_typed("2", false, &view));
    assert_eq!(a.goto_buf, "12");
    assert_eq!(a.show.as_ref().unwrap().page, 0);
    assert!(a.goto_typed("", true, &view));
    assert_eq!(a.show.as_ref().unwrap().page, 11);
    assert!(a.goto_buf.is_empty());
}

#[test]
fn ui_goto_typed_refuses_video_and_inactive_presentations() {
    let mut a = test_app();
    let view = stage7_remote_pdf(0, 10);
    assert!(!a.goto_typed("12", false, &view));
    assert_eq!(a.goto_buf, "12");
    let mut video = view.clone();
    video.video = Some(VideoInfo { state: VideoState::Armed, pos: 0, dur: 0 });
    assert!(!a.goto_typed("3", false, &video));
    assert!(a.goto_buf.is_empty());
    let mut inactive = view;
    inactive.active = None;
    assert!(!a.goto_typed("2", true, &inactive));
    assert!(a.goto_buf.is_empty());
}

#[test]
fn ui_goto_typed_limits_length_and_discards_expired_buffer() {
    let mut a = test_app();
    let view = stage7_remote_pdf(0, 999);
    assert!(!a.goto_typed("123456", false, &view));
    assert_eq!(a.goto_buf, "1234");
    a.goto_at = Some(Instant::now() - GOTO_TIMEOUT - Duration::from_secs(1));
    assert!(!a.goto_typed("7", false, &view));
    assert_eq!(a.goto_buf, "7");
}

#[test]
fn ui_timer_extra_zero_locally_and_elapsed_on_remote() {
    let mut a = test_app();
    assert_eq!(a.timer_extra(), 0);
    a.connected = true;
    a.remote_at = Instant::now() - Duration::from_millis(70);
    assert!(a.timer_extra() >= 60);
}

#[test]
fn ui_goto_number_forwards_remote_command_without_changing_local_show() {
    let (mut a, mut rx) = mock_connected_client();
    a.remote = stage7_remote_pdf(4, 9);
    let view = a.view();
    a.goto_number("9", &view);
    assert!(matches!(rx.try_recv(), Ok(CliReq::Cmd(Cmd::GoTo { page: 8 }))));
    assert!(a.show.is_none());
}

#[test]
fn ui_draw_empty_list_and_basic_controls_without_native_window() {
    let mut a = test_app();
    stage7_frame(&mut a, |a, ui, state| {
        a.tab_row(ui, state);
        a.list(ui, state);
        a.bottom_bar(ui, state);
    });
    assert!(a.items.is_empty());
    assert!(a.confirm.is_none());
}

#[test]
fn ui_draw_local_presentations_with_badges_and_selection() {
    let mut a = test_app();
    a.items.push(pdf_item(11, 5));
    a.items.push(Item { status: Status::Preparing, ..pdf_item(12, 0) });
    a.items.push(Item { status: Status::Error("Неверный формат".into()), ..pdf_item(13, 0) });
    a.items.push(Item { kind: "ВИДЕО".into(), repeat: true, linked: vec!["x.mp4".into()], ..pdf_item(14, 0) });
    a.selected = Some(12);
    stage7_frame(&mut a, |a, ui, state| {
        a.list(ui, state);
        a.bottom_bar(ui, state);
    });
    assert_eq!(a.selected, Some(12));
}

#[test]
fn ui_draw_remote_video_list_for_loading_ready_playing_paused() {
    for state in [VideoState::Loading, VideoState::Armed, VideoState::Playing, VideoState::Paused] {
        let mut a = test_app();
        a.connected = true;
        a.remote = stage7_remote_pdf(0, 1);
        a.remote.items[0].kind = "ВИДЕО".into();
        a.remote.video = Some(VideoInfo { state, pos: 62, dur: 180 });
        stage7_frame(&mut a, |a, ui, view| {
            a.list(ui, view);
            a.bottom_bar(ui, view);
        });
    }
}

#[test]
fn ui_draw_player_empty_and_nonempty_playlists() {
    let mut a = test_app();
    stage7_frame(&mut a, |a, ui, state| a.player_panel(ui, state));
    let mut a = test_app();
    a.connected = true;
    a.remote.audio.tracks = vec![
        net::TrackInfo { id: 1, name: "Песня".into() },
        net::TrackInfo { id: 2, name: "Фанфары".into() },
    ];
    a.audio_selected = Some(2);
    a.remote.audio.current = Some(1);
    a.remote.audio.state = AudioState::Playing;
    a.remote.audio.pos = 38;
    a.remote.audio.dur = 180;
    stage7_frame(&mut a, |a, ui, state| a.player_panel(ui, state));
    assert_eq!(a.audio_selected, Some(2));
}

#[test]
fn ui_draw_player_with_insertion_paused_and_fading_states() {
    for state in [AudioState::Paused, AudioState::Stopping] {
        let mut a = test_app();
        a.connected = true;
        a.remote.audio.tracks = vec![
            net::TrackInfo { id: 1, name: "Музыка".into() },
            net::TrackInfo { id: 2, name: "Вставка".into() },
        ];
        a.remote.audio.current = Some(1);
        a.remote.audio.state = state;
        a.remote.audio.insert = Some(2);
        a.remote.audio.insert_state = AudioState::Playing;
        a.remote.audio.pos = 45;
        a.remote.audio.dur = 90;
        a.remote.audio.insert_dur = 12;
        stage7_frame(&mut a, |a, ui, state| a.player_panel(ui, state));
    }
}

#[test]
fn ui_draw_network_panel_with_and_without_saved_servers() {
    let mut a = test_app();
    stage7_frame(&mut a, |a, ui, _| a.network_panel(ui));
    a.cfg.servers = vec![
        saved_server("Зал", "127.0.0.1", "14999", "secret"),
        saved_server("Резерв", "10.0.0.2", "15001", "other"),
    ];
    a.cfg.reserve = "10.0.0.2:15001".into();
    stage7_frame(&mut a, |a, ui, _| a.network_panel(ui));
    assert_eq!(a.cfg.servers.len(), 2);
}

#[test]
fn ui_draw_network_panel_when_remote_client_is_connected() {
    let (mut a, _rx) = mock_connected_client();
    a.net_status = "Подключено".into();
    stage7_frame(&mut a, |a, ui, _| a.network_panel(ui));
    assert!(a.connected);
}

#[test]
fn ui_draw_preview_pdf_first_middle_and_last_pages() {
    for (page, pages, zoom) in [(0, 5, 1.0), (2, 5, 2.0), (4, 5, 4.0)] {
        let mut a = test_app();
        a.connected = true;
        a.remote = stage7_remote_pdf(page, pages);
        a.remote.zoom = zoom;
        a.remote.view = [0.25, 0.25, 0.75, 0.75];
        stage7_frame(&mut a, |a, ui, state| a.preview_bar(ui, state));
        assert!(a.show.is_none());
    }
}

#[test]
fn ui_draw_preview_ignores_inactive_presentation() {
    let mut a = test_app();
    stage7_frame(&mut a, |a, ui, state| a.preview_bar(ui, state));
    assert!(a.previews.is_empty());
}

#[test]
fn ui_draw_preview_media_icons_and_zoom_panel() {
    let mut a = test_app();
    a.connected = true;
    a.remote = stage7_remote_pdf(1, 5);
    a.remote.items[0].kind = "PPTX".into();
    a.remote.media = vec![
        stage7_media(1, 0, true),
        stage7_media(2, 1, true),
        stage7_media(3, 2, false),
        stage7_media(4, 3, false),
    ];
    a.remote.zoom = 2.0;
    a.remote.view = [0.1, 0.2, 0.8, 0.9];
    stage7_frame(&mut a, |a, ui, state| a.preview_bar(ui, state));
    assert!(a.confirm.is_none());
}

#[test]
fn ui_draw_bottom_bar_countdown_running_and_stopped() {
    for running in [false, true] {
        let mut a = test_app();
        a.connected = true;
        a.remote.screen = Screen::Timer;
        a.remote.timer.countdown = true;
        a.remote.timer.total = 65;
        a.remote.timer.running = running;
        a.remote.timer.elapsed = 30_000;
        stage7_frame(&mut a, |a, ui, state| a.bottom_bar(ui, state));
        assert_eq!(a.remote.timer.total, 65);
    }
}

#[test]
fn ui_draw_bottom_bar_stopwatch_overtime_and_clock() {
    for (countdown, elapsed) in [(false, 4_500), (true, 190_000)] {
        let mut a = test_app();
        a.connected = true;
        a.remote.screen = Screen::Timer;
        a.remote.timer.countdown = countdown;
        a.remote.timer.total = 120;
        a.remote.timer.elapsed = elapsed;
        stage7_frame(&mut a, |a, ui, state| a.bottom_bar(ui, state));
    }
    let mut a = test_app();
    a.screen = Screen::Clock;
    stage7_frame(&mut a, |a, ui, state| a.bottom_bar(ui, state));
}

#[test]
fn ui_draw_top_bar_on_client_and_local_server() {
    let mut a = test_app();
    stage7_frame(&mut a, |a, ui, state| a.top_bar(ui, state));
    let (mut a, _rx) = mock_connected_client();
    a.remote = stage7_remote_pdf(1, 4);
    a.remote.audio.tracks = vec![net::TrackInfo { id: 11, name: "Трек".into() }];
    a.remote.audio.current = Some(11);
    a.remote.audio.state = AudioState::Playing;
    stage7_frame(&mut a, |a, ui, state| {
        a.top_bar(ui, state);
        a.tab_row(ui, state);
    });
}

#[test]
fn ui_draw_health_window_empty_and_real_samples() {
    let mut a = test_app();
    a.health_open = true;
    a.server_online = true;
    stage7_frame(&mut a, |a, _, _| { let ctx = a.ctx.clone(); a.health_window(&ctx); });
    assert!(a.health_open);
    a.local_health = Some(healthy_snapshot());
    stage7_frame(&mut a, |a, _, _| { let ctx = a.ctx.clone(); a.health_window(&ctx); });
    assert!(a.health_open);
}

#[test]
fn ui_draw_health_window_closes_when_neither_server_nor_client() {
    let mut a = test_app();
    a.health_open = true;
    stage7_frame(&mut a, |a, _, _| { let ctx = a.ctx.clone(); a.health_window(&ctx); });
    assert!(!a.health_open);
}

#[test]
fn ui_draw_alarm_on_connection_loss_and_reconnect_countdown() {
    let mut a = test_app();
    a.lost = Some("сетевой кабель отключён".into());
    a.reconnect = Some((3, Instant::now()));
    a.cfg.servers.push(saved_server("Резерв", "10.0.0.3", "14999", "pwd"));
    a.cfg.reserve = "10.0.0.3:14999".into();
    stage7_frame(&mut a, |a, _, _| { let ctx = a.ctx.clone(); a.alarm_bar(&ctx); });
    assert!(a.lost.is_some());
}

#[test]
fn ui_draw_reserve_editor_with_invalid_and_valid_port() {
    let mut a = test_app();
    a.reserve_form = Some(saved_server("Тест", "10.0.0.2", "oops", "p"));
    stage7_frame(&mut a, |a, _, _| { let ctx = a.ctx.clone(); a.reserve_window(&ctx); });
    assert!(a.reserve_form.is_some());
    a.reserve_form = Some(saved_server("Тест", "10.0.0.2", "15000", "p"));
    stage7_frame(&mut a, |a, _, _| { let ctx = a.ctx.clone(); a.reserve_window(&ctx); });
    assert!(a.reserve_form.is_some());
}

#[test]
fn ui_draw_help_window_and_version_mismatch_banners() {
    let mut a = test_app();
    a.help_open = true;
    stage7_frame(&mut a, |a, _, _| { let ctx = a.ctx.clone(); a.help_window(&ctx); });
    assert!(a.help_open);
    a.connected = true;
    a.got_state = true;
    a.remote.version = "1.0.0".into();
    stage7_frame(&mut a, |a, _, _| { let ctx = a.ctx.clone(); a.version_bar(&ctx); });
    a.remote.version = "99.0.0".into();
    stage7_frame(&mut a, |a, _, _| { let ctx = a.ctx.clone(); a.version_bar(&ctx); });
    assert!(!a.version_hidden);
}

#[test]
fn ui_draw_confirmation_dialog_without_clicking_buttons() {
    let mut a = test_app();
    a.confirm = Some(("Очистить список?".into(), "Очистить", Cmd::Clear));
    stage7_frame(&mut a, |a, _, _| { let ctx = a.ctx.clone(); a.confirm_window(&ctx); });
    assert!(a.confirm.is_some());
}

#[test]
fn ui_draw_media_labels_for_playing_paused_stopped_and_unstarted() {
    let mut a = test_app();
    a.connected = true;
    a.remote = stage7_remote_pdf(0, 5);
    let media = vec![
        stage7_media(1, 0, true),
        stage7_media(2, 1, false),
        stage7_media(3, 2, true),
        stage7_media(4, 3, false),
    ];
    stage7_frame(&mut a, |a, ui, _| a.media_list(ui, &media));
}

#[test]
fn ui_draw_zoom_panel_when_zooming_is_and_is_not_possible() {
    for zoom in [1.0, 1.5, 4.0] {
        let mut a = test_app();
        a.connected = true;
        a.remote = stage7_remote_pdf(1, 6);
        a.remote.zoom = zoom;
        a.remote.view = if zoom > 1.0 { [0.1, 0.1, 0.9, 0.9] } else { [0.0, 0.0, 1.0, 1.0] };
        stage7_frame(&mut a, |a, ui, state| a.zoom_panel(ui, state));
    }
}

#[test]
fn ui_draw_background_menu_when_default_or_custom_is_selected() {
    for current in ["", "stage.png"] {
        let mut a = test_app();
        stage7_frame(&mut a, |a, ui, _| a.background_menu(ui, current));
        assert!(a.confirm.is_none());
    }
}

#[test]
fn ui_draw_ppt_focus_info_for_unknown_lost_restoring_failed() {
    for focus in [
        PptFocus::Unknown,
        PptFocus::Ok,
        PptFocus::Lost { window: "другое окно".into() },
        PptFocus::Restoring { window: "другое окно".into() },
        PptFocus::RestoreFailed { window: "другое окно".into() },
    ] {
        let mut a = test_app();
        a.connected = true;
        a.remote = stage7_remote_pdf(0, 10);
        a.remote.items[0].kind = "PPTX".into();
        a.remote.ppt_focus = focus;
        stage7_frame(&mut a, |a, _, state| { let ctx = a.ctx.clone(); a.ppt_focus_bar(&ctx, state); });
    }
}

fn stage7_key_commands(view: &StateMsg, keys: &[egui::Key], modifiers: egui::Modifiers) -> Vec<Cmd> {
    let ctx = egui::Context::default();
    let mut raw = egui::RawInput::default();
    raw.modifiers = modifiers;
    raw.events = keys.iter().map(|&key| egui::Event::Key {
        key, physical_key: None, pressed: true, repeat: false, modifiers,
    }).collect();
    let mut cmds = Vec::new();
    let _ = ctx.run(raw, |ctx| cmds = nav_keys(ctx, view));
    cmds
}

#[test]
fn ui_keyboard_navigation_next_prev_zoom_and_home_end() {
    let state = stage7_remote_pdf(3, 10);
    let keys = [
        egui::Key::ArrowRight, egui::Key::ArrowLeft,
        egui::Key::Plus, egui::Key::Minus,
        egui::Key::Home, egui::Key::End, egui::Key::Space,
    ];
    let cmds = stage7_key_commands(&state, &keys, egui::Modifiers::NONE);
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Next)));
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Prev)));
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Zoom { step: 1 })));
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Zoom { step: -1 })));
    assert!(cmds.iter().any(|c| matches!(c, Cmd::GoTo { page: 0 })));
    assert!(cmds.iter().any(|c| matches!(c, Cmd::GoTo { page: usize::MAX })));
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Play)));
}

#[test]
fn ui_keyboard_ctrl_arrows_pan_instead_of_changing_slides() {
    let state = stage7_remote_pdf(2, 6);
    let mods = egui::Modifiers { ctrl: true, ..Default::default() };
    let cmds = stage7_key_commands(&state, &[
        egui::Key::ArrowLeft, egui::Key::ArrowRight,
        egui::Key::ArrowUp, egui::Key::ArrowDown,
    ], mods);
    assert_eq!(cmds.len(), 4);
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Pan { dx: -1, dy: 0 })));
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Pan { dx: 1, dy: 0 })));
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Pan { dx: 0, dy: -1 })));
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Pan { dx: 0, dy: 1 })));
    assert!(!cmds.iter().any(|c| matches!(c, Cmd::Next | Cmd::Prev)));
}

#[test]
fn ui_keyboard_shift_number_opens_only_ready_item_and_shift_zero_closes() {
    let mut state = stage7_remote_pdf(0, 5);
    state.items.push(ItemInfo { id: 20, name: "loading".into(), ready: false, ..Default::default() });
    let mods = egui::Modifiers { shift: true, ..Default::default() };
    let cmds = stage7_key_commands(&state, &[
        egui::Key::Num1, egui::Key::Num2, egui::Key::Num0,
    ], mods);
    assert_eq!(cmds.len(), 2);
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Open { id: 10 })));
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Close)));
}

#[test]
fn ui_keyboard_timer_space_toggles_only_when_not_showing_slides() {
    let mut state = StateMsg::default();
    state.screen = Screen::Timer;
    let cmds = stage7_key_commands(&state, &[egui::Key::Space], egui::Modifiers::NONE);
    assert!(matches!(cmds.as_slice(), [Cmd::TimerRun { run: true }]));
    state.timer.running = true;
    let cmds = stage7_key_commands(&state, &[egui::Key::Space], egui::Modifiers::NONE);
    assert!(matches!(cmds.as_slice(), [Cmd::TimerRun { run: false }]));
    state.screen = Screen::Clock;
    assert!(stage7_key_commands(&state, &[egui::Key::Space], egui::Modifiers::NONE).is_empty());
}

#[test]
fn ui_keyboard_without_active_show_ignores_slide_arrows_and_esc() {
    let state = StateMsg::default();
    let cmds = stage7_key_commands(&state, &[
        egui::Key::ArrowRight, egui::Key::ArrowLeft,
        egui::Key::Escape, egui::Key::Space,
    ], egui::Modifiers::NONE);
    assert!(cmds.is_empty());
}

#[test]
fn ui_typing_digits_ignores_modifiers_and_detects_enter() {
    for (mods, expected) in [
        (egui::Modifiers::NONE, "123"),
        (egui::Modifiers { shift: true, ..Default::default() }, ""),
        (egui::Modifiers { ctrl: true, ..Default::default() }, ""),
        (egui::Modifiers { alt: true, ..Default::default() }, ""),
    ] {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.modifiers = mods;
        input.events = vec![
            egui::Event::Text("a1b2c3".into()),
            egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: mods },
        ];
        let mut actual = (String::new(), false);
        let _ = ctx.run(input, |ctx| actual = typed_digits(ctx));
        assert_eq!(actual.0, expected);
        assert!(actual.1);
    }
}

#[test]
fn ui_draw_settings_panel_without_changing_persistent_configuration() {
    let mut a = test_app();
    let original = (a.cfg.windowed, a.cfg.monitor.clone(), a.cfg.pdf_engine.clone());
    stage7_frame(&mut a, |a, ui, _| a.settings_panel(ui));
    assert_eq!((a.cfg.windowed, a.cfg.monitor.clone(), a.cfg.pdf_engine.clone()), original);
}

#[test]
fn ui_draw_settings_with_missing_selected_monitor_and_keyboard_enabled() {
    let mut a = test_app();
    a.cfg.monitor = "\\\\.\\UNPLUGGED_DISPLAY".into();
    a.cfg.keyboard = true;
    stage7_frame(&mut a, |a, ui, _| a.settings_panel(ui));
    assert!(a.cfg.keyboard);
}

#[test]
fn ui_draw_health_warning_when_remote_server_is_overloaded() {
    let mut a = test_app();
    a.connected = true;
    a.remote_live = true;
    a.last_seen = Some(Instant::now());
    a.remote_health = Some(health::HealthInfo {
        cpu: 99.0, disk_free_mb: 300, ..healthy_snapshot()
    });
    stage7_frame(&mut a, |a, _, _| { let ctx = a.ctx.clone(); a.alarm_bar(&ctx); });
    assert!(a.connected);
}

#[test]
fn ui_monitor_label_marks_primary_display_and_dimensions() {
    let main = display::Monitor {
        device: "DISPLAY_PRIMARY".into(), rect: (0, 0, 1920, 1080), primary: true,
    };
    let second = display::Monitor {
        device: "DISPLAY_OTHER".into(), rect: (1920, 0, 2560, 1440), primary: false,
    };
    let p = main.label(1);
    let s = second.label(2);
    assert!(p.contains("1920×1080") && p.contains("основной"));
    assert!(s.contains("2560×1440") && !s.contains("основной"));
}

#[test]
fn command_move_reorders_without_losing_items_or_ids() {
    let mut a = test_app();
    for id in [10, 20, 30] { a.items.push(pdf_item(id, 3)); }
    a.apply(Cmd::Move { id: 20, up: true });
    assert_eq!(a.items.iter().map(|it| it.id).collect::<Vec<_>>(), vec![20, 10, 30]);
    a.apply(Cmd::Move { id: 20, up: false });
    assert_eq!(a.items.iter().map(|it| it.id).collect::<Vec<_>>(), vec![10, 20, 30]);
}

#[test]
fn command_move_ignores_first_last_and_missing_items() {
    let mut a = test_app();
    for id in [10, 20] { a.items.push(pdf_item(id, 3)); }
    for cmd in [Cmd::Move { id: 10, up: true }, Cmd::Move { id: 20, up: false }, Cmd::Move { id: 404, up: true }] {
        a.apply(cmd);
    }
    assert_eq!(a.items.iter().map(|it| it.id).collect::<Vec<_>>(), vec![10, 20]);
}

#[test]
fn command_removing_active_item_closes_show_but_keeps_other_items() {
    let mut a = test_app();
    open_pdf(&mut a, 10, 6);
    a.items.push(pdf_item(20, 3));
    a.apply(Cmd::Remove { id: 10 });
    assert!(a.show.is_none());
    assert_eq!(a.items.len(), 1);
    assert_eq!(a.items[0].id, 20);
    assert!(a.stage);
}

#[test]
fn command_removing_unrelated_item_preserves_active_show() {
    let mut a = test_app();
    open_pdf(&mut a, 10, 6);
    a.items.push(pdf_item(20, 3));
    a.apply(Cmd::Remove { id: 20 });
    assert_eq!(a.show.as_ref().unwrap().id, 10);
    a.apply(Cmd::Remove { id: 404 });
    assert_eq!(a.items.len(), 1);
}

#[test]
fn command_clear_empties_list_and_active_show_without_hiding_stage() {
    let mut a = test_app();
    open_pdf(&mut a, 10, 8);
    a.items.push(pdf_item(20, 4));
    a.apply(Cmd::Clear);
    assert!(a.items.is_empty());
    assert!(a.show.is_none());
    assert!(a.stage);
}

#[test]
fn command_audio_volume_clamps_without_needing_audio_engine() {
    let mut a = test_app();
    a.apply(Cmd::AudioVolume { volume: 200 });
    assert_eq!(a.player.volume, 100);
    assert_eq!(a.cfg.audio_volume, 100);
    a.apply(Cmd::AudioVolume { volume: 27 });
    assert_eq!(a.player.volume, 27);
    assert_eq!(a.cfg.audio_volume, 27);
}

#[test]
fn command_audio_playlist_reordering_and_removal_work_without_device() {
    let mut a = test_app();
    a.player.add(PathBuf::from("not_owned_by_test.mp3"), "First".into());
    a.player.add(PathBuf::from("not_owned_by_test2.mp3"), "Second".into());
    a.apply(Cmd::AudioMove { id: 2, up: true });
    assert_eq!(a.player.tracks[0].name, "Second");
    a.apply(Cmd::AudioRemove { id: 2 });
    assert_eq!(a.player.tracks.len(), 1);
    a.apply(Cmd::AudioClear);
    assert!(a.player.tracks.is_empty());
}

#[test]
fn command_audio_transport_is_harmless_with_empty_playlist() {
    let mut a = test_app();
    for cmd in [
        Cmd::AudioStop, Cmd::AudioToggle, Cmd::AudioNext, Cmd::AudioPrev,
        Cmd::AudioEndInsert, Cmd::AudioSeek { secs: 65 },
        Cmd::AudioPlay { id: 999 }, Cmd::AudioInsert { id: 999 },
    ] {
        a.apply(cmd);
    }
    assert!(a.player.tracks.is_empty());
    assert_eq!(a.player.info().state, AudioState::Stopped);
}

#[test]
fn command_timer_set_run_stop_reset_and_clamp_are_consistent() {
    let mut a = test_app();
    a.apply(Cmd::TimerSet { countdown: true, secs: 0 });
    assert_eq!(a.timer.info().total, 1);
    a.apply(Cmd::TimerSet { countdown: true, secs: u32::MAX });
    assert_eq!(a.timer.info().total, 99 * 3600 + 59 * 60 + 59);
    a.apply(Cmd::TimerRun { run: true });
    assert!(a.timer.info().running);
    a.apply(Cmd::TimerRun { run: false });
    assert!(!a.timer.info().running);
    a.apply(Cmd::TimerReset);
    assert_eq!(a.timer.info().elapsed, 0);
}

#[test]
fn command_timer_switching_modes_resets_elapsed_and_keeps_duration() {
    let mut a = test_app();
    a.apply(Cmd::TimerSet { countdown: false, secs: 300 });
    assert!(!a.timer.info().countdown);
    assert_eq!(a.timer.info().total, 300);
    a.apply(Cmd::TimerRun { run: true });
    a.apply(Cmd::TimerSet { countdown: true, secs: 120 });
    let state = a.timer.info();
    assert!(state.countdown);
    assert_eq!(state.total, 120);
    assert_eq!(state.elapsed, 0);
    assert!(!state.running);
}

// Stage 8: drive the real event dispatcher with synthetic, in-memory events.
// No COM bridge, audio device, native window, video decoding, or network port.
fn stage8_ppt(app: &mut App, id: u64) {
    app.items.push(Item { kind: "PPTX".into(), pdf: None, slides: None, ..pdf_item(id, 6) });
    app.show_gen += 1;
    app.show = Some(Show {
        id, page: 0, pages: 6, gen: app.show_gen,
        kind: ShowKind::Ppt(PptShow {
            phase: PptPhase::Loading, hwnd: 0, zoom_img: None,
            zoom_busy: None, zoom_up: false, media: (0, vec![]),
        }),
        opened: Instant::now(), zoom: 1.0, center: (0.5, 0.5),
    });
}

fn stage8_video(app: &mut App, id: u64) {
    app.show_gen += 1;
    app.show = Some(Show {
        id, page: 0, pages: 1, gen: app.show_gen,
        kind: ShowKind::Video(VideoShow {
            player: video::mock_for_tests(),
            state: VideoState::Loading, pos: 0.0, dur: 0.0,
        }),
        opened: Instant::now(), zoom: 1.0, center: (0.5, 0.5),
    });
}

fn stage8_img() -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([2, 2], &[
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
    ])
}

fn stage8_out(app: &mut App, msg: Out) {
    app.out_tx.send(msg).unwrap();
    let ctx = app.ctx.clone();
    app.poll(&ctx);
}

fn stage8_client(app: &mut App, ev: CliEvent) {
    app.cli_tx.send(ev).unwrap();
    let ctx = app.ctx.clone();
    app.poll(&ctx);
}

fn stage8_server(app: &mut App, ev: SrvEvent) {
    app.srv_tx.send(ev).unwrap();
    let ctx = app.ctx.clone();
    app.poll(&ctx);
}

fn stage8_ppt_info(app: &App) -> &PptShow {
    match &app.show.as_ref().unwrap().kind {
        ShowKind::Ppt(p) => p,
        _ => panic!("expected PPT show"),
    }
}

#[test]
fn stage8_ppt_ready_updates_phase_handle_and_slide_count() {
    let mut a = test_app();
    stage8_ppt(&mut a, 7);
    a.status = "loading".into();
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Ppt { gen, ev: ppt::PptEvent::Ready(12, 99) });
    assert_eq!(a.show.as_ref().unwrap().pages, 12);
    assert!(matches!(stage8_ppt_info(&a).phase, PptPhase::Ready(_)));
    assert_eq!(stage8_ppt_info(&a).hwnd, 99);
    assert!(a.status.is_empty());
}

#[test]
fn stage8_ppt_ready_sanitizes_zero_slide_count() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Ppt { gen, ev: ppt::PptEvent::Ready(0, 0) });
    assert_eq!(a.show.as_ref().unwrap().pages, 1);
}

#[test]
fn stage8_ppt_position_resets_zoom_only_on_page_change() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    a.show.as_mut().unwrap().page = 2;
    a.show.as_mut().unwrap().zoom = 2.0;
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Ppt { gen, ev: ppt::PptEvent::Pos(3, 8) });
    assert_eq!(a.show.as_ref().unwrap().zoom, 2.0);
    stage8_out(&mut a, Out::Ppt { gen, ev: ppt::PptEvent::Pos(4, 8) });
    assert_eq!(a.show.as_ref().unwrap().page, 3);
    assert_eq!(a.show.as_ref().unwrap().zoom, 1.0);
    assert_eq!(a.show.as_ref().unwrap().pages, 8);
}

#[test]
fn stage8_ppt_zero_based_position_never_underflows() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Ppt { gen, ev: ppt::PptEvent::Pos(0, 0) });
    assert_eq!((a.show.as_ref().unwrap().page, a.show.as_ref().unwrap().pages), (0, 1));
}

#[test]
fn stage8_ppt_stale_generation_does_not_change_newer_show() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    let stale = a.show_gen + 2;
    stage8_out(&mut a, Out::Ppt { gen: stale, ev: ppt::PptEvent::Error("old".into()) });
    assert!(a.show.is_some());
    assert!(!a.status.contains("old"));
}

#[test]
fn stage8_ppt_slide_event_clears_only_matching_failed_thumbnail() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    a.thumb_failed.insert((7, 1)); a.thumb_failed.insert((7, 3));
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Ppt { gen, ev: ppt::PptEvent::Slide(2) });
    assert!(!a.thumb_failed.contains(&(7, 1)));
    assert!(a.thumb_failed.contains(&(7, 3)));
}

#[test]
fn stage8_ppt_media_event_reaches_current_slide() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    let gen = a.show_gen;
    let media = ppt::Media { id: 18, name: "sound".into(), video: false, state: 3,
        pos: 0, dur: 42, rect: [0.0, 0.0, 1.0, 1.0] };
    stage8_out(&mut a, Out::Ppt { gen, ev: ppt::PptEvent::Media(1, vec![media]) });
    assert_eq!(a.slide_media().len(), 1);
    assert_eq!(a.slide_media()[0].id, 18);
    a.show.as_mut().unwrap().page = 1;
    assert!(a.slide_media().is_empty());
}

#[test]
fn stage8_ppt_zoom_failure_clears_busy_and_reports_reason() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    if let ShowKind::Ppt(p) = &mut a.show.as_mut().unwrap().kind { p.zoom_busy = Some(2); }
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Ppt { gen, ev: ppt::PptEvent::ZoomFailed("disk full".into()) });
    assert_eq!(stage8_ppt_info(&a).zoom_busy, None);
    assert!(a.status.contains("disk full"));
}

#[test]
fn stage8_ppt_zoom_texture_matches_generation() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    let gen = a.show_gen;
    stage8_out(&mut a, Out::PptZoom { gen: gen + 1, page: 0, image: Ok(stage8_img()) });
    assert!(stage8_ppt_info(&a).zoom_img.is_none());
    stage8_out(&mut a, Out::PptZoom { gen, page: 0, image: Ok(stage8_img()) });
    assert_eq!(stage8_ppt_info(&a).zoom_img.as_ref().unwrap().0, 0);
}

#[test]
fn stage8_ppt_zoom_read_error_keeps_show_open() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    let gen = a.show_gen;
    if let ShowKind::Ppt(p) = &mut a.show.as_mut().unwrap().kind { p.zoom_busy = Some(0); }
    stage8_out(&mut a, Out::PptZoom { gen, page: 0, image: Err("bad jpg".into()) });
    assert!(a.show.is_some());
    assert!(a.status.contains("bad jpg"));
    assert_eq!(stage8_ppt_info(&a).zoom_busy, None);
}

#[test]
fn stage8_ppt_error_closes_show_and_preserves_error_message() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Ppt { gen, ev: ppt::PptEvent::Error("COM unavailable".into()) });
    assert!(a.show.is_none());
    assert!(a.status.contains("COM unavailable"));
}

#[test]
fn stage8_ppt_ended_closes_show_without_touching_items() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Ppt { gen, ev: ppt::PptEvent::Ended });
    assert!(a.show.is_none());
    assert_eq!(a.items.len(), 1);
}

#[test]
fn stage8_video_ready_transitions_to_armed() {
    let mut a = test_app(); stage8_video(&mut a, 22);
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Video { gen, ev: video::VideoEvent::Ready(85.0) });
    let v = a.local_state().video.unwrap();
    assert_eq!(v.state, VideoState::Armed);
    assert_eq!(v.dur, 85);
}

#[test]
fn stage8_video_loading_ignores_playing_flag_until_armed() {
    let mut a = test_app(); stage8_video(&mut a, 22);
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Video { gen, ev: video::VideoEvent::Pos { pos: 4.0, dur: 40.0, playing: true } });
    assert_eq!(a.local_state().video.unwrap().state, VideoState::Loading);
}

#[test]
fn stage8_video_playing_and_paused_follow_position_events() {
    let mut a = test_app(); stage8_video(&mut a, 22);
    let gen = a.show_gen;
    a.apply(Cmd::Play); // loading: ignored
    stage8_out(&mut a, Out::Video { gen, ev: video::VideoEvent::Ready(100.0) });
    a.apply(Cmd::Play); // armed: starts playback without a native player
    stage8_out(&mut a, Out::Video { gen, ev: video::VideoEvent::Pos { pos: 25.5, dur: 100.0, playing: false } });
    assert_eq!(a.local_state().video.unwrap().state, VideoState::Paused);
    stage8_out(&mut a, Out::Video { gen, ev: video::VideoEvent::Pos { pos: 26.0, dur: 100.0, playing: true } });
    let v = a.local_state().video.unwrap();
    assert_eq!(v.state, VideoState::Playing);
    assert_eq!(v.pos, 26);
}

#[test]
fn stage8_video_end_arms_again_without_closing_file() {
    let mut a = test_app(); stage8_video(&mut a, 22);
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Video { gen, ev: video::VideoEvent::Ready(30.0) });
    a.apply(Cmd::Play);
    stage8_out(&mut a, Out::Video { gen, ev: video::VideoEvent::Ended });
    let v = a.local_state().video.unwrap();
    assert_eq!(v.state, VideoState::Armed);
    assert_eq!(v.pos, 0);
    assert!(a.status.contains("готово"));
    assert!(a.show.is_some());
}

#[test]
fn stage8_video_error_closes_only_current_generation() {
    let mut a = test_app(); stage8_video(&mut a, 22);
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Video { gen: gen + 1, ev: video::VideoEvent::Error("stale".into()) });
    assert!(a.show.is_some());
    stage8_out(&mut a, Out::Video { gen, ev: video::VideoEvent::Error("decoder failed".into()) });
    assert!(a.show.is_none());
    assert!(a.status.contains("decoder failed"));
}

#[test]
fn stage8_video_events_do_not_turn_pdf_into_video() {
    let mut a = test_app(); open_pdf(&mut a, 22, 4);
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Video { gen, ev: video::VideoEvent::Ready(100.0) });
    assert!(a.local_state().video.is_none());
    assert_eq!(a.local_state().pages, 4);
}

#[test]
fn stage8_image_event_uploads_only_current_show() {
    let mut a = test_app();
    a.show_gen += 1;
    a.show = Some(Show { id: 2, page: 0, pages: 1, gen: a.show_gen,
        kind: ShowKind::Image(None), opened: Instant::now(), zoom: 1.0, center: (0.5, 0.5) });
    let gen = a.show_gen;
    let frames = vec![(Arc::new(stage8_img()), Duration::ZERO)];
    stage8_out(&mut a, Out::Image { gen: gen + 1, result: Ok(frames.clone()) });
    assert!(matches!(&a.show.as_ref().unwrap().kind, ShowKind::Image(None)));
    stage8_out(&mut a, Out::Image { gen, result: Ok(frames) });
    assert!(matches!(&a.show.as_ref().unwrap().kind, ShowKind::Image(Some(_))));
}

#[test]
fn stage8_image_failure_preserves_show_and_reports_error() {
    let mut a = test_app();
    a.show_gen += 1;
    a.show = Some(Show { id: 2, page: 0, pages: 1, gen: a.show_gen,
        kind: ShowKind::Image(None), opened: Instant::now(), zoom: 1.0, center: (0.5, 0.5) });
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Image { gen, result: Err("corrupt image".into()) });
    assert!(a.status.contains("corrupt image"));
    assert!(a.show.is_some());
}

#[test]
fn stage8_background_accepts_current_generation_and_ignores_old() {
    let mut a = test_app(); a.bg_gen = 4;
    let frames = vec![(Arc::new(stage8_img()), Duration::ZERO)];
    stage8_out(&mut a, Out::Background { gen: 3, result: Ok(frames.clone()) });
    assert!(a.bg.is_none());
    stage8_out(&mut a, Out::Background { gen: 4, result: Ok(frames) });
    assert!(a.bg.is_some());
    stage8_out(&mut a, Out::Background { gen: 4, result: Err("not found".into()) });
    assert!(a.status.contains("not found"));
}

#[test]
fn stage8_page_event_populates_only_current_pdf_cache() {
    let mut a = test_app(); open_pdf(&mut a, 9, 20);
    let key = PageKey { id: 9, page: 0, w: 100, h: 100, thumb: false, view: FULL_VIEW };
    stage8_out(&mut a, Out::Page { key, image: stage8_img() });
    assert!(a.textures.contains_key(&key));
    let old_key = PageKey { page: 12, ..key };
    stage8_out(&mut a, Out::Page { key: old_key, image: stage8_img() });
    assert!(!a.textures.contains_key(&old_key));
}

#[test]
fn stage8_page_error_reports_only_active_page() {
    let mut a = test_app(); open_pdf(&mut a, 9, 20);
    let key = PageKey { id: 9, page: 1, w: 100, h: 100, thumb: false, view: FULL_VIEW };
    stage8_out(&mut a, Out::PageErr { key, msg: "other page".into() });
    assert!(a.failed.contains(&key));
    assert!(a.render_error.is_none());
    let current = PageKey { page: 0, ..key };
    stage8_out(&mut a, Out::PageErr { key: current, msg: "page failure".into() });
    assert_eq!(a.render_error.as_deref(), Some("page failure"));
}

#[test]
fn stage8_thumbnail_near_current_slide_is_retained() {
    let mut a = test_app(); open_pdf(&mut a, 9, 20);
    a.thumb_busy.insert((9, 1));
    stage8_out(&mut a, Out::Thumb { id: 9, page: 1, image: stage8_img(), jpg: vec![1, 2, 3] });
    assert!(!a.thumb_busy.contains(&(9, 1)));
    assert!(a.previews.contains_key(&(9, 1)));
    assert_eq!(a.thumb_jpg.get(&(9, 1)).unwrap().as_ref(), &vec![1, 2, 3]);
}

#[test]
fn stage8_thumbnail_far_from_current_slide_is_not_cached() {
    let mut a = test_app(); open_pdf(&mut a, 9, 20);
    stage8_out(&mut a, Out::Thumb { id: 9, page: 12, image: stage8_img(), jpg: vec![1] });
    assert!(!a.previews.contains_key(&(9, 12)));
}

#[test]
fn stage8_thumbnail_failure_clears_pending_but_remembers_error() {
    let mut a = test_app(); a.thumb_busy.insert((9, 4));
    stage8_out(&mut a, Out::ThumbErr { id: 9, page: 4 });
    assert!(!a.thumb_busy.contains(&(9, 4)));
    assert!(a.thumb_failed.contains(&(9, 4)));
}

#[test]
fn stage8_health_event_replaces_local_snapshot() {
    let mut a = test_app();
    let h = health::HealthInfo { cpu: 71.0, monitors: vec!["screen".into()], ..Default::default() };
    stage8_out(&mut a, Out::Health(h));
    assert_eq!(a.local_health.as_ref().unwrap().cpu, 71.0);
}

#[test]
fn stage8_prepared_event_marks_file_ready_and_keeps_page_count() {
    let mut a = test_app(); a.items.push(Item { status: Status::Preparing, ..pdf_item(12, 0) });
    stage8_out(&mut a, Out::Prepared { id: 12, result: Ok((Some(PathBuf::from("ready.pdf")), 23)), linked: vec![] });
    assert_eq!(a.items[0].pages, 23);
    assert!(matches!(a.items[0].status, Status::Ready));
    assert_eq!(a.items[0].pdf.as_deref(), Some(Path::new("ready.pdf")));
}

#[test]
fn stage8_prepared_failure_and_missing_links_produce_visible_errors() {
    let mut a = test_app(); a.items.push(Item { status: Status::Preparing, ..pdf_item(12, 0) });
    stage8_out(&mut a, Out::Prepared { id: 12, result: Err("damaged".into()), linked: vec!["a.mp4".into()] });
    assert!(matches!(&a.items[0].status, Status::Error(s) if s == "damaged"));
    assert!(a.status.contains("a.mp4"));
}

#[test]
fn stage8_prepared_event_for_deleted_file_is_ignored() {
    let mut a = test_app();
    stage8_out(&mut a, Out::Prepared { id: 999, result: Ok((None, 55)), linked: vec![] });
    assert!(a.items.is_empty());
}

#[test]
fn stage8_ppt_tick_times_out_stuck_loading_without_opening_com() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    a.show.as_mut().unwrap().opened = Instant::now() - PPT_TIMEOUT - Duration::from_secs(1);
    let ctx = a.ctx.clone(); a.tick_ppt(&ctx);
    assert!(a.show.is_none());
    assert!(a.status.contains("не отвечает"));
}

#[test]
fn stage8_ppt_tick_waits_for_settle_then_lowers_scene() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    a.show.as_mut().unwrap().opened = Instant::now() - SWITCH_BG - Duration::from_secs(1);
    if let ShowKind::Ppt(p) = &mut a.show.as_mut().unwrap().kind {
        p.phase = PptPhase::Ready(Instant::now() - PPT_SETTLE - Duration::from_secs(1));
    }
    let ctx = a.ctx.clone(); a.tick_ppt(&ctx);
    assert!(matches!(stage8_ppt_info(&a).phase, PptPhase::Lowered(_)));
}

#[test]
fn stage8_ppt_focus_poll_without_valid_window_resets_focus_state() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    a.ppt_focus = PptFocus::Lost { window: "other".into() };
    a.ppt_focus_candidate = Some(123);
    // Без ожидания интервала метод только планирует следующий опрос.
    a.ppt_focus_polled = Instant::now() - Duration::from_secs(1);
    let ctx = a.ctx.clone(); a.tick_ppt_focus(&ctx);
    assert!(a.ppt_focus_candidate.is_none());
    assert_eq!(a.ppt_focus, PptFocus::Unknown);
    assert!(a.ppt_focus_restore_deadline.is_none());
}

#[test]
fn stage8_ppt_focus_expired_restore_reports_failure_without_valid_window() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    a.ppt_focus = PptFocus::Restoring { window: "other".into() };
    a.ppt_focus_restore_deadline = Some(Instant::now() - Duration::from_secs(1));
    a.ppt_focus_polled = Instant::now() - Duration::from_secs(1);
    let ctx = a.ctx.clone(); a.tick_ppt_focus(&ctx);
    assert_eq!(a.ppt_focus, PptFocus::RestoreFailed { window: "other".into() });
    assert!(a.ppt_focus_restore_deadline.is_none());
}

#[test]
fn stage8_ppt_zoom_filename_uses_one_based_slide_and_subfolder() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    a.items[0].slides = Some(PathBuf::from("slides_test"));
    assert_eq!(a.ppt_zoom_file(3), Some(PathBuf::from("slides_test").join("zoom_4.jpg")));
}

#[test]
fn stage8_ppt_zoom_requests_image_only_once_while_busy() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    a.items[0].slides = Some(std::env::temp_dir().join(format!("stage8_no_zoom_{}", net::unique_prefix())));
    a.show.as_mut().unwrap().zoom = 2.0;
    if let ShowKind::Ppt(p) = &mut a.show.as_mut().unwrap().kind { p.phase = PptPhase::Shown; }
    a.tick_ppt_zoom();
    assert_eq!(stage8_ppt_info(&a).zoom_busy, Some(0));
    a.tick_ppt_zoom();
    assert_eq!(stage8_ppt_info(&a).zoom_busy, Some(0));
}

#[test]
fn stage8_client_rtt_and_health_restore_live_telemetry() {
    let mut a = test_app();
    stage8_client(&mut a, CliEvent::Rtt(88));
    assert_eq!(a.rtt, Some(88)); assert!(a.remote_live);
    stage8_client(&mut a, CliEvent::Health(health::HealthInfo { cpu: 12.0, ..Default::default() }));
    assert_eq!(a.remote_health.as_ref().unwrap().cpu, 12.0);
    assert!(a.health_at.is_some());
}

#[test]
fn stage8_client_replies_update_status_for_upload_success_and_error() {
    let mut a = test_app();
    stage8_client(&mut a, CliEvent::UploadOk("slides.pdf".into()));
    assert!(a.status.contains("slides.pdf"));
    stage8_client(&mut a, CliEvent::UploadErr { name: "slides.pdf".into(), msg: "denied".into() });
    assert!(a.status.contains("denied"));
}

#[test]
fn stage8_client_upload_progress_handles_empty_and_nonempty_totals() {
    let mut a = test_app();
    stage8_client(&mut a, CliEvent::Progress { name: "x.zip".into(), sent: 1, total: 4 });
    assert!(a.status.contains("25%"));
    stage8_client(&mut a, CliEvent::Progress { name: "x.zip".into(), sent: 0, total: 0 });
    assert!(a.status.contains("100%"));
    assert!(a.last_seen.is_some());
}

#[test]
fn stage8_client_state_updates_active_item_and_last_show() {
    let mut a = test_app(); a.connected = true;
    let remote = StateMsg {
        items: vec![ItemInfo { id: 51, name: "music.pdf".into(), kind: "PDF".into(), ready: true, ..Default::default() }],
        active: Some(51), page: 4, message: "remote message".into(),
        version: env!("CARGO_PKG_VERSION").into(), ..Default::default()
    };
    stage8_client(&mut a, CliEvent::State(remote));
    assert!(a.got_state);
    assert_eq!(a.last_show, Some(("music.pdf".into(), 4)));
    assert_eq!(a.status, "remote message");
}

#[test]
fn stage8_client_close_after_connection_schedules_reconnect() {
    let mut a = test_app(); a.connected = true;
    stage8_client(&mut a, CliEvent::Closed("connection lost".into()));
    assert!(!a.connected);
    assert!(a.lost.is_some());
    assert!(a.reconnect.is_some());
    assert!(a.remote.items.is_empty());
    assert!(!a.remote_live);
}

#[test]
fn stage8_client_bad_password_disables_retries() {
    let mut a = test_app(); a.connected = true;
    stage8_client(&mut a, CliEvent::Closed("Неверный пароль".into()));
    assert!(a.reconnect.is_none());
    assert_eq!(a.net_status, "Неверный пароль");
}

#[test]
fn stage8_client_voluntary_disconnect_leaves_no_alarm() {
    let mut a = test_app(); a.connected = true;
    stage8_client(&mut a, CliEvent::Closed(String::new()));
    assert!(a.lost.is_none());
    assert!(a.reconnect.is_none());
    assert_eq!(a.net_status, "Отключено");
}

#[test]
fn stage8_client_thumbnail_is_accepted_only_for_current_remote_show() {
    let mut a = test_app(); a.connected = true;
    a.remote.active = Some(7);
    stage8_client(&mut a, CliEvent::Thumb { id: 8, page: 0, image: stage8_img() });
    assert!(a.previews.is_empty());
    stage8_client(&mut a, CliEvent::Thumb { id: 7, page: 1, image: stage8_img() });
    assert!(a.previews.contains_key(&(7, 1)));
}

#[test]
fn stage8_server_failed_and_client_count_are_reported() {
    let mut a = test_app();
    stage8_server(&mut a, SrvEvent::Clients(4));
    assert_eq!(a.clients, 4);
    stage8_server(&mut a, SrvEvent::Failed("occupied port".into()));
    assert!(!a.server_online);
    assert!(a.net_status.contains("occupied port"));
}

#[test]
fn stage8_server_detects_old_protocol_client() {
    let mut a = test_app();
    stage8_server(&mut a, SrvEvent::Hello { version: String::new() });
    assert!(a.status.contains("обновите"));
    a.status.clear();
    stage8_server(&mut a, SrvEvent::Hello { version: env!("CARGO_PKG_VERSION").into() });
    assert!(a.status.is_empty());
}

#[test]
fn stage8_server_does_not_execute_commands_without_server() {
    let mut a = test_app(); a.items.push(pdf_item(1, 3));
    stage8_server(&mut a, SrvEvent::Cmd(Cmd::Clear));
    assert_eq!(a.items.len(), 1);
}

#[test]
fn stage8_mirror_upload_success_and_error_have_distinct_messages() {
    let mut a = test_app();
    a.mirror_tx.send(CliEvent::UploadOk("x.pdf".into())).unwrap();
    let ctx = a.ctx.clone(); a.poll(&ctx);
    assert!(a.status.contains("резервный сервер"));
    a.mirror_tx.send(CliEvent::UploadErr { name: "x.pdf".into(), msg: "disk full".into() }).unwrap();
    a.poll(&ctx);
    assert!(a.status.contains("disk full"));
}

#[test]
fn stage8_trim_pages_retains_only_adjacent_cached_pages() {
    let mut a = test_app();
    for page in 0..12 {
        let key = PageKey { id: 1, page, w: 3, h: 3, thumb: false, view: FULL_VIEW };
        a.textures.insert(key, a.ctx.load_texture(format!("stage8_{page}"), stage8_img(), egui::TextureOptions::LINEAR));
    }
    a.trim_pages(1, 5);
    let remaining: Vec<usize> = a.textures.keys().map(|k| k.page).collect();
    assert!(remaining.iter().all(|&p| p + 2 >= 5 && p <= 5 + PREFETCH_AHEAD));
    assert!(!remaining.is_empty());
}

#[test]
fn stage8_reset_pages_clears_cache_failures_and_render_error() {
    let mut a = test_app();
    let key = PageKey { id: 1, page: 2, w: 3, h: 3, thumb: false, view: FULL_VIEW };
    a.failed.insert(key); a.render_error = Some("failure".into());
    a.textures.insert(key, a.ctx.load_texture("stage8_reset", stage8_img(), egui::TextureOptions::LINEAR));
    a.reset_pages(true);
    assert!(a.failed.is_empty()); assert!(a.render_error.is_none()); assert!(a.textures.is_empty());
}

#[test]
fn stage8_add_local_routes_music_and_rejects_unsupported_formats() {
    let mut a = test_app(); let ctx = a.ctx.clone();
    a.add_local(PathBuf::from("song.flac"), Some("track".into()), &ctx);
    assert!(a.items.is_empty());
    assert_eq!(a.player.tracks[0].name, "track");
    a.add_local(PathBuf::from("script.exe"), None, &ctx);
    assert!(a.status.contains("не поддерживается"));
    assert!(a.items.is_empty());
}

#[test]
fn stage8_file_paths_include_normalized_port_and_hash_slides() {
    let mut a = test_app(); a.cfg.port = " 9201 ".into();
    assert!(a.list_path().ends_with("list_9201.json"));
    assert!(a.audio_path().ends_with("audio_9201.json"));
    let path = PathBuf::from("unique-stage8.pptx");
    assert_eq!(a.slides_dir(&path), a.slides_dir(&path));
    assert_ne!(a.slides_dir(&path), a.slides_dir(Path::new("other-stage8.pptx")));
}

#[test]
fn stage8_previews_prunes_far_thumbnail_entries_even_when_connected() {
    let mut a = test_app(); open_pdf(&mut a, 4, 12);
    for page in [0usize, 1, 6] {
        a.previews.insert((4, page), a.ctx.load_texture(format!("stage8_thumb_{page}"), stage8_img(), egui::TextureOptions::LINEAR));
        a.thumb_jpg.insert((4, page), Arc::new(vec![page as u8]));
    }
    a.connected = true;
    let state = StateMsg { active: Some(4), page: 1, ..Default::default() };
    a.tick_previews(&state);
    assert!(a.previews.contains_key(&(4, 0)));
    assert!(!a.previews.contains_key(&(4, 6)));
    assert!(!a.thumb_jpg.contains_key(&(4, 6)));
}

#[test]
fn stage8_ppt_zoom_not_requested_until_show_is_visible() {
    let mut a = test_app(); stage8_ppt(&mut a, 7);
    a.items[0].slides = Some(std::env::temp_dir().join(format!("stage8_missing_zoom_{}", net::unique_prefix())));
    a.show.as_mut().unwrap().zoom = 2.0;
    a.tick_ppt_zoom();
    assert_eq!(stage8_ppt_info(&a).zoom_busy, None);
}

struct Stage8Scratch(PathBuf);
impl Stage8Scratch {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("presentator_stage8_state_{}", net::unique_prefix()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn path(&self, file: &str) -> PathBuf { self.0.join(file) }
}
impl Drop for Stage8Scratch {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

#[test]
fn stage8_save_list_only_writes_when_server_is_online() {
    let scratch = Stage8Scratch::new();
    let mut a = test_app(); a.work_dir = scratch.0.clone();
    a.player.add(scratch.path("a.mp3"), "sample".into());
    a.save_list();
    assert!(!a.audio_path().exists());
    a.server_online = true;
    a.save_list();
    assert!(a.audio_path().exists());
    assert!(a.saved_audio.contains("sample"));
    let before = std::fs::read(a.audio_path()).unwrap();
    a.save_list();
    assert_eq!(before, std::fs::read(a.audio_path()).unwrap());
}

#[test]
fn stage8_load_list_restores_tracks_and_reports_missing_files() {
    let scratch = Stage8Scratch::new();
    let present = scratch.path("present.mp3");
    let missing = scratch.path("missing.mp3");
    std::fs::write(&present, b"fixture").unwrap();
    let mut a = test_app(); a.work_dir = scratch.0.clone();
    let entries = vec![
        SavedItem { name: "available".into(), src: present.clone(), repeat: false },
        SavedItem { name: "missing".into(), src: missing.clone(), repeat: false },
    ];
    std::fs::write(a.audio_path(), serde_json::to_string(&entries).unwrap()).unwrap();
    let ctx = a.ctx.clone(); a.load_list(&ctx);
    assert_eq!(a.player.tracks.len(), 1);
    assert_eq!(a.player.tracks[0].name, "available");
    assert!(a.status.contains("не найдено: 1"));
}

#[test]
fn stage8_load_list_removes_unreferenced_file_only_in_owned_workdir() {
    let scratch = Stage8Scratch::new();
    let stray = scratch.path("stray.tmp");
    std::fs::write(&stray, b"unreferenced").unwrap();
    let mut a = test_app(); a.work_dir = scratch.0.clone();
    let ctx = a.ctx.clone(); a.load_list(&ctx);
    assert!(!stray.exists());
}

#[test]
fn stage8_delete_files_never_removes_non_owned_source() {
    let owned = Stage8Scratch::new();
    let external = Stage8Scratch::new();
    let src = external.path("external.pdf");
    std::fs::write(&src, b"keep").unwrap();
    let mut a = test_app(); a.work_dir = owned.0.clone();
    let it = Item { src: src.clone(), pdf: None, ..pdf_item(99, 2) };
    a.delete_files(&it);
    assert!(src.exists());
}

#[test]
fn stage8_delete_files_removes_owned_copy_and_unused_slide_directory() {
    let scratch = Stage8Scratch::new();
    let src = scratch.path("uploaded.pptx");
    let dir = scratch.path("slides_cache");
    std::fs::write(&src, b"uploaded").unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("slide1.jpg"), b"frame").unwrap();
    let mut a = test_app(); a.work_dir = scratch.0.clone();
    let it = Item { src: src.clone(), slides: Some(dir.clone()), ..pdf_item(99, 2) };
    a.delete_files(&it);
    assert!(!src.exists()); assert!(!dir.exists());
}

#[test]
fn stage8_add_files_local_iterates_music_and_unsupported_entries() {
    let mut a = test_app(); let ctx = a.ctx.clone();
    a.add_files(vec![PathBuf::from("song.mp3"), PathBuf::from("invalid.dat")], &ctx);
    assert_eq!(a.player.tracks.len(), 1);
    assert!(a.status.contains("не поддерживается"));
}

#[test]
fn stage8_video_navigation_commands_keep_video_item_open() {
    let mut a = test_app(); stage8_video(&mut a, 41);
    let id = a.show.as_ref().unwrap().id;
    for cmd in [Cmd::Next, Cmd::Prev, Cmd::GoTo { page: 0 }, Cmd::GoTo { page: 99 }] {
        a.apply(cmd);
        assert_eq!(a.show.as_ref().unwrap().id, id);
    }
}

#[test]
fn stage8_video_transport_toggles_armed_playing_and_paused() {
    let mut a = test_app(); stage8_video(&mut a, 41);
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Video { gen, ev: video::VideoEvent::Ready(50.0) });
    a.apply(Cmd::Play);
    assert_eq!(a.local_state().video.unwrap().state, VideoState::Playing);
    a.apply(Cmd::Play);
    assert_eq!(a.local_state().video.unwrap().state, VideoState::Paused);
    a.apply(Cmd::Play);
    assert_eq!(a.local_state().video.unwrap().state, VideoState::Playing);
}

#[test]
fn stage8_video_repeat_updates_open_player_and_item() {
    let mut a = test_app(); stage8_video(&mut a, 41);
    a.items.push(Item { kind: "ВИДЕО".into(), pdf: None, ..pdf_item(41, 0) });
    a.apply(Cmd::VideoRepeat { id: 41, on: true });
    assert!(a.items[0].repeat);
    assert!(a.show.is_some());
    a.apply(Cmd::VideoRepeat { id: 41, on: false });
    assert!(!a.items[0].repeat);
}

#[test]
fn stage8_ppt_media_commands_keep_show_active() {
    let mut a = test_app(); stage8_ppt(&mut a, 88);
    for action in [MediaAction::Play, MediaAction::Pause, MediaAction::Stop] {
        a.apply(Cmd::PptMedia { id: 25, action });
        assert_eq!(a.show.as_ref().unwrap().id, 88);
    }
}

#[test]
fn stage8_ppt_position_only_changes_after_bridge_event() {
    let mut a = test_app(); stage8_ppt(&mut a, 88);
    a.apply(Cmd::GoTo { page: 5 });
    assert_eq!(a.show.as_ref().unwrap().page, 0);
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Ppt { gen, ev: ppt::PptEvent::Pos(6, 6) });
    assert_eq!(a.show.as_ref().unwrap().page, 5);
}

#[test]
fn stage8_ppt_focus_state_does_not_reset_on_identical_update() {
    let mut a = test_app();
    a.ppt_focus = PptFocus::Unknown;
    a.dirty = false;
    a.update_ppt_focus(PptFocus::Unknown);
    assert!(!a.dirty);
    a.update_ppt_focus(PptFocus::Lost { window: "browser".into() });
    assert!(a.dirty);
    assert_eq!(a.ppt_focus, PptFocus::Lost { window: "browser".into() });
}

#[test]
fn stage8_out_show_escape_arms_double_press_without_hiding_stage() {
    let mut a = test_app(); a.stage = true;
    stage8_out(&mut a, Out::ShowEsc);
    assert!(a.stage);
    assert!(a.esc_at.is_some());
}

#[test]
fn stage8_prepared_background_failed_generation_does_not_change_current_background() {
    let mut a = test_app();
    a.bg_gen = 5;
    stage8_out(&mut a, Out::Background { gen: 2, result: Err("old failure".into()) });
    assert!(!a.status.contains("old failure"));
}


// Stage 9: exercise the real scene-building path with an egui context in memory.
// In windowed mode these tests do not move an OS window or contact PowerPoint.
fn stage9_scene_frame(a: &mut App) {
    a.stage = true;
    a.bg_checked = true; // no disk access or asynchronous background loading
    a.cfg.windowed = true;
    let ctx = a.ctx.clone();
    ctx.set_embed_viewports(true); // the stage is rendered inside the test frame
    let mut input = egui::RawInput::default();
    input.screen_rect = Some(egui::Rect::from_min_size(
        egui::Pos2::ZERO, egui::vec2(1280.0, 720.0),
    ));
    let _ = ctx.run(input, |ctx| a.stage_window(ctx));
}

#[test]
fn stage9_scene_windowed_background_keeps_scene_alive_without_native_window() {
    let mut a = test_app();
    stage9_scene_frame(&mut a);
    assert!(a.stage);
    // Фон и PDF остаются поверх других окон даже в оконном режиме.
    assert_eq!(a.stage_sent, Some((true, false)));
    assert!(a.bg_checked);
    assert!(!a.bg_shown); // no background texture was loaded
}

#[test]
fn stage9_scene_draws_clock_and_timer_as_separate_modes() {
    for screen in [Screen::Clock, Screen::Timer] {
        let mut a = test_app();
        a.screen = screen;
        stage9_scene_frame(&mut a);
        assert_eq!(a.screen, screen);
        assert!(a.show.is_none());
        assert!(a.stage);
    }
}

#[test]
fn stage9_scene_pdf_handles_unrendered_page_without_blank_window_crash() {
    let mut a = test_app();
    open_pdf(&mut a, 907, 3);
    stage9_scene_frame(&mut a);
    assert_eq!(a.show.as_ref().map(|s| s.id), Some(907));
    assert!(a.stage);
}

#[test]
fn stage9_scene_pdf_zoom_handles_missing_zoom_texture() {
    let mut a = test_app();
    open_pdf(&mut a, 908, 2);
    a.show.as_mut().unwrap().zoom = 2.5;
    stage9_scene_frame(&mut a);
    assert_eq!(a.show.as_ref().unwrap().zoom, 2.5);
    // Фон и PDF остаются поверх других окон даже в оконном режиме.
    assert_eq!(a.stage_sent, Some((true, false)));
}

#[test]
fn stage9_scene_ppt_loading_keeps_show_while_stage_is_drawn() {
    let mut a = test_app();
    stage8_ppt(&mut a, 909);
    stage9_scene_frame(&mut a);
    assert_eq!(a.show.as_ref().map(|s| s.id), Some(909));
    assert!(matches!(stage8_ppt_info(&a).phase, PptPhase::Loading));
}

#[test]
fn stage9_scene_video_armed_does_not_start_native_decoder() {
    let mut a = test_app();
    stage8_video(&mut a, 910);
    let gen = a.show_gen;
    stage8_out(&mut a, Out::Video { gen, ev: video::VideoEvent::Ready(100.0) });
    stage9_scene_frame(&mut a);
    assert_eq!(a.local_state().video.unwrap().state, VideoState::Armed);
}

#[test]
fn stage9_scene_renders_static_background_texture() {
    let mut a = test_app();
    a.bg = Some(background::Background::new(
        &a.ctx, vec![(Arc::new(stage8_img()), Duration::ZERO)],
    ));
    stage9_scene_frame(&mut a);
    assert!(a.bg_shown);
}

#[test]
fn stage9_scene_pdf_can_display_previous_texture_while_page_is_prepared() {
    let mut a = test_app();
    open_pdf(&mut a, 911, 2);
    a.show.as_mut().unwrap().opened = Instant::now() - SWITCH_BG - Duration::from_secs(1);
    a.last_tex = Some(a.ctx.load_texture(
        "stage9_previous_slide", stage8_img(), egui::TextureOptions::LINEAR,
    ));
    stage9_scene_frame(&mut a);
    assert!(a.show.is_some());
    assert!(a.last_tex.is_some());
}

#[test]
fn stage9_scene_ppt_zoom_draws_a_prepared_zoom_texture() {
    let mut a = test_app();
    stage8_ppt(&mut a, 912);
    let texture = a.ctx.load_texture(
        "stage9_ppt_zoom", stage8_img(), egui::TextureOptions::LINEAR,
    );
    {
        let s = a.show.as_mut().unwrap();
        s.opened = Instant::now() - SWITCH_BG - Duration::from_secs(1);
        s.zoom = 2.0;
        if let ShowKind::Ppt(p) = &mut s.kind {
            p.zoom_img = Some((0, texture));
            p.phase = PptPhase::Shown;
        }
    }
    stage9_scene_frame(&mut a);
    assert!(a.show.as_ref().unwrap().ppt_zoom_on());
}

#[test]
fn stage9_scene_displays_loaded_image_presentation() {
    let mut a = test_app();
    let img = background::Background::new(
        &a.ctx, vec![(Arc::new(stage8_img()), Duration::ZERO)],
    );
    a.show_gen += 1;
    a.show = Some(Show {
        id: 913, page: 0, pages: 1, gen: a.show_gen,
        kind: ShowKind::Image(Some(img)),
        opened: Instant::now() - SWITCH_BG - Duration::from_secs(1),
        zoom: 1.5, center: (0.5, 0.5),
    });
    stage9_scene_frame(&mut a);
    assert_eq!(a.show.as_ref().map(|s| s.id), Some(913));
}


#[test]
fn stage19_focus_loss_and_recovery_cover_pdf_video_and_image() {
    for kind in ["PDF", "VIDEO", "IMAGE"] {
        let mut a = test_app();
        open_pdf(&mut a, 991, 4);
        if kind == "VIDEO" { stage8_video(&mut a, 991); }
        if kind == "IMAGE" { a.show.as_mut().unwrap().kind = ShowKind::Image(None); }
        assert!(a.show_uses_stage_focus());
        let now = Instant::now();
        a.handle_focus_probe(ppt::FocusProbe::Focused, now, 0, true);
        assert_eq!(a.local_state().ppt_focus, PptFocus::Ok);
        for _ in 0..2 {
            a.handle_focus_probe(ppt::FocusProbe::Away { hwnd: 123, title: "Office".into() }, now, 0, true);
        }
        assert_eq!(a.local_state().ppt_focus, PptFocus::Lost { window: "Office".into() });
        a.apply(Cmd::RestoreFocus);
        assert!(matches!(a.local_state().ppt_focus, PptFocus::Restoring { .. }));
        a.handle_focus_probe(ppt::FocusProbe::Focused, now, 0, true);
        assert_eq!(a.local_state().ppt_focus, PptFocus::Ok);
        assert!(a.ppt_focus_restore_deadline.is_none());
    }
}

#[test]
fn stage19_restore_timeout_is_reported_and_unknown_is_not_ok() {
    let mut a = test_app();
    open_pdf(&mut a, 992, 3);
    a.apply(Cmd::RestoreFocus);
    a.handle_focus_probe(ppt::FocusProbe::Unknown, Instant::now() + Duration::from_secs(3), 0, true);
    assert!(matches!(a.local_state().ppt_focus, PptFocus::RestoreFailed { .. }));
    a.handle_focus_probe(ppt::FocusProbe::Unknown, Instant::now(), 0, true);
    assert_eq!(a.local_state().ppt_focus, PptFocus::Unknown);
}

#[test]
fn stage19_operator_focus_poll_preserves_remote_server_state() {
    let mut a = test_app();
    a.connected = true;
    a.remote.active = Some(993);
    a.remote.ppt_focus = PptFocus::Lost { window: "Server Office".into() };
    a.ppt_focus = PptFocus::Ok;
    let ctx = a.ctx.clone();
    a.tick_ppt_focus(&ctx);
    assert_eq!(a.view().ppt_focus, PptFocus::Lost { window: "Server Office".into() });
    a.act(Cmd::RestoreFocus); // absent client must never dispatch locally
    assert_eq!(a.ppt_focus, PptFocus::Ok);
}

#[test]
fn stage19_pdf_engine_invalid_request_keeps_server_renderer() {
    let mut a = test_app();
    let before = a.local_state().pdf_engine;
    a.apply(Cmd::SetPdfEngine { engine: "invalid".into() });
    assert_eq!(a.local_state().pdf_engine, before);
    assert!(a.local_state().message.contains("Неизвестный PDF-движок"));
    assert!(a.dirty);
}

#[test]
fn stage19_operator_pdf_setting_does_not_change_local_renderer() {
    let mut a = test_app();
    a.connected = true;
    a.remote.pdf_engine = Some("pdfium".into());
    a.act(Cmd::SetPdfEngine { engine: "pdfium".into() });
    assert_eq!(a.renderer.engine(), render::PdfEngine::Hayro);
    assert_eq!(a.cfg.pdf_engine, "hayro");
    assert_eq!(a.view().pdf_engine.as_deref(), Some("pdfium"));
}

#[test]
fn stage19_pdf_engine_change_publishes_actual_result_and_preserves_show() {
    let mut a = test_app();
    open_pdf(&mut a, 994, 4);
    let generation = a.show_gen;
    a.apply(Cmd::SetPdfEngine { engine: "pdfium".into() });
    let state = a.local_state();
    assert_eq!(state.pdf_engine.as_deref(), Some(a.renderer.engine().as_str()));
    assert_eq!(state.active, Some(994));
    assert_eq!(a.show_gen, generation);
    if a.renderer.pdfium_error().is_some() {
        assert_eq!(a.renderer.engine(), render::PdfEngine::Hayro);
        assert_eq!(a.cfg.pdf_engine, "hayro");
        assert!(state.message.contains("PDFium недоступен"));
    } else {
        assert_eq!(a.renderer.engine(), render::PdfEngine::Pdfium);
        assert_eq!(a.cfg.pdf_engine, "pdfium");
    }
}

fn stage20_text_rects(shape: &egui::epaint::Shape, out: &mut Vec<(String, egui::Rect)>) {
    match shape {
        egui::epaint::Shape::Text(text) => out.push((
            text.galley.job.text.clone(),
            egui::Rect::from_min_size(text.pos, text.galley.size()),
        )),
        egui::epaint::Shape::Vec(shapes) => {
            for shape in shapes { stage20_text_rects(shape, out); }
        }
        _ => {}
    }
}

#[test]
fn stage20_video_status_never_overlaps_controls_at_narrow_widths() {
    for width in [520.0, 750.0, 1000.0] {
        for state in [VideoState::Loading, VideoState::Armed, VideoState::Playing, VideoState::Paused] {
            let mut app = test_app();
            let ctx = app.ctx.clone();
            configure_operator_style(&ctx);
            let mut view = stage7_remote_pdf(0, 1);
            view.video = Some(VideoInfo { state, pos: 61, dur: 125 });
            app.status = "Длинное сообщение о состоянии сервера ".repeat(10);
            app.selected = Some(10);
            let mut output = Vec::new();
            for _ in 0..3 {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 650.0))),
                    ..Default::default()
                };
                let frame = ctx.run(input, |ctx| {
                    egui::TopBottomPanel::bottom("stage20_bottom").show(ctx, |ui| {
                        app.bottom_bar(ui, &view);
                    });
                });
                output.clear();
                for shape in &frame.shapes { stage20_text_rects(&shape.shape, &mut output); }
            }
            let (_, status_rect) = output.iter().find(|(text, _)| text.starts_with("Видео · "))
                .expect("video status must remain visible");
            for label in ["◀ Назад", "Вперёд ▶", "▶ Показать", "■ Закрыть показ", "Удалить"] {
                let (_, button_rect) = output.iter().find(|(text, _)| text == label)
                    .expect("show control must remain visible");
                assert!(!status_rect.intersects(*button_rect), "overlap at width {width}: {label}");
                assert!(button_rect.left() >= 0.0 && button_rect.right() <= width + 1.0);
            }
        }
    }
}

#[test]
fn stage20_long_row_text_is_single_line_and_clipped_to_its_column() {
    let ctx = egui::Context::default();
    let target = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(160.0, 34.0));
    let frame = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            row_label(ui.painter(), target, &"Длинное название презентации ".repeat(20), 14.0, GRAY, false);
        });
    });
    let text = frame.shapes.iter().find_map(|shape| match &shape.shape {
        egui::epaint::Shape::Text(text) if text.galley.job.text.starts_with("Длинное") => Some((shape.clip_rect, text)),
        _ => None,
    }).expect("row text rendered");
    assert_eq!(text.1.galley.rows.len(), 1);
    assert!(text.0.left() >= target.left() && text.0.right() <= target.right());
}

// Редизайн: проверяем настоящую отрисовку egui без создания окон Windows.
#[test]
fn redesign_tabs_keep_controls_inside_standard_window() {
    for tab in [Tab::List, Tab::Player, Tab::Net, Tab::Settings] {
        let mut app = test_app();
        app.tab = tab;
        app.connected = true;
        app.remote = stage7_remote_pdf(0, 3);
        app.remote.items[0].name = "Очень длинное название презентации ".repeat(20);
        app.cfg.name = "Очень длинное название сервера ".repeat(20);
        app.selected = Some(10);
        let ctx = app.ctx.clone();
        configure_operator_style(&ctx);
        let before = serde_json::to_string(&app.remote).unwrap();
        let mut texts = Vec::new();
        for _ in 0..3 {
            let view = app.view();
            let output = ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(750.0, 650.0))),
                ..Default::default()
            }, |ctx| {
                egui::TopBottomPanel::top("redesign_top").show(ctx, |ui| app.top_bar(ui, &view));
                egui::TopBottomPanel::bottom("redesign_bottom").show(ctx, |ui| app.bottom_bar(ui, &view));
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.tab_row(ui, &view);
                    match tab {
                        Tab::List => app.list(ui, &view),
                        Tab::Player => app.player_panel(ui, &view),
                        Tab::Net => app.network_panel(ui),
                        Tab::Settings => app.settings_panel(ui),
                    }
                });
            });
            texts.clear();
            for shape in &output.shapes { stage20_text_rects(&shape.shape, &mut texts); }
        }
        for label in ["▶ Показать", "■ Закрыть показ", "Фон", "Часы", "Таймер", "Презентации", "Настройки"] {
            let (_, rect) = texts.iter().find(|(text, _)| text == label).expect(label);
            assert!(rect.left() >= 0.0 && rect.right() <= 751.0, "{label}: {rect:?}");
            assert!(rect.top() >= 0.0 && rect.bottom() <= 651.0, "{label}: {rect:?}");
        }
        assert_eq!(app.selected, Some(10));
        assert_eq!(serde_json::to_string(&app.remote).unwrap(), before, "отрисовка не меняет удалённое состояние");
    }
}

#[test]
fn redesign_list_distinguishes_selection_from_current_show() {
    let mut app = test_app();
    let mut view = stage7_remote_pdf(0, 3);
    let mut selected = view.items[0].clone();
    selected.id = 11;
    selected.name = "Следующий файл.pdf".into();
    view.items.push(selected);
    app.selected = Some(11);
    let ctx = app.ctx.clone();
    configure_operator_style(&ctx);
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| app.list(ui, &view));
    });
    let mut texts = Vec::new();
    for shape in &output.shapes { stage20_text_rects(&shape.shape, &mut texts); }
    let (_, live) = texts.iter().find(|(text, _)| text.starts_with("Текущий показ · ")).unwrap();
    let (_, next) = texts.iter().find(|(text, _)| text.starts_with("Выбрано · ")).unwrap();
    assert!(!live.intersects(*next));
    assert_eq!(app.selected, Some(11));
    assert_eq!(view.active, Some(10));
}
