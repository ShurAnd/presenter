//! Плеер: список треков на сервере и правила воспроизведения. Звук — в `audio.rs`.
//!
//! Режим общий на плеер: «один трек» или «по списку», плюс «повтор» (один трек по кругу
//! или весь список по кругу). Отдельно — «вставка» (фанфары): выбранный трек играет один раз,
//! музыка на это время стоит на паузе и потом продолжается с того же места.

use std::path::PathBuf;
use std::sync::Arc;

use crate::audio::{Audio, AudioEvent, Deck};
use crate::net::{AudioInfo, AudioState, Cmd, TrackInfo};

pub struct Track {
    pub id: u64,
    pub name: String,
    pub src: PathBuf,
}

/// Состояние одного проигрывателя (музыка или вставка).
#[derive(Default)]
struct DeckState {
    track: Option<u64>,
    state: AudioState,
    pos: f64,
    dur: f64,
    /// Номер загрузки — события прежних загрузок отбрасываются.
    gen: u64,
}

type OnEvent = Arc<dyn Fn(AudioEvent) + Send + Sync>;

pub struct Player {
    pub tracks: Vec<Track>,
    next_id: u64,
    main: DeckState,
    ins: DeckState,
    /// Музыку поставили на паузу ради вставки — после неё продолжить.
    resume: bool,
    pub list: bool,
    pub repeat: bool,
    pub volume: u8,
    /// Сообщение оператору (ошибка воспроизведения) — забирает `App`.
    pub message: Option<String>,
    /// Звуковой поток запускается при первом треке.
    engine: Option<Audio>,
    on_event: OnEvent,
}

impl Player {
    pub fn new(list: bool, repeat: bool, volume: u8, on_event: impl Fn(AudioEvent) + Send + Sync + 'static) -> Player {
        Player {
            tracks: Vec::new(),
            next_id: 1,
            main: DeckState::default(),
            ins: DeckState::default(),
            resume: false,
            list,
            repeat,
            volume: volume.min(100),
            message: None,
            engine: None,
            on_event: Arc::new(on_event),
        }
    }

    fn engine(&mut self) -> &Audio {
        let on_event = self.on_event.clone();
        let volume = self.volume;
        self.engine.get_or_insert_with(|| Audio::start(volume, move |e| on_event(e)))
    }

    pub fn add(&mut self, src: PathBuf, name: String) {
        let id = self.next_id;
        self.next_id += 1;
        self.tracks.push(Track { id, name, src });
    }

    pub fn info(&self) -> AudioInfo {
        AudioInfo {
            tracks: self.tracks.iter().map(|t| TrackInfo { id: t.id, name: t.name.clone() }).collect(),
            current: self.main.track,
            state: self.main.state,
            pos: self.main.pos as u32,
            dur: self.main.dur as u32,
            insert: self.ins.track,
            insert_state: self.ins.state,
            insert_pos: self.ins.pos as u32,
            insert_dur: self.ins.dur as u32,
            list: self.list,
            repeat: self.repeat,
            volume: self.volume,
        }
    }

    fn src(&self, id: u64) -> Option<PathBuf> {
        self.tracks.iter().find(|t| t.id == id).map(|t| t.src.clone())
    }

    /// Один трек по кругу — это делает сам mpv, без перезагрузки файла.
    fn loop_file(&self) -> bool {
        !self.list && self.repeat
    }

    /// Команда плеера. Возвращает файлы, которые больше не нужны (удалённые из списка).
    pub fn apply(&mut self, cmd: Cmd) -> Vec<PathBuf> {
        match cmd {
            Cmd::AudioPlay { id } => self.play(id),
            Cmd::AudioInsert { id } => self.insert(id),
            Cmd::AudioToggle => self.toggle(),
            Cmd::AudioStop => self.stop(),
            Cmd::AudioNext => self.step(true),
            Cmd::AudioPrev => self.step(false),
            Cmd::AudioSeek { secs } => {
                let deck = if self.ins.track.is_some() { Deck::Insert } else { Deck::Main };
                if self.engine.is_some() {
                    self.engine().seek(deck, secs as f64);
                }
            }
            Cmd::AudioMode { list, repeat } => {
                (self.list, self.repeat) = (list, repeat);
                if self.engine.is_some() {
                    let lf = self.loop_file();
                    self.engine().repeat(Deck::Main, lf);
                }
            }
            Cmd::AudioVolume { volume } => {
                self.volume = volume.min(100);
                if self.engine.is_some() {
                    let v = self.volume;
                    self.engine().volume(v);
                }
            }
            Cmd::AudioRemove { id } => {
                if self.main.track == Some(id) {
                    self.halt(Deck::Main);
                    self.main.track = None;
                }
                if self.ins.track == Some(id) {
                    self.halt(Deck::Insert);
                    self.end_insert();
                }
                if let Some(pos) = self.tracks.iter().position(|t| t.id == id) {
                    return vec![self.tracks.remove(pos).src];
                }
            }
            Cmd::AudioMove { id, up } => {
                if let Some(pos) = self.tracks.iter().position(|t| t.id == id) {
                    if up && pos > 0 {
                        self.tracks.swap(pos, pos - 1);
                    } else if !up && pos + 1 < self.tracks.len() {
                        self.tracks.swap(pos, pos + 1);
                    }
                }
            }
            Cmd::AudioClear => {
                self.halt(Deck::Main);
                self.halt(Deck::Insert);
                self.main = DeckState { gen: self.main.gen, ..Default::default() };
                self.end_insert();
                return std::mem::take(&mut self.tracks).into_iter().map(|t| t.src).collect();
            }
            _ => {}
        }
        Vec::new()
    }

    /// Музыка: загрузить трек и играть. Вставка, если звучала, прекращается.
    fn play(&mut self, id: u64) {
        let Some(src) = self.src(id) else { return };
        if self.ins.track.is_some() {
            self.halt(Deck::Insert);
            self.ins.track = None;
            self.ins.state = AudioState::Stopped;
        }
        self.resume = false;
        self.main.gen += 1;
        (self.main.track, self.main.state, self.main.pos, self.main.dur) = (Some(id), AudioState::Playing, 0.0, 0.0);
        let (gen, lf) = (self.main.gen, self.loop_file());
        self.engine().load(Deck::Main, gen, &src, lf);
    }

    /// Вставка: музыку на паузу (если играла), трек — один раз.
    fn insert(&mut self, id: u64) {
        let Some(src) = self.src(id) else { return };
        match self.main.state {
            AudioState::Playing => {
                self.engine().pause(Deck::Main, true);
                self.main.state = AudioState::Paused;
                self.resume = true;
            }
            // Затухающую музыку не возобновляем — её остановили.
            AudioState::Stopping => self.halt(Deck::Main),
            _ => {}
        }
        self.ins.gen += 1;
        (self.ins.track, self.ins.state, self.ins.pos, self.ins.dur) = (Some(id), AudioState::Playing, 0.0, 0.0);
        let gen = self.ins.gen;
        self.engine().load(Deck::Insert, gen, &src, false);
    }

    fn toggle(&mut self) {
        if self.ins.track.is_some() {
            // Пауза вставки — и только её: музыка ждёт своей очереди.
            let pause = self.ins.state == AudioState::Playing;
            self.engine().pause(Deck::Insert, pause);
            self.ins.state = if pause { AudioState::Paused } else { AudioState::Playing };
            return;
        }
        match self.main.state {
            AudioState::Playing => {
                self.engine().pause(Deck::Main, true);
                self.main.state = AudioState::Paused;
            }
            AudioState::Paused => {
                self.engine().pause(Deck::Main, false);
                self.main.state = AudioState::Playing;
            }
            AudioState::Stopped | AudioState::Stopping => {
                if let Some(id) = self.main.track.filter(|id| self.src(*id).is_some()).or_else(|| self.tracks.first().map(|t| t.id)) {
                    self.play(id);
                }
            }
        }
    }

    /// «Стоп»: звучащее — плавно; повторный «Стоп» во время затухания или стоящее на паузе — сразу.
    fn stop(&mut self) {
        self.resume = false;
        for deck in [Deck::Main, Deck::Insert] {
            let d = if deck == Deck::Main { &mut self.main } else { &mut self.ins };
            if d.state == AudioState::Stopped {
                continue;
            }
            let fade = d.state == AudioState::Playing;
            d.state = AudioState::Stopping;
            if let Some(e) = &self.engine {
                e.stop(deck, fade);
            }
        }
    }

    /// Сразу, без затухания.
    fn halt(&mut self, deck: Deck) {
        if self.engine.is_some() {
            self.engine().stop(deck, false);
        }
        let d = if deck == Deck::Main { &mut self.main } else { &mut self.ins };
        d.state = AudioState::Stopped;
        d.gen += 1; // события остановленного больше не нужны
    }

    /// Следующий / предыдущий трек (по кругу) — сразу играть.
    fn step(&mut self, forward: bool) {
        let n = self.tracks.len();
        if n == 0 {
            return;
        }
        let cur = self.main.track.and_then(|id| self.tracks.iter().position(|t| t.id == id));
        let i = match cur {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None => 0,
        };
        let id = self.tracks[i].id;
        self.play(id);
    }

    /// Вставка закончилась (доиграла, остановлена, ошибка): музыка — дальше, если её ради вставки ставили на паузу.
    fn end_insert(&mut self) {
        (self.ins.track, self.ins.state, self.ins.pos, self.ins.dur) = (None, AudioState::Stopped, 0.0, 0.0);
        if std::mem::take(&mut self.resume) && self.main.state == AudioState::Paused {
            self.engine().pause(Deck::Main, false);
            self.main.state = AudioState::Playing;
        }
    }

    /// Музыка доиграла: дальше по режиму.
    fn main_ended(&mut self) {
        self.main.state = AudioState::Stopped;
        self.main.pos = 0.0;
        if !self.list {
            return; // один трек (по кругу его крутит сам mpv, сюда не попадаем)
        }
        let n = self.tracks.len();
        let cur = self.main.track.and_then(|id| self.tracks.iter().position(|t| t.id == id));
        let next = match cur {
            Some(i) if i + 1 < n => Some(i + 1),
            Some(_) if self.repeat => Some(0),
            None if n > 0 && self.repeat => Some(0),
            _ => None,
        };
        if let Some(i) = next {
            let id = self.tracks[i].id;
            self.play(id);
        }
    }

    /// Событие движка. Возвращает true, если состояние для клиентов изменилось.
    pub fn on_event(&mut self, ev: AudioEvent) -> bool {
        let (deck, gen) = match &ev {
            AudioEvent::Pos { deck, gen, .. } | AudioEvent::Ended { deck, gen } | AudioEvent::Stopped { deck, gen } | AudioEvent::Error { deck, gen, .. } => (*deck, *gen),
        };
        let d = if deck == Deck::Main { &mut self.main } else { &mut self.ins };
        if gen != d.gen {
            return false;
        }
        match ev {
            AudioEvent::Pos { pos, dur, .. } => {
                (d.pos, d.dur) = (pos, dur);
            }
            AudioEvent::Ended { .. } => match deck {
                Deck::Main => self.main_ended(),
                Deck::Insert => self.end_insert(),
            },
            AudioEvent::Stopped { .. } => match deck {
                Deck::Main => {
                    (self.main.state, self.main.pos) = (AudioState::Stopped, 0.0);
                }
                Deck::Insert => self.end_insert(),
            },
            AudioEvent::Error { msg, .. } => {
                let id = d.track;
                let name = id.and_then(|id| self.tracks.iter().find(|t| t.id == id)).map(|t| t.name.clone()).unwrap_or_default();
                self.message = Some(format!("Не удалось воспроизвести «{name}»: {msg}"));
                match deck {
                    Deck::Main => self.main.state = AudioState::Stopped,
                    Deck::Insert => self.end_insert(),
                }
            }
        }
        true
    }
}
