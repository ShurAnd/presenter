//! Stage 5: player state transitions and stale event filtering, without libmpv/audio hardware.
use super::*;

fn mock() -> Player {
    Player::new(false, false, 70, |_| {})
}

fn add(p: &mut Player, name: &str) -> u64 {
    p.add(PathBuf::from(format!("C:/fake/{name}")), name.into());
    p.tracks.last().unwrap().id
}

#[test]
fn player_add_assigns_distinct_ids_and_preserves_order() {
    let mut p = mock();
    let a = add(&mut p, "intro.mp3");
    let b = add(&mut p, "outro.mp3");
    assert_ne!(a, b);
    let info = p.info();
    assert_eq!(info.tracks.len(), 2);
    assert_eq!(info.tracks[0].id, a);
    assert_eq!(info.tracks[1].id, b);
    assert_eq!(info.tracks[0].name, "intro.mp3");
}

#[test]
fn player_volume_clamps_and_mode_changes_without_starting_engine() {
    let mut p = Player::new(false, false, 255, |_| {});
    assert_eq!(p.volume, 100);
    p.apply(Cmd::AudioVolume { volume: 255 });
    assert_eq!(p.info().volume, 100);
    p.apply(Cmd::AudioVolume { volume: 0 });
    p.apply(Cmd::AudioMode { list: true, repeat: true });
    let info = p.info();
    assert_eq!(info.volume, 0);
    assert!(info.list && info.repeat);
    assert!(p.engine.is_none());
}

#[test]
fn player_moves_tracks_and_ignores_out_of_range_moves() {
    let mut p = mock();
    let a = add(&mut p, "a.mp3");
    let b = add(&mut p, "b.mp3");
    let c = add(&mut p, "c.mp3");
    p.apply(Cmd::AudioMove { id: a, up: true });
    assert_eq!(p.tracks[0].id, a);
    p.apply(Cmd::AudioMove { id: c, up: false });
    assert_eq!(p.tracks[2].id, c);
    p.apply(Cmd::AudioMove { id: c, up: true });
    assert_eq!(p.tracks.iter().map(|t| t.id).collect::<Vec<_>>(), vec![a, c, b]);
    p.apply(Cmd::AudioMove { id: 999_999, up: false });
    assert_eq!(p.tracks.iter().map(|t| t.id).collect::<Vec<_>>(), vec![a, c, b]);
}

#[test]
fn player_remove_returns_only_removed_file_and_retains_other_tracks() {
    let mut p = mock();
    let first = add(&mut p, "first.mp3");
    let second = add(&mut p, "second.mp3");
    assert!(p.apply(Cmd::AudioRemove { id: 999 }).is_empty());
    assert_eq!(p.apply(Cmd::AudioRemove { id: first }), vec![PathBuf::from("C:/fake/first.mp3")]);
    assert_eq!(p.tracks.len(), 1);
    assert_eq!(p.tracks[0].id, second);
}

#[test]
fn player_remove_active_track_invalidates_late_events() {
    let mut p = mock();
    let id = add(&mut p, "current.mp3");
    p.main.track = Some(id);
    p.main.state = AudioState::Playing;
    p.main.gen = 5;
    p.apply(Cmd::AudioRemove { id });
    assert_eq!(p.main.track, None);
    assert_eq!(p.main.state, AudioState::Stopped);
    assert_eq!(p.main.gen, 6);
    assert!(!p.on_event(AudioEvent::Pos { deck: Deck::Main, gen: 5, pos: 9.0, dur: 60.0 }));
}

#[test]
fn player_clear_returns_paths_and_resets_both_decks() {
    let mut p = mock();
    let first = add(&mut p, "first.mp3");
    let second = add(&mut p, "second.mp3");
    p.main.track = Some(first);
    p.main.state = AudioState::Paused;
    p.main.gen = 3;
    p.ins.track = Some(second);
    p.ins.state = AudioState::Paused;
    p.ins.gen = 9;
    let removed = p.apply(Cmd::AudioClear);
    assert_eq!(removed, vec![PathBuf::from("C:/fake/first.mp3"), PathBuf::from("C:/fake/second.mp3")]);
    assert!(p.tracks.is_empty());
    assert_eq!(p.main.track, None);
    assert_eq!(p.ins.track, None);
    assert_eq!(p.main.state, AudioState::Stopped);
    assert_eq!(p.ins.state, AudioState::Stopped);
    assert!(p.main.gen > 3);
    assert!(p.ins.gen > 9);
}

#[test]
fn player_ignores_stale_position_and_error_events() {
    let mut p = mock();
    let id = add(&mut p, "sample.mp3");
    p.main.track = Some(id);
    p.main.gen = 8;
    p.main.state = AudioState::Playing;
    assert!(!p.on_event(AudioEvent::Pos { deck: Deck::Main, gen: 7, pos: 50.0, dur: 100.0 }));
    assert!(!p.on_event(AudioEvent::Error { deck: Deck::Main, gen: 7, msg: "obsolete".into() }));
    assert_eq!(p.info().pos, 0);
    assert_eq!(p.main.state, AudioState::Playing);
    assert!(p.message.is_none());
}

#[test]
fn player_updates_only_the_matching_deck_on_position_events() {
    let mut p = mock();
    p.main.gen = 4;
    p.ins.gen = 6;
    assert!(p.on_event(AudioEvent::Pos { deck: Deck::Main, gen: 4, pos: 11.4, dur: 90.5 }));
    assert_eq!(p.info().pos, 11);
    assert_eq!(p.info().dur, 90);
    assert_eq!(p.info().insert_pos, 0);
    assert!(p.on_event(AudioEvent::Pos { deck: Deck::Insert, gen: 6, pos: 2.0, dur: 4.0 }));
    assert_eq!(p.info().insert_pos, 2);
    assert_eq!(p.info().pos, 11);
}

#[test]
fn player_single_track_end_stops_and_rewinds() {
    let mut p = mock();
    let id = add(&mut p, "one.mp3");
    p.main.track = Some(id);
    p.main.gen = 10;
    p.main.pos = 88.0;
    p.main.state = AudioState::Playing;
    assert!(p.on_event(AudioEvent::Ended { deck: Deck::Main, gen: 10 }));
    assert_eq!(p.main.state, AudioState::Stopped);
    assert_eq!(p.main.pos, 0.0);
    assert_eq!(p.main.track, Some(id));
    assert!(p.engine.is_none());
}

#[test]
fn player_insert_end_clears_insertion_without_resuming_unpaused_main() {
    let mut p = mock();
    let id = add(&mut p, "insert.wav");
    p.main.state = AudioState::Stopped;
    p.ins.track = Some(id);
    p.ins.state = AudioState::Playing;
    p.ins.gen = 2;
    p.ins.pos = 1.5;
    assert!(p.on_event(AudioEvent::Ended { deck: Deck::Insert, gen: 2 }));
    assert!(p.ins.track.is_none());
    assert_eq!(p.ins.state, AudioState::Stopped);
    assert_eq!(p.ins.pos, 0.0);
    assert_eq!(p.main.state, AudioState::Stopped);
    assert!(p.engine.is_none());
}

#[test]
fn player_insert_stop_resets_state_without_affecting_music() {
    let mut p = mock();
    p.main.state = AudioState::Playing;
    p.ins.gen = 3;
    p.ins.track = Some(200);
    p.ins.state = AudioState::Paused;
    assert!(p.on_event(AudioEvent::Stopped { deck: Deck::Insert, gen: 3 }));
    assert!(p.ins.track.is_none());
    assert_eq!(p.main.state, AudioState::Playing);
    assert!(p.engine.is_none());
}

#[test]
fn player_error_reports_track_name_and_stops_main() {
    let mut p = mock();
    let id = add(&mut p, "broken.mp3");
    p.main.track = Some(id);
    p.main.gen = 17;
    p.main.state = AudioState::Playing;
    assert!(p.on_event(AudioEvent::Error { deck: Deck::Main, gen: 17, msg: "decode failed".into() }));
    assert_eq!(p.main.state, AudioState::Stopped);
    let msg = p.message.as_deref().unwrap();
    assert!(msg.contains("broken.mp3"));
    assert!(msg.contains("decode failed"));
}

#[test]
fn player_stop_does_not_spawn_audio_device_when_no_engine_exists() {
    let mut p = mock();
    p.main.state = AudioState::Playing;
    p.ins.state = AudioState::Paused;
    p.resume = true;
    p.apply(Cmd::AudioStop);
    assert_eq!(p.main.state, AudioState::Stopping);
    assert_eq!(p.ins.state, AudioState::Stopping);
    assert!(!p.resume);
    assert!(p.engine.is_none());
}

#[test]
fn player_no_tracks_navigation_and_seek_are_harmless() {
    let mut p = mock();
    for cmd in [Cmd::AudioNext, Cmd::AudioPrev, Cmd::AudioSeek { secs: 500 }, Cmd::AudioToggle] {
        assert!(p.apply(cmd).is_empty());
    }
    assert_eq!(p.main.track, None);
    assert!(p.engine.is_none());
}
