//! Встроенный видеопроигрыватель на Windows Media Foundation (MFPlay) — входит в Windows,
//! ничего докладывать не нужно.
//!
//! Видео играет в собственном окне на весь экран. При открытии окно лежит под сценой с фоном,
//! а видео загружено и стоит на паузе — поэтому по команде «старт» оно начинается сразу:
//! окно выходит поверх сцены и включается воспроизведение. Окно не забирает фокус —
//! клавиши по-прежнему ловит сцена.

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
    pub fn start(src: &Path, windowed: bool, on_event: impl Fn(VideoEvent) + Send + 'static) -> Video {
        let (tx, rx) = channel();
        #[cfg(windows)]
        {
            let (id_tx, id_rx) = channel();
            let src = src.to_path_buf();
            std::thread::spawn(move || win::run(&src, windowed, rx, id_tx, on_event));
            let thread = id_rx.recv().unwrap_or(0);
            Video { tx, thread }
        }
        #[cfg(not(windows))]
        {
            let _ = (src, windowed, rx);
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

impl Drop for Video {
    fn drop(&mut self) {
        self.send(VCmd::Quit);
    }
}

#[cfg(windows)]
mod win {
    use super::{VCmd, VideoEvent};
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

    const TICK_MS: u32 = 200;
    /// Сколько ждать загрузки файла, прежде чем сообщить об ошибке.
    const OPEN_TIMEOUT_TICKS: u32 = 15_000 / TICK_MS;

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

    unsafe fn create_window(windowed: bool) -> windows::core::Result<HWND> {
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
        let (x, y, cx, cy) = if windowed {
            (120, 120, 960, 540)
        } else {
            (0, 0, GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
        };
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!("Presentator — видео"),
            WS_POPUP,
            x,
            y,
            cx,
            cy,
            None,
            None,
            hinst,
            None,
        )
    }

    fn secs(p: windows::core::Result<PROPVARIANT>) -> f64 {
        p.ok().and_then(|v| i64::try_from(&v).ok()).map_or(0.0, |t| t as f64 / 1e7)
    }

    pub fn run(src: &Path, windowed: bool, rx: Receiver<VCmd>, id_tx: Sender<u32>, on_event: impl Fn(VideoEvent)) {
        unsafe {
            // Очередь сообщений потока должна существовать до того, как в неё начнут писать.
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
            let _ = id_tx.send(GetCurrentThreadId());
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            if let Err(e) = play_loop(src, windowed, &rx, &on_event) {
                on_event(VideoEvent::Error(e.message().to_string()));
            }
            CoUninitialize();
        }
    }

    unsafe fn play_loop(src: &Path, windowed: bool, rx: &Receiver<VCmd>, on_event: &impl Fn(VideoEvent)) -> windows::core::Result<()> {
        let hwnd = create_window(windowed)?;
        // Окно видно, но лежит под сценой (она поверх всех окон): в него уже выводится первый кадр.
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let result = (|| {
            let mut player: Option<IMFPMediaPlayer> = None;
            MFPCreateMediaPlayer(PCWSTR::null(), BOOL::from(false), MFP_OPTION_NONE, None, hwnd, Some(&mut player))?;
            let player = player.ok_or_else(windows::core::Error::empty)?;
            let mut item: Option<IMFPMediaItem> = None;
            player.CreateMediaItemFromURL(&HSTRING::from(src.as_os_str()), BOOL::from(true), 0, Some(&mut item))?;
            let item = item.ok_or_else(windows::core::Error::empty)?;
            player.SetMediaItem(&item)?;
            let r = event_loop(&player, hwnd, rx, on_event);
            let _ = player.Shutdown();
            r
        })();
        let _ = DestroyWindow(hwnd);
        result
    }

    unsafe fn event_loop(player: &IMFPMediaPlayer, hwnd: HWND, rx: &Receiver<VCmd>, on_event: &impl Fn(VideoEvent)) -> windows::core::Result<()> {
        let timer = SetTimer(None, 0, TICK_MS, None);
        let pos_type = &MFP_POSITIONTYPE_100NS;
        let (mut ready, mut started, mut ended) = (false, false, false);
        let mut ticks = 0u32;
        let mut msg = MSG::default();
        'outer: while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.hwnd.is_invalid() && msg.message == WM_APP {
                while let Ok(c) = rx.try_recv() {
                    match c {
                        VCmd::Quit => break 'outer,
                        VCmd::Start if ready && !started => {
                            started = true;
                            // Поверх сцены, не отнимая фокус, — и сразу воспроизведение.
                            let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                            let _ = player.Play();
                        }
                        VCmd::Start => {}
                        VCmd::Toggle if started => {
                            if player.GetState().ok() == Some(MFP_MEDIAPLAYER_STATE_PLAYING) {
                                let _ = player.Pause();
                            } else {
                                let _ = player.Play();
                            }
                        }
                        VCmd::Toggle => {}
                        VCmd::Seek(d) | VCmd::SeekTo(d) if ready => {
                            let dur = secs(player.GetDuration(pos_type));
                            let base = if matches!(c, VCmd::Seek(_)) { secs(player.GetPosition(pos_type)) } else { 0.0 };
                            let to = (base + d).clamp(0.0, (dur - 0.5).max(0.0));
                            let _ = player.SetPosition(pos_type, &PROPVARIANT::from((to * 1e7) as i64));
                        }
                        VCmd::Seek(_) | VCmd::SeekTo(_) => {}
                    }
                }
                continue;
            }
            if msg.hwnd.is_invalid() && msg.message == WM_TIMER && msg.wParam.0 == timer {
                ticks += 1;
                let state = player.GetState().unwrap_or(MFP_MEDIAPLAYER_STATE_EMPTY);
                if !ready {
                    if state == MFP_MEDIAPLAYER_STATE_STOPPED {
                        // Файл открыт. Пауза выводит первый кадр и готовит воспроизведение.
                        let _ = player.Pause();
                        ready = true;
                        on_event(VideoEvent::Ready(secs(player.GetDuration(pos_type))));
                    } else if ticks > OPEN_TIMEOUT_TICKS {
                        on_event(VideoEvent::Error("не удалось открыть видео (формат не поддерживается?)".into()));
                        break;
                    }
                    continue;
                }
                if started {
                    // Сцену могли поднять наверх — держим видео поверх неё.
                    let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
                let (pos, dur) = (secs(player.GetPosition(pos_type)), secs(player.GetDuration(pos_type)));
                let playing = state == MFP_MEDIAPLAYER_STATE_PLAYING;
                on_event(VideoEvent::Pos { pos, dur, playing });
                // В конце файла MFPlay сам переходит в «остановлено».
                if started && !ended && state == MFP_MEDIAPLAYER_STATE_STOPPED {
                    ended = true;
                    on_event(VideoEvent::Ended);
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = KillTimer(None, timer);
        Ok(())
    }
}
