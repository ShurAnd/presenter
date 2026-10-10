//! Звук плеера: свой поток и два экземпляра libmpv без окна — «музыка» и «вставка» (фанфары).
//! Пока играет вставка, музыка стоит на паузе в своём экземпляре и потом продолжается с того же
//! места, без перезагрузки файла. Что и когда играть, решает `player.rs`; здесь — только движок.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};

/// Какой из двух проигрывателей.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Deck {
    Main = 0,
    Insert = 1,
}

/// События движка. `gen` — номер загрузки (у каждой новой — свой), события старых отбрасываются.
#[derive(Debug)]
pub enum AudioEvent {
    /// Позиция и длительность в секундах (сообщается при смене секунды или паузы).
    Pos { deck: Deck, gen: u64, pos: f64, dur: f64 },
    /// Трек доиграл до конца.
    Ended { deck: Deck, gen: u64 },
    /// Остановлено по команде (после затухания).
    Stopped { deck: Deck, gen: u64 },
    Error { deck: Deck, gen: u64, msg: String },
}

enum ACmd {
    Load { deck: Deck, gen: u64, path: PathBuf, repeat: bool },
    Pause(Deck, bool),
    Stop { deck: Deck, fade: bool },
    Seek(Deck, f64),
    Repeat(Deck, bool),
    Volume(u8),
}

/// Сколько длится плавное затухание при «Стоп».
pub const FADE_SECS: f64 = 2.0;

/// Пока жив этот объект — жив поток звука. При уничтожении звук обрывается.
pub struct Audio {
    tx: Sender<ACmd>,
}

impl Audio {
    pub fn start(volume: u8, on_event: impl Fn(AudioEvent) + Send + 'static) -> Audio {
        let (tx, rx) = channel();
        #[cfg(windows)]
        std::thread::spawn(move || win::run(rx, volume, on_event));
        #[cfg(not(windows))]
        {
            let _ = (rx, volume, on_event);
        }
        Audio { tx }
    }

    fn send(&self, c: ACmd) {
        let _ = self.tx.send(c);
    }

    /// Загрузить и сразу играть. `repeat` — крутить этот трек по кругу.
    pub fn load(&self, deck: Deck, gen: u64, path: &Path, repeat: bool) {
        self.send(ACmd::Load { deck, gen, path: path.to_path_buf(), repeat });
    }

    pub fn pause(&self, deck: Deck, on: bool) {
        self.send(ACmd::Pause(deck, on));
    }

    /// Остановить: `fade` — плавно, за FADE_SECS.
    pub fn stop(&self, deck: Deck, fade: bool) {
        self.send(ACmd::Stop { deck, fade });
    }

    pub fn seek(&self, deck: Deck, secs: f64) {
        self.send(ACmd::Seek(deck, secs));
    }

    pub fn repeat(&self, deck: Deck, on: bool) {
        self.send(ACmd::Repeat(deck, on));
    }

    /// Громкость обоих проигрывателей, 0–100.
    pub fn volume(&self, v: u8) {
        self.send(ACmd::Volume(v));
    }
}

#[cfg(windows)]
mod win {
    use super::{ACmd, AudioEvent, Deck, FADE_SECS};
    use crate::mpv;
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::time::{Duration, Instant};

    /// mpv END_FILE: доиграл до конца.
    const END_FILE_REASON_EOF: i32 = 0;

    /// Один проигрыватель.
    struct Player {
        mpv: Option<mpv::Mpv>,
        gen: u64,
        loaded: bool,
        /// Идёт затухание (с этого момента).
        fade: Option<Instant>,
        last: Option<(u32, u32, bool)>,
    }

    impl Player {
        fn new() -> Player {
            Player { mpv: None, gen: 0, loaded: false, fade: None, last: None }
        }

        /// Экземпляр mpv создаётся при первой загрузке.
        fn mpv(&mut self) -> Result<&mpv::Mpv, String> {
            if self.mpv.is_none() {
                self.mpv = Some(mpv::Mpv::new(&[
                    ("vid", "no"), // обложки и видео в файле не нужны
                    ("audio-display", "no"),
                    ("idle", "yes"), // без файла не завершаться
                    ("keep-open", "no"),
                    ("force-window", "no"),
                    ("osc", "no"),
                    ("input-default-bindings", "no"),
                    ("terminal", "no"),
                    ("audio-client-name", "Presentator"),
                ])?);
            }
            Ok(self.mpv.as_ref().unwrap())
        }

        fn set_volume(&self, v: f64) {
            if let Some(m) = &self.mpv {
                let _ = m.set("volume", &format!("{v:.1}"));
            }
        }

        fn stop_now(&mut self) {
            if let Some(m) = &self.mpv {
                let _ = m.command(&["stop"]);
            }
            self.loaded = false;
            self.fade = None;
            self.last = None;
        }
    }

    pub fn run(rx: Receiver<ACmd>, volume: u8, on_event: impl Fn(AudioEvent)) {
        let mut decks = [Player::new(), Player::new()];
        let mut volume = volume as f64;
        loop {
            // Пока идёт затухание — часто, чтобы громкость спадала плавно.
            let fading = decks.iter().any(|d| d.fade.is_some());
            let wait = if fading { Duration::from_millis(40) } else { Duration::from_millis(200) };
            match rx.recv_timeout(wait) {
                Ok(c) => {
                    handle(c, &mut decks, &mut volume, &on_event);
                    while let Ok(c) = rx.try_recv() {
                        handle(c, &mut decks, &mut volume, &on_event);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break, // Audio уничтожен — звук обрывается вместе с mpv
            }
            for (i, d) in decks.iter_mut().enumerate() {
                let deck = if i == 0 { Deck::Main } else { Deck::Insert };
                poll(deck, d, volume, &on_event);
            }
        }
    }

    fn handle(c: ACmd, decks: &mut [Player; 2], volume: &mut f64, on_event: &impl Fn(AudioEvent)) {
        match c {
            ACmd::Load { deck, gen, path, repeat } => {
                let d = &mut decks[deck as usize];
                d.gen = gen;
                d.fade = None;
                d.last = None;
                let res = d.mpv().and_then(|m| {
                    m.set("loop-file", if repeat { "inf" } else { "no" })?;
                    m.set("volume", &format!("{volume:.1}"))?;
                    m.set("pause", "no")?;
                    m.command(&["loadfile", &path.to_string_lossy(), "replace"])
                });
                match res {
                    Ok(()) => d.loaded = true,
                    Err(msg) => on_event(AudioEvent::Error { deck, gen, msg }),
                }
            }
            ACmd::Pause(deck, on) => {
                if let Some(m) = &decks[deck as usize].mpv {
                    let _ = m.set("pause", if on { "yes" } else { "no" });
                }
            }
            ACmd::Stop { deck, fade } => {
                let d = &mut decks[deck as usize];
                let playing = d.loaded && d.mpv.as_ref().is_some_and(|m| m.get_flag("pause") == Some(false));
                if fade && playing && d.fade.is_none() {
                    d.fade = Some(Instant::now());
                } else {
                    d.stop_now();
                    d.set_volume(*volume);
                    on_event(AudioEvent::Stopped { deck, gen: d.gen });
                }
            }
            ACmd::Seek(deck, secs) => {
                if let Some(m) = &decks[deck as usize].mpv {
                    let _ = m.command(&["seek", &format!("{secs:.2}"), "absolute"]);
                }
            }
            ACmd::Repeat(deck, on) => {
                if let Some(m) = &decks[deck as usize].mpv {
                    let _ = m.set("loop-file", if on { "inf" } else { "no" });
                }
            }
            ACmd::Volume(v) => {
                *volume = v as f64;
                for d in decks.iter().filter(|d| d.fade.is_none()) {
                    d.set_volume(*volume);
                }
            }
        }
    }

    fn poll(deck: Deck, d: &mut Player, volume: f64, on_event: &impl Fn(AudioEvent)) {
        let Some(m) = &d.mpv else { return };
        let gen = d.gen;
        for (id, _, end) in m.events() {
            if id != mpv::EVENT_END_FILE || !d.loaded {
                continue;
            }
            // Конец предыдущего файла при замене (причина «stop») нас не интересует.
            match end {
                Some((END_FILE_REASON_EOF, _)) => {
                    d.loaded = false;
                    d.last = None;
                    on_event(AudioEvent::Ended { deck, gen });
                }
                Some((mpv::END_FILE_REASON_ERROR, code)) => {
                    d.loaded = false;
                    d.last = None;
                    on_event(AudioEvent::Error { deck, gen, msg: m.error(code) });
                }
                _ => {}
            }
        }
        if let Some(t) = d.fade {
            let k = t.elapsed().as_secs_f64() / FADE_SECS;
            if k >= 1.0 {
                d.stop_now();
                d.set_volume(volume);
                on_event(AudioEvent::Stopped { deck, gen });
                return;
            }
            d.set_volume(volume * (1.0 - k));
        }
        if !d.loaded {
            return;
        }
        let pos = m.get_f64("time-pos").unwrap_or(0.0);
        let dur = m.get_f64("duration").unwrap_or(0.0);
        let playing = m.get_flag("pause") == Some(false) && m.get_flag("core-idle") != Some(true);
        let now = (pos as u32, dur as u32, playing);
        if d.last != Some(now) {
            d.last = Some(now);
            on_event(AudioEvent::Pos { deck, gen, pos, dur });
        }
    }
}
