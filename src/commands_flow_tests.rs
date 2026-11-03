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
