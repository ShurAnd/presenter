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
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        let left = until.saturating_duration_since(Instant::now());
        let (actual_sid, event) = rx.recv_timeout(left)
            .unwrap_or_else(|e| panic!("PowerPoint timeout or disconnect waiting for {label}: {e}"));
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
    await_event(&rx, SID, "Ready(3)", |e| matches!(e, PptEvent::Ready(3, _)));
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
