//! Stage 12: true playback against bundled short H.264 MP4 (libmpv or MF).
//! Does not inspect the UI, but observes Ready, playback progress and Ended twice.
use super::*;
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires a Windows interactive test session: scripts/verify_stage12.ps1 -Native"]
fn stage12_native_video_play_end_rearm_and_replay() {
    assert_eq!(
        std::env::var("PRESENTATOR_STAGE12_NATIVE").ok().as_deref(),
        Some("I_UNDERSTAND"),
        "Native video tests require explicit opt-in via scripts/verify_stage12.ps1 -Native"
    );
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/stage12_video.mp4");
    assert!(src.is_file(), "Missing video fixture: {}", src.display());
    let (tx, rx) = channel();
    // Native video window stays outside the monitor. No GUI automation is needed.
    let video = Video::start(&src, false, Some((-3500, -3500, 160, 90)), false, move |ev| {
        let _ = tx.send(ev);
    });
    let deadline = Instant::now() + Duration::from_secs(35);
    let (mut ready, mut playback_progress, mut ended) = (false, false, 0);
    while Instant::now() < deadline && ended < 2 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(VideoEvent::Ready(duration)) => {
                assert!(!ready, "Ready must be emitted only once");
                assert!(duration > 1.0 && duration < 10.0, "Unexpected video duration: {duration}");
                ready = true;
                video.play();
            }
            Ok(VideoEvent::Pos { pos, dur, playing }) => {
                assert!(pos.is_finite() && dur.is_finite());
                if playing && pos > 0.0 { playback_progress = true; }
            }
            Ok(VideoEvent::Ended) => {
                assert!(ready, "Ended before Ready");
                ended += 1;
                if ended == 1 { video.play(); }
            }
            Ok(VideoEvent::Error(message)) => panic!("Video engine failed: {message}"),
            Err(e) => panic!("Video playback did not finish twice: {e}"),
        }
    }
    assert!(ready, "Video never became ready");
    assert!(playback_progress, "Player never reported positive playing position");
    assert_eq!(ended, 2, "Video did not rearm and play twice");
    drop(video);
}

struct TemporaryFile(std::path::PathBuf);

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// Wait for an observed native state, allowing queued position events to settle.
// Each phase has its own fixed deadline; unrelated events cannot extend it.
fn stage15_wait_video(
    rx: &std::sync::mpsc::Receiver<VideoEvent>,
    label: &str,
    accept: impl Fn(&VideoEvent) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "video timeout: {label}");
        let event = rx.recv_timeout(left).unwrap_or_else(|e| panic!("video {label}: {e}"));
        match &event {
            VideoEvent::Error(e) => panic!("video {label}: {e}"),
            VideoEvent::Pos { pos, dur, .. } => assert!(pos.is_finite() && dur.is_finite() && *dur > 0.0),
            _ => {}
        }
        if accept(&event) { return; }
        assert!(!matches!(event, VideoEvent::Ended), "unexpected end while waiting for {label}");
    }
}

#[test]
#[ignore = "real native playback controls; scripts/verify_stage15.ps1 -Native on an isolated desktop"]
fn stage15_native_video_pause_seek_resume_then_finish() {
    assert_eq!(std::env::var("PRESENTATOR_STAGE12_NATIVE").ok().as_deref(), Some("I_UNDERSTAND"));
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stage12_video.mp4");
    assert!(fixture.is_file(), "missing video fixture");
    let (tx, rx) = channel();
    let video = Video::start(&fixture, false, Some((-3500, -3500, 160, 90)), false, move |ev| { let _ = tx.send(ev); });
    stage15_wait_video(&rx, "Ready", |e| matches!(e, VideoEvent::Ready(d) if *d > 1.0 && *d < 10.0));
    video.seek_to(1.0);
    stage15_wait_video(&rx, "absolute seek while paused", |e| matches!(e, VideoEvent::Pos { pos, playing: false, .. } if *pos >= 0.9 && *pos <= 1.1));
    video.seek(-1.0);
    stage15_wait_video(&rx, "relative seek backwards while paused", |e| matches!(e, VideoEvent::Pos { pos, playing: false, .. } if *pos < 0.5));
    video.play();
    stage15_wait_video(&rx, "play starts playback", |e| matches!(e, VideoEvent::Pos { playing: true, .. }));
    video.toggle();
    stage15_wait_video(&rx, "toggle pauses playback", |e| matches!(e, VideoEvent::Pos { playing: false, .. }));
    video.toggle();
    stage15_wait_video(&rx, "toggle resumes playback", |e| matches!(e, VideoEvent::Pos { playing: true, .. }));
    stage15_wait_video(&rx, "Ended after controls", |e| matches!(e, VideoEvent::Ended));
    drop(video);
}

#[test]
#[ignore = "native video loop and stop transition; scripts/verify_stage16.ps1 -Native on an isolated desktop"]
fn stage16_native_video_repeat_loops_then_stops_cleanly() {
    assert_eq!(std::env::var("PRESENTATOR_STAGE12_NATIVE").ok().as_deref(), Some("I_UNDERSTAND"));
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stage12_video.mp4");
    assert!(fixture.is_file(), "missing video fixture");
    let (tx, rx) = channel();
    let video = Video::start(&fixture, false, Some((-3500, -3500, 160, 90)), true, move |ev| { let _ = tx.send(ev); });
    stage15_wait_video(&rx, "Ready for repeat", |e| matches!(e, VideoEvent::Ready(d) if *d > 1.0 && *d < 10.0));
    video.play();

    // Require a real backwards position jump while playback remains active. This proves the loop
    // happened; merely receiving Ready or staying alive does not count as repeat success.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut furthest = 0.0f64;
    let mut looped = false;
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(VideoEvent::Pos { pos, dur, playing }) => {
                assert!(pos.is_finite() && dur.is_finite() && dur > 0.0);
                if playing && pos + 0.2 < furthest { looped = true; break; }
                furthest = furthest.max(pos);
            }
            Ok(VideoEvent::Error(e)) => panic!("repeat playback failed: {e}"),
            Ok(VideoEvent::Ended) => panic!("repeat mode emitted Ended instead of looping"),
            Ok(VideoEvent::Ready(_)) => panic!("duplicate Ready during repeat"),
            Err(e) => panic!("video did not loop before deadline: {e}"),
        }
    }
    assert!(looped, "no backwards position jump observed; highest position was {furthest:.2}s");

    video.set_repeat(false);
    stage15_wait_video(&rx, "position after repeat disabled", |e| matches!(e, VideoEvent::Pos { playing: true, .. }));
    stage15_wait_video(&rx, "normal end after repeat disabled", |e| matches!(e, VideoEvent::Ended));
    drop(video);
}

#[test]
#[ignore = "requires a Windows interactive test session and real media engine; scripts/verify_stage14.ps1 -Native"]
fn stage14_native_video_invalid_media_then_valid_recovery() {
    assert_eq!(
        std::env::var("PRESENTATOR_STAGE12_NATIVE").ok().as_deref(),
        Some("I_UNDERSTAND"),
        "Native video recovery test requires explicit opt-in via scripts/verify_stage14.ps1 -Native"
    );
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/stage12_video.mp4");
    assert!(fixture.is_file(), "Missing video fixture: {}", fixture.display());

    let bad_path = std::env::temp_dir().join(format!(
        "presentator_stage14_invalid_{}_{}.mp4", std::process::id(), crate::net::unique_prefix()
    ));
    std::fs::write(&bad_path, b"not a media container or video stream").expect("write invalid media input");
    let _bad_file = TemporaryFile(bad_path.clone());

    let (bad_tx, bad_rx) = channel();
    let invalid = Video::start(&bad_path, false, Some((-3500, -3500, 160, 90)), false, move |ev| {
        let _ = bad_tx.send(ev);
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match bad_rx.recv_timeout(remaining) {
            Ok(VideoEvent::Error(message)) => {
                assert!(!message.trim().is_empty(), "native decoder error must carry a message");
                break;
            }
            Ok(VideoEvent::Ready(_)) => panic!("invalid media unexpectedly became ready"),
            Ok(VideoEvent::Pos { .. }) | Ok(VideoEvent::Ended) => panic!("invalid media emitted playback events before Error"),
            Err(e) => panic!("invalid media did not produce a bounded error event: {e}"),
        }
    }
    drop(invalid);
    drop(_bad_file);

    let (tx, rx) = channel();
    let recovered = Video::start(&fixture, false, Some((-3500, -3500, 160, 90)), false, move |ev| {
        let _ = tx.send(ev);
    });
    let deadline = Instant::now() + Duration::from_secs(35);
    let (mut ready, mut progressed) = (false, false);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(VideoEvent::Ready(duration)) => {
                assert!(duration > 1.0 && duration < 10.0, "unexpected recovery video duration: {duration}");
                ready = true;
                recovered.play();
            }
            Ok(VideoEvent::Pos { pos, dur, playing }) => {
                assert!(pos.is_finite() && dur.is_finite());
                progressed |= playing && pos > 0.0;
            }
            Ok(VideoEvent::Ended) => {
                assert!(ready, "recovery playback ended before Ready");
                assert!(progressed, "recovery playback never reported progress");
                break;
            }
            Ok(VideoEvent::Error(message)) => panic!("valid video failed after invalid input: {message}"),
            Err(e) => panic!("valid video did not recover and finish playback: {e}"),
        }
    }
    drop(recovered);
}
