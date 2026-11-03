//! Stage 5: verify the audio command interface without loading libmpv or touching hardware.
use super::*;
use std::sync::mpsc::Receiver;

fn mock() -> (Audio, Receiver<ACmd>) {
    let (tx, rx) = channel();
    (Audio { tx }, rx)
}

#[test]
fn audio_load_forwards_deck_generation_path_and_repeat() {
    let (audio, rx) = mock();
    let path = Path::new("C:/presentator/music/first track.mp3");
    audio.load(Deck::Main, 41, path, true);
    match rx.recv().unwrap() {
        ACmd::Load { deck, gen, path: received, repeat } => {
            assert_eq!(deck, Deck::Main);
            assert_eq!(gen, 41);
            assert_eq!(received, path);
            assert!(repeat);
        }
        _ => panic!("expected audio load"),
    }
    audio.load(Deck::Insert, 42, Path::new("insert.wav"), false);
    match rx.recv().unwrap() {
        ACmd::Load { deck, gen, repeat, .. } => {
            assert_eq!(deck, Deck::Insert);
            assert_eq!(gen, 42);
            assert!(!repeat);
        }
        _ => panic!("expected insert load"),
    }
}

#[test]
fn audio_pause_and_resume_target_the_requested_deck() {
    let (audio, rx) = mock();
    audio.pause(Deck::Main, true);
    audio.pause(Deck::Insert, false);
    assert!(matches!(rx.recv().unwrap(), ACmd::Pause(Deck::Main, true)));
    assert!(matches!(rx.recv().unwrap(), ACmd::Pause(Deck::Insert, false)));
}

#[test]
fn audio_stop_preserves_fade_selection_and_order() {
    let (audio, rx) = mock();
    audio.stop(Deck::Main, true);
    audio.stop(Deck::Main, false);
    audio.stop(Deck::Insert, false);
    assert!(matches!(rx.recv().unwrap(), ACmd::Stop { deck: Deck::Main, fade: true }));
    assert!(matches!(rx.recv().unwrap(), ACmd::Stop { deck: Deck::Main, fade: false }));
    assert!(matches!(rx.recv().unwrap(), ACmd::Stop { deck: Deck::Insert, fade: false }));
}

#[test]
fn audio_seek_repeat_and_volume_keep_values() {
    let (audio, rx) = mock();
    audio.seek(Deck::Insert, 12.75);
    audio.repeat(Deck::Main, true);
    audio.volume(0);
    audio.volume(100);
    match rx.recv().unwrap() {
        ACmd::Seek(Deck::Insert, value) => assert_eq!(value, 12.75),
        _ => panic!("expected seek"),
    }
    assert!(matches!(rx.recv().unwrap(), ACmd::Repeat(Deck::Main, true)));
    assert!(matches!(rx.recv().unwrap(), ACmd::Volume(0)));
    assert!(matches!(rx.recv().unwrap(), ACmd::Volume(100)));
    assert!(rx.try_recv().is_err());
}

#[test]
fn audio_dropping_control_handle_closes_command_channel() {
    let (audio, rx) = mock();
    audio.volume(65);
    drop(audio);
    assert!(matches!(rx.recv().unwrap(), ACmd::Volume(65)));
    assert!(rx.recv().is_err());
}
