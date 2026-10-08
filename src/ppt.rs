//! Показ PPTX через установленный PowerPoint: запускается настоящее слайд-шоу,
//! которым мы управляем (вперёд, назад, переход к слайду) и за которым следим.
//! Мост к PowerPoint — скрытый процесс PowerShell, работающий с ним по COM.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub enum PptEvent {
    /// Показ запущен; число слайдов.
    Ready(usize),
    /// Текущий слайд (с единицы) и всего слайдов.
    Pos(usize, usize),
    /// Показ завершён (в том числе закрыт на самом сервере).
    Ended,
    Error(String),
}

const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
function Say($s) { [Console]::Out.WriteLine($s); [Console]::Out.Flush() }
$app = $null; $pres = $null; $win = $null; $total = 0
try {
    $app = New-Object -ComObject PowerPoint.Application
    $pres = $app.Presentations.Open($env:PRES_SRC, -1, 0, 0)
    $total = $pres.Slides.Count
    $ss = $pres.SlideShowSettings
    if ($env:PRES_WINDOWED -eq '1') { $ss.ShowType = 2 } else { $ss.ShowType = 1 }
    try { $ss.ShowPresenterView = 0 } catch {}
    $win = $ss.Run()
    # Настройки показа выше меняют презентацию. Помечаем её сохранённой: иначе при закрытии
    # (в том числе Esc в самом PowerPoint) он спросит о сохранении файла, открытого только для чтения.
    $pres.Saved = -1
    # Указатель мыши в показе всегда скрыт.
    try { $win.View.PointerType = 3 } catch {}
    Say ('ready ' + $total)
} catch {
    Say ('error ' + ($_.Exception.Message -replace '\s+', ' '))
}
if ($win) {
    while ($true) {
        $line = [Console]::In.ReadLine()
        if ($line -eq $null -or $line -eq 'close') { break }
        try {
            $view = $win.View
            $cur = $view.CurrentShowPosition
            if ($line -eq 'next') {
                # На последнем шаге последнего слайда не идём дальше, иначе показ закроется.
                $last = $false
                if ($cur -ge $total) {
                    try { $last = ($view.GetClickIndex() -ge $view.GetClickCount()) } catch { $last = $true }
                }
                if (-not $last) { $view.Next() }
            } elseif ($line -eq 'prev') {
                $view.Previous()
            } elseif ($line -like 'goto *') {
                $view.GotoSlide([int]$line.Substring(5))
            } elseif ($line -eq 'show') {
                # Сцена с фоном отпустила экран — выводим слайд-шоу на передний план.
                try { $win.Activate() } catch {}
            }
            # Опрос (poll) PowerPoint лишний раз не трогаем — он может как раз менять слайд.
            if ($line -ne 'poll') { try { $pres.Saved = -1 } catch {} }
            Say ('pos ' + $view.CurrentShowPosition + ' ' + $total)
        } catch {
            # PowerPoint мог быть просто занят; показ окончен, только если его окна больше нет.
            $alive = $false
            try { $alive = ($app.SlideShowWindows.Count -gt 0) } catch {}
            if (-not $alive) { Say 'ended'; $win = $null; break }
        }
    }
}
try { if ($win) { $win.View.Exit() } } catch {}
try { if ($pres) { $pres.Saved = -1; $pres.Close() } } catch {}
try { if ($app -and $app.Presentations.Count -eq 0) { $app.Quit() } } catch {}
"#;

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn AllowSetForegroundWindow(pid: u32) -> i32;
}

/// Перехват Esc в слайд-шоу PowerPoint. Сам PowerPoint закрывает показ по одиночному Esc,
/// а у нас показ закрывается только кнопкой или двойным Esc. Поэтому, пока идёт наш показ
/// (`grab_esc(true)`) и на переднем плане окно слайд-шоу, Esc до PowerPoint не доходит,
/// а каждое нажатие передаётся в приложение.
#[cfg(windows)]
mod esc_hook {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;

    type Callback = Box<dyn Fn() + Send + Sync>;
    static CALLBACK: OnceLock<Callback> = OnceLock::new();
    pub static GRAB: AtomicBool = AtomicBool::new(false);
    /// Esc зажат: автоповтор не должен считаться повторным нажатием.
    static DOWN: AtomicBool = AtomicBool::new(false);

    const WH_KEYBOARD_LL: i32 = 13;
    const VK_ESCAPE: u32 = 0x1B;
    const WM_KEYDOWN: usize = 0x100;
    const WM_SYSKEYDOWN: usize = 0x104;

    #[repr(C)]
    struct KbdLlHookStruct {
        vk_code: u32,
        scan_code: u32,
        flags: u32,
        time: u32,
        extra_info: usize,
    }

    #[repr(C)]
    struct Msg {
        hwnd: isize,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        pt: [i32; 2],
        reserved: [u32; 4],
    }

    type HookProc = unsafe extern "system" fn(i32, usize, isize) -> isize;

    #[link(name = "user32")]
    extern "system" {
        fn SetWindowsHookExW(id: i32, f: HookProc, hmod: isize, thread: u32) -> isize;
        fn CallNextHookEx(hook: isize, code: i32, w: usize, l: isize) -> isize;
        fn GetMessageW(msg: *mut Msg, hwnd: isize, min: u32, max: u32) -> i32;
        fn GetForegroundWindow() -> isize;
        fn GetClassNameW(hwnd: isize, buf: *mut u16, len: i32) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> isize;
    }

    /// Окно слайд-шоу PowerPoint имеет класс screenClass.
    unsafe fn slideshow_in_front() -> bool {
        let mut buf = [0u16; 32];
        let n = GetClassNameW(GetForegroundWindow(), buf.as_mut_ptr(), buf.len() as i32);
        n > 0 && String::from_utf16_lossy(&buf[..n as usize]) == "screenClass"
    }

    unsafe extern "system" fn hook(code: i32, w: usize, l: isize) -> isize {
        if code == 0 && GRAB.load(Ordering::SeqCst) {
            let k = &*(l as *const KbdLlHookStruct);
            if k.vk_code == VK_ESCAPE && slideshow_in_front() {
                if w == WM_KEYDOWN || w == WM_SYSKEYDOWN {
                    if !DOWN.swap(true, Ordering::SeqCst) {
                        if let Some(cb) = CALLBACK.get() {
                            cb();
                        }
                    }
                } else {
                    DOWN.store(false, Ordering::SeqCst);
                }
                return 1; // клавиша не доходит до PowerPoint
            }
        }
        CallNextHookEx(0, code, w, l)
    }

    pub fn install(cb: Callback) {
        if CALLBACK.set(cb).is_err() {
            return; // уже установлен
        }
        // Низкоуровневому хуку нужен поток с циклом сообщений.
        std::thread::spawn(|| unsafe {
            if SetWindowsHookExW(WH_KEYBOARD_LL, hook, GetModuleHandleW(std::ptr::null()), 0) == 0 {
                return;
            }
            let mut msg: Msg = std::mem::zeroed();
            while GetMessageW(&mut msg, 0, 0, 0) > 0 {}
        });
    }
}

/// Ставит перехват Esc (один раз за время работы программы); `on_esc` вызывается на каждое нажатие.
pub fn install_esc_hook(on_esc: impl Fn() + Send + Sync + 'static) {
    #[cfg(windows)]
    esc_hook::install(Box::new(on_esc));
    #[cfg(not(windows))]
    let _ = on_esc;
}

/// Включает и выключает перехват Esc: включён, пока идёт наш показ PowerPoint.
pub fn grab_esc(on: bool) {
    #[cfg(windows)]
    esc_hook::GRAB.store(on, std::sync::atomic::Ordering::SeqCst);
    #[cfg(not(windows))]
    let _ = on;
}

/// Пока жив этот объект — идёт показ. При уничтожении показ закрывается.
pub struct Ppt {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    done: Arc<AtomicBool>,
}

impl Ppt {
    pub fn start(src: &Path, windowed: bool, on_event: impl Fn(PptEvent) + Send + 'static) -> Result<Ppt, String> {
        // PRESENTATOR_PPT_CMD — подмена моста для отладки без PowerPoint.
        let mut cmd = match std::env::var_os("PRESENTATOR_PPT_CMD") {
            Some(c) => Command::new(c),
            None => {
                let mut c = Command::new("powershell");
                c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", SCRIPT]);
                c
            }
        };
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd
            .env("PRES_SRC", src)
            .env("PRES_WINDOWED", if windowed { "1" } else { "0" })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("не удалось запустить PowerShell: {e}"))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take()));
        let stdout = child.stdout.take().ok_or("нет вывода процесса")?;
        let done = Arc::new(AtomicBool::new(false));

        // Чтение событий.
        let done_r = done.clone();
        std::thread::spawn(move || {
            let mut finished = false;
            let mut started = false;
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let mut it = line.trim().splitn(2, ' ');
                let (head, rest) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
                match head {
                    "ready" => {
                        started = true;
                        on_event(PptEvent::Ready(rest.trim().parse().unwrap_or(0)));
                    }
                    "pos" => {
                        let mut n = rest.split_whitespace().filter_map(|x| x.parse::<usize>().ok());
                        if let (Some(cur), Some(total)) = (n.next(), n.next()) {
                            on_event(PptEvent::Pos(cur, total));
                        }
                    }
                    "ended" => {
                        finished = true;
                        on_event(PptEvent::Ended);
                    }
                    "error" => {
                        finished = true;
                        on_event(PptEvent::Error(rest.to_string()));
                    }
                    _ => {}
                }
            }
            let _ = child.wait();
            done_r.store(true, Ordering::SeqCst);
            if !finished {
                on_event(if started { PptEvent::Ended } else { PptEvent::Error("PowerPoint не запустился".into()) });
            }
        });

        // Регулярный опрос позиции: слайды могут листать и на самом сервере.
        let poll = stdin.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(400));
            let mut g = poll.lock().unwrap_or_else(|e| e.into_inner());
            let ok = g.as_mut().is_some_and(|s| s.write_all(b"poll\n").and_then(|_| s.flush()).is_ok());
            if !ok {
                break;
            }
        });

        Ok(Ppt { stdin, done })
    }

    /// Флаг «процесс моста завершился». Нужен, чтобы не запускать новый показ,
    /// пока прежний ещё закрывает PowerPoint (иначе его Quit() убьёт новый).
    pub fn done_flag(&self) -> Arc<AtomicBool> {
        self.done.clone()
    }

    /// Вывести слайд-шоу на передний план (после того как сцена опустилась под него).
    pub fn reveal(&self) {
        // Разрешаем процессу PowerShell забрать фокус: сами мы сейчас активное окно.
        #[cfg(windows)]
        unsafe {
            AllowSetForegroundWindow(u32::MAX); // ASFW_ANY
        }
        self.send("show");
    }

    pub fn send(&self, line: &str) {
        let mut g = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = g.as_mut() {
            let _ = s.write_all(format!("{line}\n").as_bytes()).and_then(|_| s.flush());
        }
    }
}

impl Drop for Ppt {
    fn drop(&mut self) {
        self.send("close");
        // Закрываем канал: скрипт завершит показ и закроет презентацию.
        *self.stdin.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}
