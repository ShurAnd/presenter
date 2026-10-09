//! Показ PPTX через установленный PowerPoint: запускается настоящее слайд-шоу,
//! которым мы управляем (вперёд, назад, переход к слайду) и за которым следим.
//! Мост к PowerPoint — скрытый постоянный процесс PowerShell, работающий с ним по COM.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub enum PptEvent {
    /// Показ запущен; число слайдов и дескриптор окна слайд-шоу (0 — неизвестен).
    Ready(usize, isize),
    /// Текущий слайд (с единицы) и всего слайдов.
    Pos(usize, usize),
    /// Показ завершён (в том числе закрыт на самом сервере).
    Ended,
    Error(String),
}

/// Постоянный мост: один процесс PowerShell и один PowerPoint на всё время работы программы.
/// Открыть показ = открыть файл в уже запущенном PowerPoint (~0,2 с вместо ~1,5 с).
/// Переключение — закрыть файл и открыть другой в том же PowerPoint: без перезапуска,
/// поэтому нет гонки, когда новый показ подключался к ещё закрывающемуся PowerPoint.
///
/// Команды (stdin): `open <sid> <в окне 0/1> <путь в base64 UTF-8>`, `next`, `prev`, `goto N`,
/// `poll`, `show`, `close`, `quit`. Ответы: `host`, `ready <sid> N hwnd`, `pos <sid> cur total`,
/// `ended <sid>`, `error <sid> текст`.
const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
function Say($s) { [Console]::Out.WriteLine($s); [Console]::Out.Flush() }
function Msg($e) { $e.Exception.Message -replace '\s+', ' ' }
$script:app = $null; $script:pres = $null; $script:win = $null; $script:total = 0; $script:sid = '0'

# Живой PowerPoint; если его закрыли или он упал — запускаем заново.
function App {
    if ($script:app) { try { $null = $script:app.Version; return $script:app } catch { $script:app = $null } }
    $script:app = New-Object -ComObject PowerPoint.Application
    return $script:app
}

function CloseShow {
    # Exit у показа файла, открытого без окна, закрывает и сам файл; Close — на всякий случай.
    if ($script:win) { try { $script:win.View.Exit() } catch {} }
    if ($script:pres) { try { $script:pres.Saved = -1; $script:pres.Close() } catch {} }
    $script:win = $null; $script:pres = $null
}

function OpenShow($path, $windowed) {
    $a = App
    $script:pres = $a.Presentations.Open($path, -1, 0, 0)
    $script:total = $script:pres.Slides.Count
    $ss = $script:pres.SlideShowSettings
    if ($windowed -eq '1') { $ss.ShowType = 2 } else { $ss.ShowType = 1 }
    try { $ss.ShowPresenterView = 0 } catch {}
    $script:win = $ss.Run()
    # Настройки показа выше меняют презентацию. Помечаем её сохранённой: иначе при закрытии
    # PowerPoint спросит о сохранении файла, открытого только для чтения.
    $script:pres.Saved = -1
    # Указатель мыши в показе всегда скрыт.
    try { $script:win.View.PointerType = 3 } catch {}
}

# PowerPoint запускаем сразу (скрыто) — к первому показу он уже готов.
try { $null = App } catch {}
Say 'host'

while ($true) {
    $line = [Console]::In.ReadLine()
    if ($line -eq $null -or $line -eq 'quit') { break }
    $parts = $line.Split(' ')
    $cmd = $parts[0]
    if ($cmd -eq 'open') {
        CloseShow
        $script:sid = $parts[1]
        try {
            $path = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($parts[3]))
            try { OpenShow $path $parts[2] } catch {
                # Одна повторная попытка с заново запущенным PowerPoint.
                CloseShow; $script:app = $null; OpenShow $path $parts[2]
            }
            $hwnd = 0; try { $hwnd = $script:win.HWND } catch {}
            Say ('ready ' + $script:sid + ' ' + $script:total + ' ' + $hwnd)
        } catch {
            Say ('error ' + $script:sid + ' ' + (Msg $_)); CloseShow
        }
        continue
    }
    if ($cmd -eq 'close') { CloseShow; continue }
    if (-not $script:win) { continue }
    try {
        $view = $script:win.View
        $cur = $view.CurrentShowPosition
        if ($cmd -eq 'next') {
            # На последнем шаге последнего слайда не идём дальше, иначе показ закроется.
            $last = $false
            if ($cur -ge $script:total) {
                try { $last = ($view.GetClickIndex() -ge $view.GetClickCount()) } catch { $last = $true }
            }
            if (-not $last) { $view.Next() }
        } elseif ($cmd -eq 'prev') {
            $view.Previous()
        } elseif ($cmd -eq 'goto') {
            $view.GotoSlide([int]$parts[1])
        } elseif ($cmd -eq 'show') {
            # Сцена с фоном отпустила экран — выводим слайд-шоу на передний план.
            try { $script:win.Activate() } catch {}
        }
        # Опрос (poll) PowerPoint лишний раз не трогаем — он может как раз менять слайд.
        if ($cmd -ne 'poll') { try { $script:pres.Saved = -1 } catch {} }
        Say ('pos ' + $script:sid + ' ' + $view.CurrentShowPosition + ' ' + $script:total)
    } catch {
        # PowerPoint мог быть просто занят; показ окончен, только если его окна больше нет.
        $alive = $false
        try { $alive = ($script:app.SlideShowWindows.Count -gt 0) } catch {}
        if (-not $alive) { Say ('ended ' + $script:sid); $script:win = $null; CloseShow }
    }
}
CloseShow
try { if ($script:app -and $script:app.Presentations.Count -eq 0) { $script:app.Quit() } } catch {}
"#;

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn AllowSetForegroundWindow(pid: u32) -> i32;
    fn FindWindowW(class: *const u16, title: *const u16) -> isize;
    fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
}

/// Заголовок окна сцены — по нему мост находит её, чтобы положить прямо под слайд-шоу.
pub const STAGE_TITLE: &str = "Presentator — сцена";

/// Сцена «поверх всех окон» или обычное окно — напрямую через Windows. Команда уровня окна
/// через egui/winit после первого показа PowerPoint повторно не срабатывала, и следующее
/// слайд-шоу появлялось поверх фона ещё во время загрузки.
pub fn stage_topmost(on: bool) {
    #[cfg(windows)]
    unsafe {
        let title: Vec<u16> = STAGE_TITLE.encode_utf16().chain(Some(0)).collect();
        let stage = FindWindowW(std::ptr::null(), title.as_ptr());
        if stage != 0 {
            // HWND_TOPMOST = -1, HWND_NOTOPMOST = -2; NOSIZE | NOMOVE | NOACTIVATE.
            SetWindowPos(stage, if on { -1 } else { -2 }, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0010);
        }
    }
    #[cfg(not(windows))]
    let _ = on;
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

type Callback = Arc<dyn Fn(u64, PptEvent) + Send + Sync>;

struct Proc {
    stdin: ChildStdin,
    child: Arc<Mutex<Child>>,
    /// Номер запуска процесса — чтобы события умершего моста не путать с новым.
    epoch: u64,
}

/// Мост к PowerPoint. Процесс запускается при первой надобности и живёт до выхода из программы;
/// если он упал или его пришлось убить — запускается заново.
pub struct Host {
    proc: Arc<Mutex<Option<Proc>>>,
    /// Номер текущего показа (0 — показа нет). События чужих показов отбрасываются в `App`.
    current: Arc<AtomicU64>,
    epoch: AtomicU64,
    on_event: Callback,
}

impl Host {
    pub fn new(on_event: impl Fn(u64, PptEvent) + Send + Sync + 'static) -> Host {
        let host = Host { proc: Arc::new(Mutex::new(None)), current: Arc::new(AtomicU64::new(0)), epoch: AtomicU64::new(0), on_event: Arc::new(on_event) };
        // Регулярный опрос позиции, пока идёт показ: слайды могут листать и на самом сервере.
        let (proc, current) = (Arc::downgrade(&host.proc), host.current.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(400));
            let Some(proc) = proc.upgrade() else { break };
            if current.load(Ordering::SeqCst) != 0 {
                write_line(&proc, "poll");
            }
        });
        host
    }

    /// Запустить мост (и с ним PowerPoint) заранее, чтобы первый показ открылся быстро.
    pub fn prewarm(&self) {
        let _ = self.ensure();
    }

    fn ensure(&self) -> Result<(), String> {
        let mut g = self.proc.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_some() {
            return Ok(());
        }
        // PRESENTATOR_PPT_CMD — подмена моста для отладки без PowerPoint (тот же протокол).
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
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("не удалось запустить PowerShell: {e}"))?;
        let stdin = child.stdin.take().ok_or("нет ввода процесса")?;
        let stdout = child.stdout.take().ok_or("нет вывода процесса")?;
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let child = Arc::new(Mutex::new(child));
        *g = Some(Proc { stdin, child: child.clone(), epoch });

        // Чтение ответов. Конец вывода — мост умер: показ (если был) считаем оборвавшимся.
        let (on_event, current, proc) = (self.on_event.clone(), self.current.clone(), Arc::downgrade(&self.proc));
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let mut it = line.split_whitespace();
                let (head, sid) = (it.next().unwrap_or(""), it.next().and_then(|x| x.parse::<u64>().ok()).unwrap_or(0));
                let rest: Vec<&str> = it.collect();
                let num = |i: usize| rest.get(i).and_then(|x| x.parse::<usize>().ok());
                match head {
                    "ready" => {
                        let hwnd = rest.get(1).and_then(|x| x.parse::<isize>().ok()).unwrap_or(0);
                        on_event(sid, PptEvent::Ready(num(0).unwrap_or(0), hwnd));
                    }
                    "pos" => {
                        if let (Some(cur), Some(total)) = (num(0), num(1)) {
                            on_event(sid, PptEvent::Pos(cur, total));
                        }
                    }
                    "ended" => on_event(sid, PptEvent::Ended),
                    "error" => on_event(sid, PptEvent::Error(rest.join(" "))),
                    _ => {}
                }
            }
            let _ = child.lock().unwrap_or_else(|e| e.into_inner()).wait();
            if let Some(proc) = proc.upgrade() {
                let mut g = proc.lock().unwrap_or_else(|e| e.into_inner());
                if g.as_ref().is_some_and(|p| p.epoch == epoch) {
                    *g = None;
                }
            }
            let sid = current.load(Ordering::SeqCst);
            if sid != 0 {
                on_event(sid, PptEvent::Error("мост к PowerPoint завершился".into()));
            }
        });
        Ok(())
    }

    /// Открыть презентацию и запустить показ. Прежний показ (если был) мост закроет сам.
    pub fn open(&self, sid: u64, src: &Path, windowed: bool) -> Result<(), String> {
        self.ensure()?;
        self.current.store(sid, Ordering::SeqCst);
        let path = base64(src.to_string_lossy().as_bytes());
        self.send(&format!("open {sid} {} {path}", if windowed { 1 } else { 0 }));
        Ok(())
    }

    /// Закрыть текущий показ; PowerPoint остаётся запущенным для следующего.
    pub fn close(&self) {
        if self.current.swap(0, Ordering::SeqCst) != 0 {
            self.send("close");
        }
    }

    /// PowerPoint завис: убиваем мост, следующий показ запустит его заново.
    pub fn restart(&self) {
        self.current.store(0, Ordering::SeqCst);
        if let Some(p) = self.proc.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = p.child.lock().unwrap_or_else(|e| e.into_inner()).kill();
        }
    }

    /// Вывести слайд-шоу на экран (сцена уже не «поверх всех»). Порядок окон ставим явно:
    /// слайд-шоу наверх, сцену — прямо под него. Иначе со второго показа слайд-шоу, уже получившее
    /// фокус, оставалось под сценой: опущенная сцена встаёт выше всех обычных окон,
    /// а «вывести вперёд» окно, которое и так в фокусе, ничего не меняет.
    pub fn reveal(&self, slideshow: isize) {
        #[cfg(windows)]
        unsafe {
            const SWP: u32 = 0x0001 | 0x0002 | 0x0010; // NOSIZE | NOMOVE | NOACTIVATE
            // Разрешаем PowerPoint забрать фокус (если сейчас он у нас) — для кликера.
            AllowSetForegroundWindow(u32::MAX); // ASFW_ANY
            // PowerPoint не всегда отдаёт дескриптор окна показа — тогда ищем его по классу.
            let class: Vec<u16> = "screenClass".encode_utf16().chain(Some(0)).collect();
            let slideshow = if slideshow != 0 { slideshow } else { FindWindowW(class.as_ptr(), std::ptr::null()) };
            if slideshow != 0 {
                SetWindowPos(slideshow, 0, 0, 0, 0, 0, SWP); // HWND_TOP
                let title: Vec<u16> = STAGE_TITLE.encode_utf16().chain(Some(0)).collect();
                let stage = FindWindowW(std::ptr::null(), title.as_ptr());
                if stage != 0 {
                    SetWindowPos(stage, slideshow, 0, 0, 0, 0, SWP);
                }
            }
        }
        #[cfg(not(windows))]
        let _ = slideshow;
        self.send("show");
    }

    pub fn send(&self, line: &str) {
        write_line(&self.proc, line);
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        // Мост закроет показ и PowerPoint (если в нём нет чужих файлов) уже после нашего выхода.
        self.send("quit");
        self.proc.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

fn write_line(proc: &Mutex<Option<Proc>>, line: &str) {
    let mut g = proc.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = g.as_mut() {
        let _ = p.stdin.write_all(format!("{line}\n").as_bytes()).and_then(|_| p.stdin.flush());
    }
}

/// Путь передаём в base64: так в одной строке протокола не мешают пробелы, кириллица и кавычки.
fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64("тест".as_bytes()), "0YLQtdGB0YI=");
    }
}
