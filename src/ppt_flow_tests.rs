//! Stage 4: tests for the PowerPoint Host's actual dispatch and failure guards.
//! The worker is an in-memory channel; none of these tests start COM or PowerPoint.
use super::*;

fn fake_host() -> (Host, Receiver<Req>) {
    let host = Host::new(|_, _| {});
    let (tx, rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel::<()>();
    drop(done_tx); // prevent Host::shutdown waiting four seconds
    *host.worker.lock().unwrap() = Some(Worker { tx, epoch: 1, done: done_rx });
    host.epoch.store(1, Ordering::SeqCst);
    (host, rx)
}

#[test]
fn host_queues_navigation_in_order_without_com() {
    let (host, rx) = fake_host();
    host.next(); host.prev(); host.goto(7); host.activate();
    assert!(matches!(rx.try_recv().unwrap(), Req::Next));
    assert!(matches!(rx.try_recv().unwrap(), Req::Prev));
    assert!(matches!(rx.try_recv().unwrap(), Req::Goto(7)));
    assert!(matches!(rx.try_recv().unwrap(), Req::Show));
    assert!(rx.try_recv().is_err());
}

#[test]
fn host_sends_media_cursor_and_zoom_parameters_unmodified() {
    let (host, rx) = fake_host();
    host.set_cursor(true);
    host.media("pause", 52);
    let zoom = PathBuf::from("test_zoom_5.jpg");
    host.export_zoom(5, 1800, &zoom);
    host.set_cursor(false);
    assert!(matches!(rx.try_recv().unwrap(), Req::Cursor(4)));
    match rx.try_recv().unwrap() {
        Req::Media(action, id) => { assert_eq!(action, "pause"); assert_eq!(id, 52); }
        _ => panic!("expected media command"),
    }
    match rx.try_recv().unwrap() {
        Req::Zoom(page, width, path) => {
            assert_eq!((page, width), (5, 1800));
            assert_eq!(path, zoom);
        }
        _ => panic!("expected zoom command"),
    }
    assert!(matches!(rx.try_recv().unwrap(), Req::Cursor(3)));
    assert!(!host.cursor.load(Ordering::SeqCst));
}

#[test]
fn host_close_is_idempotent_and_does_not_quit_worker() {
    let (host, rx) = fake_host();
    host.current.store(71, Ordering::SeqCst);
    host.close(); host.close();
    assert_eq!(host.current.load(Ordering::SeqCst), 0);
    assert!(matches!(rx.try_recv().unwrap(), Req::Close));
    assert!(rx.try_recv().is_err());
    assert!(host.worker.lock().unwrap().is_some());
}

#[test]
fn host_close_with_no_presentation_sends_nothing() {
    let (host, rx) = fake_host();
    host.close();
    assert!(rx.try_recv().is_err());
}

#[test]
fn host_lag_probe_is_none_until_show_is_active() {
    let host = Host::new(|_, _| {});
    let probe = host.lag_probe();
    assert_eq!(probe(), None);
    host.current.store(1, Ordering::SeqCst);
    assert_eq!(probe(), Some(0));
    host.waiting.store(u64::MAX, Ordering::SeqCst);
    assert_eq!(probe(), Some(0)); // saturating arithmetic on inconsistent clock
    host.current.store(0, Ordering::SeqCst);
    assert_eq!(probe(), None);
}

#[test]
fn host_restart_discards_stale_queue_without_killing_process() {
    let (host, rx) = fake_host();
    host.current.store(42, Ordering::SeqCst);
    host.waiting.store(1234, Ordering::SeqCst);
    host.next();
    host.restart(); // pid == 0; cannot terminate any Windows process
    assert_eq!(host.current.load(Ordering::SeqCst), 0);
    assert_eq!(host.waiting.load(Ordering::SeqCst), 0);
    assert!(host.worker.lock().unwrap().is_none());
    assert!(matches!(rx.try_recv().unwrap(), Req::Next));
    assert!(rx.try_recv().is_err());
}

#[test]
fn host_does_not_reopen_worker_for_commands_without_open() {
    let host = Host::new(|_, _| {});
    host.next(); host.prev(); host.goto(3); host.close(); host.activate();
    assert!(host.worker.lock().unwrap().is_none());
    assert_eq!(host.epoch.load(Ordering::SeqCst), 0);
}

#[test]
fn end_guard_emits_error_for_current_live_presentation() {
    let (event_tx, event_rx) = mpsc::channel();
    let host = Host::new(move |sid, ev| { let _ = event_tx.send((sid, ev)); });
    let (worker_tx, _worker_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    *host.worker.lock().unwrap() = Some(Worker { tx: worker_tx, epoch: 5, done: done_rx });
    host.current.store(93, Ordering::SeqCst);
    let guard = EndGuard {
        _done: done_tx, epoch: 5,
        worker: Arc::downgrade(&host.worker),
        current: host.current.clone(), on_event: host.on_event.clone(),
    };
    drop(guard);
    assert!(host.worker.lock().unwrap().is_none());
    match event_rx.try_recv().unwrap() {
        (93, PptEvent::Error(message)) => assert!(message.contains("завершился")),
        _ => panic!("expected bridge-terminated event"),
    }
}

#[test]
fn end_guard_does_not_send_false_alarm_after_show_closed() {
    let (event_tx, event_rx) = mpsc::channel();
    let host = Host::new(move |sid, ev| { let _ = event_tx.send((sid, ev)); });
    let (worker_tx, _worker_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    *host.worker.lock().unwrap() = Some(Worker { tx: worker_tx, epoch: 2, done: done_rx });
    // current = 0: no presentation is active
    let guard = EndGuard {
        _done: done_tx, epoch: 2,
        worker: Arc::downgrade(&host.worker),
        current: host.current.clone(), on_event: host.on_event.clone(),
    };
    drop(guard);
    assert!(host.worker.lock().unwrap().is_none());
    assert!(event_rx.try_recv().is_err());
}

#[test]
fn end_guard_ignores_stale_worker_after_restart() {
    let (event_tx, event_rx) = mpsc::channel();
    let host = Host::new(move |sid, ev| { let _ = event_tx.send((sid, ev)); });
    let (worker_tx, _worker_rx) = mpsc::channel();
    let (done_tx, _done_rx) = mpsc::channel();
    let (new_done_tx, new_done_rx) = mpsc::channel::<()>();
    drop(new_done_tx);
    *host.worker.lock().unwrap() = Some(Worker { tx: worker_tx, epoch: 3, done: new_done_rx });
    host.current.store(88, Ordering::SeqCst);
    let old_guard = EndGuard {
        _done: done_tx, epoch: 2,
        worker: Arc::downgrade(&host.worker),
        current: host.current.clone(), on_event: host.on_event.clone(),
    };
    drop(old_guard);
    assert!(host.worker.lock().unwrap().is_some());
    assert!(event_rx.try_recv().is_err());
}
