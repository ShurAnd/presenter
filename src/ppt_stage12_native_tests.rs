//! Stage 12: actual PowerPoint COM integration. NOT mocked.
//! Tests are ignored by default; they create an actual slideshow window.
//! Run only on a Windows test account without other POWERPNT processes.
use super::*;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn preflight() -> PathBuf {
    assert_eq!(
        std::env::var("PRESENTATOR_STAGE12_NATIVE").ok().as_deref(),
        Some("I_UNDERSTAND"),
        "Native PowerPoint tests require explicit opt-in via scripts/verify_stage12.ps1 -Native"
    );
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/stage12_three_slides.pptx");
    assert!(fixture.is_file(), "Missing PowerPoint fixture: {}", fixture.display());
    fixture
}

fn await_event(
    rx: &mpsc::Receiver<(u64, PptEvent)>,
    sid: u64,
    label: &str,
    accept: impl Fn(&PptEvent) -> bool,
) {
    await_event_with_previous(rx, sid, None, label, accept);
}

fn await_event_with_previous(
    rx: &mpsc::Receiver<(u64, PptEvent)>,
    sid: u64,
    previous_sid: Option<u64>,
    label: &str,
    accept: impl Fn(&PptEvent) -> bool,
) {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        let left = until.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "PowerPoint timeout waiting for {label}");
        let (actual_sid, event) = rx.recv_timeout(left)
            .unwrap_or_else(|e| panic!("PowerPoint timeout or disconnect waiting for {label}: {e}"));
        // A previous worker may already have queued events before restart.
        // Those events cannot prove readiness or failure of the new session.
        if actual_sid != sid && Some(actual_sid) == previous_sid {
            println!("Ignoring previous PowerPoint session {actual_sid} while waiting for {label}");
            continue;
        }
        assert_eq!(actual_sid, sid, "event from unexpected presentation");
        println!("PowerPoint received event while waiting for {label}");
        if let PptEvent::Error(e) = &event {
            panic!("PowerPoint error while waiting for {label}: {e}");
        }
        if accept(&event) { return; }
    }
}

#[test]
#[ignore = "requires real PowerPoint and explicit opt-in: scripts/verify_stage12.ps1 -Native"]
fn stage12_native_ppt_three_slides_navigation_and_close() {
    let fixture = preflight();
    let (tx, rx) = mpsc::channel();
    let host = Host::new(move |sid, event| { let _ = tx.send((sid, event)); });
    const SID: u64 = 9012;
    host.open(SID, &fixture, true, "", None, 480).expect("bridge worker must start");
    await_event(&rx, SID, "Ready(3) with a valid slideshow HWND", |e| match e {
        PptEvent::Ready(3, hwnd) => {
            assert_ne!(*hwnd, 0, "PowerPoint must provide a slideshow HWND for focus monitoring");
            true
        }
        _ => false,
    });
    host.goto(2);
    await_event(&rx, SID, "Goto(2)", |e| matches!(e, PptEvent::Pos(2, 3)));
    host.prev();
    await_event(&rx, SID, "Prev->1", |e| matches!(e, PptEvent::Pos(1, 3)));
    host.goto(3);
    await_event(&rx, SID, "Goto(3)", |e| matches!(e, PptEvent::Pos(3, 3)));
    host.close();
    assert_eq!(host.current.load(Ordering::SeqCst), 0);
    host.shutdown();
}

#[test]
#[ignore = "requires real PowerPoint and explicit opt-in: scripts/verify_stage12.ps1 -Native"]
fn stage12_native_ppt_missing_file_then_recover() {
    let fixture = preflight();
    let absent = std::env::temp_dir().join(format!(
        "presentator_stage12_absent_{}_{}.pptx", std::process::id(), now_ms()
    ));
    assert!(!absent.exists(), "Unexpected test collision: {}", absent.display());
    let (tx, rx) = mpsc::channel();
    let host = Host::new(move |sid, event| { let _ = tx.send((sid, event)); });
    const BAD: u64 = 9121;
    const GOOD: u64 = 9122;
    host.open(BAD, &absent, true, "", None, 480).unwrap();
    let (sid, ev) = rx.recv_timeout(Duration::from_secs(30))
        .expect("PowerPoint must report missing file");
    assert_eq!(sid, BAD);
    match ev {
        PptEvent::Error(message) => assert!(!message.trim().is_empty()),
        _ => panic!("Expected error for missing presentation"),
    }
    host.open(GOOD, &fixture, true, "", None, 480).unwrap();
    await_event(&rx, GOOD, "recovery Ready(3)", |e| matches!(e, PptEvent::Ready(3, _)));
    host.close();
    host.shutdown();
}

fn wait_for_process_exit(pid: u32) {
    use std::process::Command;

    let filter = format!("PID eq {pid}");
    let needle = format!("\"{pid}\"");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let output = Command::new("tasklist.exe")
            .args(["/FI", filter.as_str(), "/FO", "CSV", "/NH"])
            .output()
            .expect("tasklist.exe must be available to verify the PowerPoint process exit");
        assert!(output.status.success(), "tasklist.exe failed while checking PowerPoint PID {pid}");
        if !String::from_utf8_lossy(&output.stdout).contains(&needle) {
            return;
        }
        assert!(Instant::now() < deadline, "PowerPoint process {pid} did not exit after Host::restart");
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[test]
#[ignore = "terminates the PowerPoint process started by this test; run only on an isolated test desktop via scripts/verify_stage14.ps1 -Native"]
fn stage14_native_ppt_process_termination_then_reopen() {
    let fixture = preflight();
    let (tx, rx) = mpsc::channel();
    let host = Host::new(move |sid, event| { let _ = tx.send((sid, event)); });
    const BEFORE: u64 = 14101;
    const AFTER: u64 = 14102;

    host.open(BEFORE, &fixture, true, "", None, 480).expect("initial bridge must start");
    await_event(&rx, BEFORE, "initial Ready(3)", |e| matches!(e, PptEvent::Ready(3, _)));

    let pid = host.pid.load(Ordering::SeqCst);
    assert_ne!(pid, 0, "PowerPoint process ID must be recorded before restart");
    host.restart();
    wait_for_process_exit(pid);

    host.open(AFTER, &fixture, true, "", None, 480).expect("bridge must start after PowerPoint termination");
    await_event_with_previous(&rx, AFTER, Some(BEFORE), "Ready(3) after restart", |e| matches!(e, PptEvent::Ready(3, _)));
    host.goto(2);
    await_event_with_previous(&rx, AFTER, Some(BEFORE), "Goto(2) after restart", |e| matches!(e, PptEvent::Pos(2, 3)));
    host.close();
    host.shutdown();
}

#[test]
fn stage14_wait_ignores_previous_session_but_requires_current_ready() {
    let (tx, rx) = mpsc::channel();
    tx.send((1, PptEvent::Ready(3, 0))).unwrap();
    tx.send((1, PptEvent::Error("previous process stopped".into()))).unwrap();
    tx.send((2, PptEvent::Ready(3, 0))).unwrap();
    await_event_with_previous(&rx, 2, Some(1), "current Ready", |e| matches!(e, PptEvent::Ready(3, _)));
    assert!(rx.try_recv().is_err(), "current Ready must have been consumed");
}

#[test]
#[should_panic(expected = "PowerPoint error while waiting for current Ready: current failure")]
fn stage14_wait_does_not_ignore_current_session_error() {
    let (tx, rx) = mpsc::channel();
    tx.send((1, PptEvent::Ready(3, 0))).unwrap();
    tx.send((2, PptEvent::Error("current failure".into()))).unwrap();
    await_event_with_previous(&rx, 2, Some(1), "current Ready", |e| matches!(e, PptEvent::Ready(3, _)));
}

#[test]
#[ignore = "restarts real PowerPoint three times; scripts/verify_stage15.ps1 -Native on an isolated desktop"]
fn stage15_native_ppt_three_restarts_keep_navigation_working() {
    let fixture = preflight();
    let (tx, rx) = mpsc::channel();
    let host = Host::new(move |sid, event| { let _ = tx.send((sid, event)); });
    let mut previous = None;
    for cycle in 0..4 {
        let sid = 15100 + cycle;
        host.open(sid, &fixture, true, "", None, 480).expect("bridge must start each cycle");
        await_event_with_previous(&rx, sid, previous, "cycle Ready(3)", |e| matches!(e, PptEvent::Ready(3, _)));
        host.goto(3);
        await_event_with_previous(&rx, sid, previous, "cycle Goto(3)", |e| matches!(e, PptEvent::Pos(3, 3)));
        host.prev();
        await_event_with_previous(&rx, sid, previous, "cycle Prev(2)", |e| matches!(e, PptEvent::Pos(2, 3)));
        assert_eq!(host.current.load(Ordering::SeqCst), sid);
        if cycle < 3 {
            let pid = host.pid.load(Ordering::SeqCst);
            assert_ne!(pid, 0);
            host.restart();
            assert_eq!(host.current.load(Ordering::SeqCst), 0);
            assert_eq!(host.waiting.load(Ordering::SeqCst), 0);
            wait_for_process_exit(pid);
        }
        previous = Some(sid);
    }
    host.close();
    assert_eq!(host.current.load(Ordering::SeqCst), 0);
    host.shutdown();
}
