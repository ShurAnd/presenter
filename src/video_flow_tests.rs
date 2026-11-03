//! Stage 5: video API command routing without opening a window or running a decoder.
use super::*;
use std::sync::mpsc::Receiver;

fn mock() -> (Video, Receiver<VCmd>) {
    let (tx, rx) = channel();
    // Invalid thread id on Windows: waking it is harmless; no GUI thread is created.
    (Video { tx, #[cfg(windows)] thread: 0 }, rx)
}

#[test]
fn video_play_and_toggle_are_sent_in_order() {
    let (video, rx) = mock();
    video.play();
    video.toggle();
    video.play();
    assert!(matches!(rx.recv().unwrap(), VCmd::Start));
    assert!(matches!(rx.recv().unwrap(), VCmd::Toggle));
    assert!(matches!(rx.recv().unwrap(), VCmd::Start));
    assert!(rx.try_recv().is_err());
}

#[test]
fn video_repeat_switches_are_forwarded() {
    let (video, rx) = mock();
    video.set_repeat(true);
    video.set_repeat(false);
    assert!(matches!(rx.recv().unwrap(), VCmd::Repeat(true)));
    assert!(matches!(rx.recv().unwrap(), VCmd::Repeat(false)));
}

#[test]
fn video_relative_and_absolute_seek_remain_distinct() {
    let (video, rx) = mock();
    video.seek(-5.25);
    video.seek_to(93.5);
    match rx.recv().unwrap() {
        VCmd::Seek(seconds) => assert_eq!(seconds, -5.25),
        _ => panic!("expected relative seek"),
    }
    match rx.recv().unwrap() {
        VCmd::SeekTo(seconds) => assert_eq!(seconds, 93.5),
        _ => panic!("expected absolute seek"),
    }
}

#[test]
fn video_drop_requests_quit_after_queued_commands() {
    let (video, rx) = mock();
    video.play();
    video.seek_to(0.0);
    drop(video);
    assert!(matches!(rx.recv().unwrap(), VCmd::Start));
    assert!(matches!(rx.recv().unwrap(), VCmd::SeekTo(v) if v == 0.0));
    assert!(matches!(rx.recv().unwrap(), VCmd::Quit));
    assert!(rx.recv().is_err());
}

// Stage 6: additional command-channel failure and ordering cases (no video engine).
#[test]
fn video_unused_player_drop_still_queues_quit() {
    let (video, rx) = mock();
    drop(video);
    assert!(matches!(rx.recv().unwrap(), VCmd::Quit));
    assert!(rx.recv().is_err());
}

#[test]
fn video_seeks_are_delivered_individually_and_in_order() {
    let (video, rx) = mock();
    video.seek_to(12.5);
    video.seek_to(0.0);
    video.seek(-3.0);
    assert!(matches!(rx.recv().unwrap(), VCmd::SeekTo(v) if v == 12.5));
    assert!(matches!(rx.recv().unwrap(), VCmd::SeekTo(v) if v == 0.0));
    assert!(matches!(rx.recv().unwrap(), VCmd::Seek(v) if v == -3.0));
    assert!(rx.try_recv().is_err());
}

#[test]
fn video_repeat_update_does_not_reorder_play_and_seek_commands() {
    let (video, rx) = mock();
    video.set_repeat(false);
    video.play();
    video.seek_to(15.0);
    video.set_repeat(true);
    video.toggle();
    assert!(matches!(rx.recv().unwrap(), VCmd::Repeat(false)));
    assert!(matches!(rx.recv().unwrap(), VCmd::Start));
    assert!(matches!(rx.recv().unwrap(), VCmd::SeekTo(v) if v == 15.0));
    assert!(matches!(rx.recv().unwrap(), VCmd::Repeat(true)));
    assert!(matches!(rx.recv().unwrap(), VCmd::Toggle));
}

#[test]
fn video_command_sender_survives_closed_receiver_without_panicking() {
    let (video, rx) = mock();
    drop(rx);
    video.play();
    video.seek(10.0);
    video.seek_to(2.0);
    video.set_repeat(true);
    video.toggle();
    drop(video);
}
