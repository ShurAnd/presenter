//! Сторож сервера (`presentator --watchdog --server`): запускает сервер и следит за ним.
//! Упал или завис — перезапускает; оператор закрыл сам (код выхода 0) — сторож тоже завершается.
//! Плюс автозапуск при входе в Windows (раздел Run текущего пользователя).

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// Окно сервера «не отвечает» дольше этого — считаем, что он завис.
const HANG_LIMIT: Duration = Duration::from_secs(20);
/// Падений подряд, после которых делаем длинную паузу (чтобы не крутиться в цикле).
const FAST_CRASHES: u32 = 5;

/// Bounded backoff for consecutive failures of the supervised server process.
/// A server that has run for >=30s breaks the rapid-crash sequence.
/// Pure policy function so the real restart thresholds can be regression-tested
/// without spawning the application or modifying the user's autostart registry.
fn restart_backoff(previous_crashes: u32, ran_for: Duration) -> (u32, Duration) {
    let crashes = if ran_for < Duration::from_secs(30) {
        previous_crashes.saturating_add(1)
    } else {
        1
    };
    let pause = if crashes >= FAST_CRASHES { Duration::from_secs(60) } else { Duration::from_secs(2) };
    (crashes, pause)
}

/// Exact command stored under HKCU\...\Run. Keep the real registry write separate
/// from the pure formatting code, so tests never affect autostart settings.
fn autostart_command(exe: &std::path::Path) -> String {
    format!("\"{}\" --watchdog --server", exe.display())
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Presentator";

fn log_path() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("presenter").join("watchdog.log")
}

fn log(msg: &str) {
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {msg}", local_time(now));
    }
}

/// «ГГГГ-ММ-ДД ЧЧ:ММ:СС» по местному времени.
fn local_time(_unix: u64) -> String {
    #[cfg(windows)]
    unsafe {
        let t = windows::Win32::System::SystemInformation::GetLocalTime();
        return format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
    }
    #[cfg(not(windows))]
    format!("{_unix}")
}

fn spawn_child(exe: &PathBuf, args: &[String]) -> std::io::Result<Child> {
    Command::new(exe).args(args).spawn()
}

/// Главный цикл сторожа. `args` — ключи для сервера (без `--watchdog`).
pub fn run(args: Vec<String>) {
    let Ok(exe) = std::env::current_exe() else { return };
    log(&format!("сторож запущен: {} {}", exe.display(), args.join(" ")));
    let mut crashes = 0u32;
    loop {
        let started = Instant::now();
        let mut child = match spawn_child(&exe, &args) {
            Ok(c) => c,
            Err(e) => {
                log(&format!("не удалось запустить сервер: {e}"));
                std::thread::sleep(Duration::from_secs(10));
                continue;
            }
        };
        let pid = child.id();
        log(&format!("сервер запущен (pid {pid})"));
        let mut hung_since: Option<Instant> = None;
        let status = loop {
            if let Ok(Some(st)) = child.try_wait() {
                break Some(st);
            }
            // Зависание: окно сервера не отвечает на сообщения дольше HANG_LIMIT.
            if win::is_hung(pid) {
                let since = *hung_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= HANG_LIMIT {
                    log(&format!("сервер не отвечает {} с — перезапуск", since.elapsed().as_secs()));
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            } else {
                hung_since = None;
            }
            std::thread::sleep(Duration::from_secs(1));
        };
        match status {
            Some(st) if st.success() => {
                log("сервер закрыт оператором — сторож завершается");
                return;
            }
            Some(st) => log(&format!("сервер завершился с ошибкой ({st}) — перезапуск")),
            None => {}
        }
        // Падает сразу после запуска раз за разом — длинная пауза, чтобы не крутиться в цикле.
        let (next_crashes, pause) = restart_backoff(crashes, started.elapsed());
        crashes = next_crashes;
        if crashes >= FAST_CRASHES {
            log(&format!("{crashes} падений подряд — пауза {} с", pause.as_secs()));
        }
        std::thread::sleep(pause);
    }
}

// ---------------------------------------------------------------- автозапуск

/// Включён ли автозапуск сервера при входе в Windows.
pub fn autostart_enabled() -> bool {
    crate::registry::get(RUN_KEY, RUN_VALUE).is_some()
}

/// Включить / выключить автозапуск: сторож с сервером при входе текущего пользователя.
/// Пишем через Windows API, а не запуском `reg.exe` — так делают обычные программы.
pub fn set_autostart(on: bool) -> Result<(), String> {
    if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        crate::registry::set(RUN_KEY, RUN_VALUE, &autostart_command(&exe))
    } else {
        crate::registry::delete(RUN_KEY, RUN_VALUE)
    }
}

#[cfg(not(windows))]
mod win {
    pub fn is_hung(_: u32) -> bool {
        false
    }
}

#[cfg(windows)]
mod win {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_NULL};

    struct Search {
        pid: u32,
        window: Option<HWND>,
    }

    unsafe extern "system" fn cb(h: HWND, data: LPARAM) -> BOOL {
        let s = &mut *(data.0 as *mut Search);
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid == s.pid && IsWindowVisible(h).as_bool() {
            s.window = Some(h);
            return false.into(); // нашли — дальше не ищем
        }
        true.into()
    }

    /// Отвечает ли окно сервера: шлём пустое сообщение и ждём не дольше 2 с. Признак Windows
    /// «не отвечает» сюда не годится: программу, застывшую в ожидании ввода, он зависшей не считает.
    pub fn is_hung(pid: u32) -> bool {
        let mut s = Search { pid, window: None };
        unsafe {
            let _ = EnumWindows(Some(cb), LPARAM(&mut s as *mut _ as isize));
            let Some(h) = s.window else { return false };
            let mut result = 0usize;
            SendMessageTimeoutW(h, WM_NULL, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 2000, Some(&mut result)).0 == 0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage13_fast_crash_restart_uses_two_seconds_until_threshold() {
        for previous in 0..(FAST_CRASHES - 1) {
            let (next, wait) = restart_backoff(previous, Duration::from_secs(5));
            assert_eq!(next, previous + 1);
            assert_eq!(wait, Duration::from_secs(2));
        }
    }

    #[test]
    fn stage13_fifth_fast_crash_causes_long_pause() {
        assert_eq!(restart_backoff(4, Duration::from_secs(29)),
            (5, Duration::from_secs(60)));
        assert_eq!(restart_backoff(5, Duration::ZERO),
            (6, Duration::from_secs(60)));
    }

    #[test]
    fn stage13_long_running_server_breaks_crash_streak_at_boundary() {
        assert_eq!(restart_backoff(17, Duration::from_secs(30)),
            (1, Duration::from_secs(2)));
        assert_eq!(restart_backoff(17, Duration::from_secs(3600)),
            (1, Duration::from_secs(2)));
    }

    #[test]
    fn stage13_crash_counter_cannot_overflow_or_disable_throttling() {
        assert_eq!(restart_backoff(u32::MAX, Duration::ZERO),
            (u32::MAX, Duration::from_secs(60)));
    }

    #[test]
    fn stage13_autostart_command_quotes_executable_with_spaces_and_unicode() {
        let exe = std::path::Path::new(r"C:\Test Files\Презентатор\presentator.exe");
        assert_eq!(autostart_command(exe),
            r#""C:\Test Files\Презентатор\presentator.exe" --watchdog --server"#);
    }

    #[test]
    fn stage13_log_location_is_under_presenter_directory() {
        assert!(log_path().ends_with(std::path::Path::new("presenter").join("watchdog.log")));
    }

    #[cfg(windows)]
    #[test]
    fn stage13_unknown_process_cannot_be_mistaken_for_hung_server() {
        // Enumerates real windows but does not modify or message any known process.
        assert!(!win::is_hung(u32::MAX));
    }
    // This test is deliberately opt-in: it temporarily changes the current user's
    // real Run key. Preserve the exact original value, not just whether it existed.
    #[cfg(windows)]
    struct RestoreAutostart(Option<String>);

    #[cfg(windows)]
    impl Drop for RestoreAutostart {
        fn drop(&mut self) {
            let result = match &self.0 {
                Some(original) => crate::registry::set(super::RUN_KEY, super::RUN_VALUE, original),
                None => crate::registry::delete(super::RUN_KEY, super::RUN_VALUE),
            };
            if let Err(error) = result {
                eprintln!("WARNING: failed to restore Presentator autostart: {error}");
            }
        }
    }

    /// Integration check of the actual Run key. Run ONLY on a disposable test user:
    /// cargo test autostart_roundtrip -- --ignored --nocapture
    #[test]
    #[ignore = "modifies real HKCU Run key: run explicitly on a test account"]
    #[cfg(windows)]
    fn autostart_roundtrip() {
        let _restore = RestoreAutostart(crate::registry::get(super::RUN_KEY, super::RUN_VALUE));
        super::set_autostart(true).unwrap();
        let exe = std::env::current_exe().unwrap();
        let expected = format!("\"{}\" --watchdog --server", exe.display());
        assert_eq!(crate::registry::get(super::RUN_KEY, super::RUN_VALUE).as_deref(), Some(expected.as_str()));
        super::set_autostart(false).unwrap();
        assert_eq!(crate::registry::get(super::RUN_KEY, super::RUN_VALUE), None);
        // The guard restores the exact previous string even on a panic.
    }
}
