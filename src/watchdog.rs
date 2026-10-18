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
        crashes = if started.elapsed() < Duration::from_secs(30) { crashes + 1 } else { 1 };
        let pause = if crashes >= FAST_CRASHES { Duration::from_secs(60) } else { Duration::from_secs(2) };
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
        crate::registry::set(RUN_KEY, RUN_VALUE, &format!("\"{}\" --watchdog --server", exe.display()))
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
    /// Включает и выключает автозапуск (запись в реестре текущего пользователя):
    /// cargo test --release autostart_roundtrip -- --ignored --nocapture
    #[test]
    #[ignore]
    fn autostart_roundtrip() {
        let was = super::autostart_enabled();
        super::set_autostart(true).unwrap();
        assert!(super::autostart_enabled());
        println!("{}", crate::registry::get(super::RUN_KEY, super::RUN_VALUE).unwrap_or_default());
        super::set_autostart(false).unwrap();
        assert!(!super::autostart_enabled());
        if was {
            super::set_autostart(true).unwrap();
        }
    }
}
