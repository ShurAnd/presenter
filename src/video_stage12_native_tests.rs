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
