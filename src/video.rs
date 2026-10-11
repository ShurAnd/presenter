//! Встроенный видеопроигрыватель. Основной движок — libmpv (FFmpeg внутри, играет практически
//! любые форматы, вшит в exe, см. `mpv.rs`); если он недоступен — Windows Media Foundation (MFPlay),
//! который есть в самой Windows. Ничего докладывать не нужно.
//!
//! Видео играет в собственном окне на весь экран. При открытии окно лежит под сценой с фоном,
//! а видео загружено и стоит на паузе — поэтому по команде «старт» оно начинается сразу:
//! окно выходит наверх и включается воспроизведение, сцена опускается прямо под него
//! (`stage_under_video`, как с PowerPoint). Окно не забирает фокус — клавиши по-прежнему ловит сцена.
//!
//! Через «поверх всех» (HWND_TOPMOST) окно видео не поднимаем: на части компьютеров Windows молча
//! не пускает его в этот слой (найдено 11.10.2026 — звук идёт, а на экране фон; та же картина и у
//! версии 1.0.4), хотя обычное окно с теми же стилями туда пускает. Обычный HWND_TOP работает везде.

use std::path::Path;
use std::sync::mpsc::{channel, Sender};

pub enum VideoEvent {
    /// Видео загружено и готово к запуску; длительность в секундах.
    Ready(f64),
    /// Позиция и длительность в секундах; идёт ли воспроизведение.
    Pos { pos: f64, dur: f64, playing: bool },
    /// Видео доиграло до конца.
    Ended,
    Error(String),
}

enum VCmd {
    Start,
    Toggle,
    Seek(f64),
    SeekTo(f64),
    Quit,
}

/// Пока жив этот объект — видео открыто. При уничтожении проигрыватель и его окно закрываются.
pub struct Video {
    tx: Sender<VCmd>,
    #[cfg(windows)]
    thread: u32,
}

impl Video {
    /// `rect` — монитор для показа (физические пиксели); None — основной монитор.
    pub fn start(src: &Path, windowed: bool, rect: Option<(i32, i32, i32, i32)>, on_event: impl Fn(VideoEvent) + Send + 'static) -> Video {
        let (tx, rx) = channel();
        #[cfg(windows)]
        {
            let (id_tx, id_rx) = channel();
            let src = src.to_path_buf();
            std::thread::spawn(move || win::run(&src, windowed, rect, rx, id_tx, on_event));
            let thread = id_rx.recv().unwrap_or(0);
            Video { tx, thread }
        }
        #[cfg(not(windows))]
        {
            let _ = (src, windowed, rect, rx);
            on_event(VideoEvent::Error("видео поддерживается только в Windows".into()));
            Video { tx }
        }
    }

    fn send(&self, c: VCmd) {
        let _ = self.tx.send(c);
        #[cfg(windows)]
        win::wake(self.thread);
    }

    /// Вывести видео поверх сцены и начать воспроизведение.
    pub fn play(&self) {
        self.send(VCmd::Start);
    }

    /// Пауза / продолжить.
    pub fn toggle(&self) {
        self.send(VCmd::Toggle);
    }

    /// Перемотка на `secs` секунд вперёд (или назад, если отрицательно).
    pub fn seek(&self, secs: f64) {
        self.send(VCmd::Seek(secs));
    }

    pub fn seek_to(&self, secs: f64) {
        self.send(VCmd::SeekTo(secs));
    }
}

/// Пока видео идёт: окно видео наверху, сцена прямо под ним (см. `win::stage_under_video`).
pub fn stage_under_video(stage_title: &str) {
    #[cfg(windows)]
    win::stage_under_video(stage_title);
    #[cfg(not(windows))]
    let _ = stage_title;
}

impl Drop for Video {
    fn drop(&mut self) {
        self.send(VCmd::Quit);
    }
}

#[cfg(windows)]
mod win {
    use super::{VCmd, VideoEvent};
    use crate::mpv;
    use std::path::Path;
    use std::sync::mpsc::{Receiver, Sender};
    use windows::core::{w, HSTRING, PCWSTR, PROPVARIANT};
    use windows::Win32::Foundation::{BOOL, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::{GetStockObject, BLACK_BRUSH, HBRUSH};
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::*;

    const TICK_MS: u32 = 100;
    /// Сколько ждать загрузки файла, прежде чем сообщить об ошибке.
    const OPEN_TIMEOUT_TICKS: u32 = 20_000 / TICK_MS;

    /// Что движок сообщает на очередном такте.
    #[derive(Default)]
    struct Tick {
        /// Файл загружен, первый кадр готов; длительность.
        ready: Option<f64>,
        pos: f64,
        dur: f64,
        playing: bool,
        /// Воспроизведение дошло до конца.
        eof: bool,
        error: Option<String>,
    }

    /// Движок воспроизведения: libmpv или MFPlay.
    trait Engine {
        fn tick(&mut self) -> Tick;
        fn play(&mut self);
        fn toggle(&mut self);
        fn seek(&mut self, secs: f64, relative: bool);
    }

    // ------------------------------------------------------------ libmpv

    struct MpvEngine {
        mpv: mpv::Mpv,
        ready: bool,
    }

    impl MpvEngine {
        fn new(hwnd: HWND, src: &Path) -> Result<MpvEngine, String> {
            let wid = (hwnd.0 as isize).to_string();
            let mpv = mpv::Mpv::new(&[
                ("wid", &wid),
                ("pause", "yes"),     // загружаем и ждём команды «старт»
                ("keep-open", "yes"), // в конце остаёмся на последнем кадре, закрываем сами
                ("hwdec", "auto-safe"),
                ("osc", "no"),
                ("osd-level", "0"),
                ("input-default-bindings", "no"),
                ("input-vo-keyboard", "no"),
                ("input-cursor", "no"),
                ("cursor-autohide", "always"),
                ("audio-display", "no"),
                ("force-window", "yes"),
                ("terminal", "no"),
            ])?;
            mpv.command(&["loadfile", &src.to_string_lossy()])?;
            Ok(MpvEngine { mpv, ready: false })
        }
    }

    impl Engine for MpvEngine {
        fn tick(&mut self) -> Tick {
            let mut t = Tick::default();
            for (id, _, end) in self.mpv.events() {
                match id {
                    mpv::EVENT_PLAYBACK_RESTART if !self.ready => {
                        self.ready = true;
                        t.ready = Some(self.mpv.get_f64("duration").unwrap_or(0.0));
                    }
                    mpv::EVENT_END_FILE => {
                        if let Some((mpv::END_FILE_REASON_ERROR, code)) = end {
                            t.error = Some(format!("не удалось открыть видео: {}", self.mpv.error(code)));
                        }
                    }
                    mpv::EVENT_SHUTDOWN => t.error = Some("проигрыватель завершился".into()),
                    _ => {}
                }
            }
            if self.ready {
                t.pos = self.mpv.get_f64("time-pos").unwrap_or(0.0);
                t.dur = self.mpv.get_f64("duration").unwrap_or(0.0);
                t.playing = self.mpv.get_flag("pause") == Some(false);
                t.eof = self.mpv.get_flag("eof-reached") == Some(true);
            }
            t
        }

        fn play(&mut self) {
            let _ = self.mpv.set("pause", "no");
        }

        fn toggle(&mut self) {
            let _ = self.mpv.command(&["cycle", "pause"]);
        }

        fn seek(&mut self, secs: f64, relative: bool) {
            let _ = self.mpv.command(&["seek", &format!("{secs:.3}"), if relative { "relative" } else { "absolute" }]);
        }
    }

    // ------------------------------------------------------------ MFPlay (запасной)

    struct MfEngine {
        player: IMFPMediaPlayer,
        ready: bool,
        /// Когда нажали «старт».
        started: Option<std::time::Instant>,
        /// Видео действительно пошло (MFPlay был в состоянии «играет»).
        played: bool,
    }

    fn secs(p: windows::core::Result<PROPVARIANT>) -> f64 {
        p.ok().and_then(|v| i64::try_from(&v).ok()).map_or(0.0, |t| t as f64 / 1e7)
    }

    impl MfEngine {
        unsafe fn new(hwnd: HWND, src: &Path) -> windows::core::Result<MfEngine> {
            let mut player: Option<IMFPMediaPlayer> = None;
            MFPCreateMediaPlayer(PCWSTR::null(), BOOL::from(false), MFP_OPTION_NONE, None, hwnd, Some(&mut player))?;
            let player = player.ok_or_else(windows::core::Error::empty)?;
            let mut item: Option<IMFPMediaItem> = None;
            player.CreateMediaItemFromURL(&HSTRING::from(src.as_os_str()), BOOL::from(true), 0, Some(&mut item))?;
            player.SetMediaItem(&item.ok_or_else(windows::core::Error::empty)?)?;
            Ok(MfEngine { player, ready: false, started: None, played: false })
        }
    }

    impl Drop for MfEngine {
        fn drop(&mut self) {
            unsafe {
                let _ = self.player.Shutdown();
            }
        }
    }

    impl Engine for MfEngine {
        fn tick(&mut self) -> Tick {
            let mut t = Tick::default();
            unsafe {
                let state = self.player.GetState().unwrap_or(MFP_MEDIAPLAYER_STATE_EMPTY);
                let dur = secs(self.player.GetDuration(&MFP_POSITIONTYPE_100NS));
                if !self.ready {
                    if state == MFP_MEDIAPLAYER_STATE_STOPPED {
                        // Файл открыт. Пауза выводит первый кадр и готовит воспроизведение.
                        let _ = self.player.Pause();
                        self.ready = true;
                        t.ready = Some(dur);
                    }
                    return t;
                }
                t.pos = secs(self.player.GetPosition(&MFP_POSITIONTYPE_100NS));
                t.dur = dur;
                t.playing = state == MFP_MEDIAPLAYER_STATE_PLAYING;
                self.played |= t.playing;
                // В конце файла MFPlay сам переходит в «остановлено». Сразу после Play() он тоже ещё
                // «остановлен» — поэтому конец засчитываем, только если видео уже успело поиграть.
                t.eof = self.played && state == MFP_MEDIAPLAYER_STATE_STOPPED;
                if self.started.is_some_and(|s| !self.played && s.elapsed() > std::time::Duration::from_secs(8)) {
                    t.error = Some("Windows не смог воспроизвести это видео".into());
                }
            }
            t
        }

        fn play(&mut self) {
            self.started = Some(std::time::Instant::now());
            unsafe {
                let _ = self.player.Play();
            }
        }

        fn toggle(&mut self) {
            unsafe {
                if self.player.GetState().ok() == Some(MFP_MEDIAPLAYER_STATE_PLAYING) {
                    let _ = self.player.Pause();
                } else {
                    let _ = self.player.Play();
                }
            }
        }

        fn seek(&mut self, d: f64, relative: bool) {
            unsafe {
                let dur = secs(self.player.GetDuration(&MFP_POSITIONTYPE_100NS));
                let base = if relative { secs(self.player.GetPosition(&MFP_POSITIONTYPE_100NS)) } else { 0.0 };
                let to = (base + d).clamp(0.0, (dur - 0.5).max(0.0));
                let _ = self.player.SetPosition(&MFP_POSITIONTYPE_100NS, &PROPVARIANT::from((to * 1e7) as i64));
            }
        }
    }

    // ------------------------------------------------------------ окно и цикл

    /// Пока видео идёт: окно видео наверху, сцена прямо под ним. Вызывается из GUI на каждой
    /// перерисовке сцены; если порядок уже такой — ничего не делает.
    pub fn stage_under_video(stage_title: &str) {
        unsafe {
            let Ok(video) = FindWindowW(w!("PresentatorVideo"), PCWSTR::null()) else { return };
            let Ok(stage) = FindWindowW(PCWSTR::null(), &HSTRING::from(stage_title)) else { return };
            if GetWindow(video, GW_HWNDNEXT).ok() == Some(stage) {
                return;
            }
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
            let _ = SetWindowPos(video, HWND_TOP, 0, 0, 0, 0, flags);
            let _ = SetWindowPos(stage, video, 0, 0, 0, 0, flags);
        }
    }

    pub fn wake(thread: u32) {
        unsafe {
            let _ = PostThreadMessageW(thread, WM_APP, WPARAM(0), LPARAM(0));
        }
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        match msg {
            // Курсор над видео не показываем.
            WM_SETCURSOR => {
                SetCursor(None);
                LRESULT(1)
            }
            // Окно не забирает фокус у сцены даже по щелчку.
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }

    unsafe fn create_window(windowed: bool, rect: Option<(i32, i32, i32, i32)>) -> windows::core::Result<HWND> {
        let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = w!("PresentatorVideo");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class,
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            ..Default::default()
        };
        RegisterClassW(&wc); // повторная регистрация вернёт ошибку — это нормально
        let (x, y, cx, cy) = match (windowed, rect) {
            (true, _) => (120, 120, 960, 540),
            (false, Some(r)) => r,
            (false, None) => (0, 0, GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)),
        };
        CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class, w!("Presentator — видео"), WS_POPUP, x, y, cx, cy, None, None, hinst, None)
    }

    pub fn run(src: &Path, windowed: bool, rect: Option<(i32, i32, i32, i32)>, rx: Receiver<VCmd>, id_tx: Sender<u32>, on_event: impl Fn(VideoEvent)) {
        unsafe {
            // Очередь сообщений потока должна существовать до того, как в неё начнут писать.
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
            let _ = id_tx.send(GetCurrentThreadId());
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            match create_window(windowed, rect) {
                Ok(hwnd) => {
                    // Окно видно, но лежит под сценой (она поверх всех окон): в него уже выводится первый кадр.
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    // Явно в самый низ: новое окно иначе встаёт выше обычных окон, и если сцена вдруг
                    // не «поверх всех», зритель увидел бы первый кадр до «Старт».
                    let _ = SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                    let engine: Result<Box<dyn Engine>, String> = match MpvEngine::new(hwnd, src) {
                        Ok(e) => Ok(Box::new(e)),
                        // libmpv недоступен — играем движком Windows.
                        Err(mpv_err) => MfEngine::new(hwnd, src)
                            .map(|e| Box::new(e) as Box<dyn Engine>)
                            .map_err(|e| format!("{mpv_err}; Media Foundation: {}", e.message())),
                    };
                    match engine {
                        Ok(mut e) => event_loop(e.as_mut(), hwnd, &rx, &on_event),
                        Err(e) => on_event(VideoEvent::Error(e)),
                    }
                    let _ = DestroyWindow(hwnd);
                }
                Err(e) => on_event(VideoEvent::Error(e.message().to_string())),
            }
            CoUninitialize();
        }
    }

    unsafe fn event_loop(engine: &mut dyn Engine, hwnd: HWND, rx: &Receiver<VCmd>, on_event: &impl Fn(VideoEvent)) {
        let timer = SetTimer(None, 0, TICK_MS, None);
        let (mut ready, mut started, mut ended) = (false, false, false);
        let mut last_pos: Option<(u32, u32, bool)> = None;
        let mut ticks = 0u32;
        let mut msg = MSG::default();
        'outer: while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.hwnd.is_invalid() && msg.message == WM_APP {
                while let Ok(c) = rx.try_recv() {
                    match c {
                        VCmd::Quit => break 'outer,
                        VCmd::Start if ready && !started => {
                            started = true;
                            // Наверх, не отнимая фокус, — и сразу воспроизведение (сцену под окно видео опустит GUI).
                            let _ = SetWindowPos(hwnd, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                            engine.play();
                        }
                        VCmd::Toggle if started => engine.toggle(),
                        VCmd::Seek(d) if ready => engine.seek(d, true),
                        VCmd::SeekTo(d) if ready => engine.seek(d, false),
                        _ => {}
                    }
                }
                continue;
            }
            if msg.hwnd.is_invalid() && msg.message == WM_TIMER && msg.wParam.0 == timer {
                ticks += 1;
                let t = engine.tick();
                if let Some(e) = t.error {
                    on_event(VideoEvent::Error(e));
                    break;
                }
                if let Some(dur) = t.ready {
                    ready = true;
                    on_event(VideoEvent::Ready(dur));
                }
                if !ready {
                    if ticks > OPEN_TIMEOUT_TICKS {
                        on_event(VideoEvent::Error("не удалось открыть видео".into()));
                        break;
                    }
                    continue;
                }

                // Сообщаем только об изменениях: иначе GUI 10 раз в секунду перерисовывал бы окна впустую.
                let now = (t.pos as u32, t.dur as u32, t.playing);
                if last_pos != Some(now) {
                    last_pos = Some(now);
                    on_event(VideoEvent::Pos { pos: t.pos, dur: t.dur, playing: t.playing });
                }
                if started && !ended && t.eof {
                    ended = true;
                    on_event(VideoEvent::Ended);
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = KillTimer(None, timer);
    }
}
