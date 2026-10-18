//! Показ PPTX через установленный PowerPoint: запускается настоящее слайд-шоу,
//! которым мы управляем (вперёд, назад, переход к слайду) и за которым следим.
//! Мост к PowerPoint — свой поток программы, работающий с ним по COM (`IDispatch`) напрямую.
//! До 18.10.2026 мостом был скрытый процесс PowerShell со скриптом — его отвергли: скрытый
//! `powershell -ExecutionPolicy Bypass` антивирусы считают подозрительным.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

pub enum PptEvent {
    /// Показ запущен; число слайдов и дескриптор окна слайд-шоу (0 — неизвестен).
    Ready(usize, isize),
    /// Текущий слайд (с единицы) и всего слайдов.
    Pos(usize, usize),
    /// Показ завершён (в том числе закрыт на самом сервере).
    Ended,
    /// Слайд (с единицы) сохранён картинкой для превью.
    Slide(usize),
    /// Слайд (с единицы) сохранён крупной картинкой для увеличения (`Host::export_zoom`).
    Zoom(usize),
    ZoomFailed(String),
    /// Видео и звук на текущем слайде (номер слайда с единицы) — их можно запускать без мыши.
    Media(usize, Vec<Media>),
    Error(String),
}

/// Видео или звук, вставленные в слайд. В слайд-шоу они запускаются щелчком мыши, а оператор
/// за другим компьютером (и курсор на экране показа обычно скрыт) — поэтому запускаем их командой моста.
#[derive(Clone, Debug, PartialEq)]
pub struct Media {
    /// Номер фигуры на слайде (`Shape.Id`) — по нему мост находит проигрыватель.
    pub id: i32,
    pub name: String,
    /// Видео (иначе звук).
    pub video: bool,
    /// Состояние проигрывателя PowerPoint: 0 — играет, 1 — пауза, 2 — остановлен, 3 — «не готов»
    /// (так до первого запуска; `Play` при этом работает — проверено 16.10.2026).
    pub state: u8,
    /// Позиция и длительность, с.
    pub pos: u32,
    pub dur: u32,
    /// Где на слайде: лево, верх, право, низ в долях слайда — для рамок на миниатюре.
    pub rect: [f32; 4],
}

/// Запросы к потоку моста.
enum Req {
    Open { sid: u64, path: PathBuf, windowed: bool, monitor: String, slides: Option<PathBuf>, width: u32 },
    Next,
    Prev,
    /// Перейти к слайду (с единицы).
    Goto(usize),
    /// Вывести слайд-шоу на передний план.
    Show,
    /// Сохранить слайд крупной картинкой: слайд, ширина, файл.
    Zoom(usize, u32, PathBuf),
    /// Указатель мыши в слайд-шоу: 3 — всегда скрыт, 4 — стрелка при движении.
    Cursor(i32),
    /// Видео на слайде: `Play` / `Pause` / `Stop` и номер фигуры.
    Media(String, i32),
    Close,
    Quit,
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn AllowSetForegroundWindow(pid: u32) -> i32;
    fn FindWindowW(class: *const u16, title: *const u16) -> isize;
    fn GetWindowLongW(hwnd: isize, index: i32) -> i32;
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
        // Уже в нужном состоянии — ничего не делаем (вызывается на каждой перерисовке сцены).
        let topmost = stage != 0 && GetWindowLongW(stage, -20) & 0x8 != 0; // GWL_EXSTYLE, WS_EX_TOPMOST
        if stage != 0 && topmost != on {
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
///
/// Сделано «горячей клавишей» Windows (`RegisterHotKey`): она занята только пока слайд-шоу на
/// переднем плане (проверка раз в 50 мс), в остальное время Esc работает как обычно. До 18.10.2026
/// здесь был низкоуровневый перехват всей клавиатуры (`WH_KEYBOARD_LL`) — так делают и кейлоггеры,
/// антивирусы такое не любят.
#[cfg(windows)]
mod esc_hook {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;

    type Callback = Box<dyn Fn() + Send + Sync>;
    static CALLBACK: OnceLock<Callback> = OnceLock::new();
    pub static GRAB: AtomicBool = AtomicBool::new(false);

    const VK_ESCAPE: u32 = 0x1B;
    const MOD_NOREPEAT: u32 = 0x4000; // автоповтор зажатой клавиши — не новое нажатие
    const WM_HOTKEY: u32 = 0x0312;
    const PM_REMOVE: u32 = 1;
    const QS_ALLINPUT: u32 = 0x04FF;
    const HOTKEY_ID: i32 = 1;

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

    #[link(name = "user32")]
    extern "system" {
        fn RegisterHotKey(hwnd: isize, id: i32, mods: u32, vk: u32) -> i32;
        fn UnregisterHotKey(hwnd: isize, id: i32) -> i32;
        fn PeekMessageW(msg: *mut Msg, hwnd: isize, min: u32, max: u32, remove: u32) -> i32;
        fn MsgWaitForMultipleObjects(count: u32, handles: *const isize, wait_all: i32, ms: u32, mask: u32) -> u32;
        fn GetForegroundWindow() -> isize;
        fn GetClassNameW(hwnd: isize, buf: *mut u16, len: i32) -> i32;
    }

    /// Окно слайд-шоу PowerPoint имеет класс screenClass.
    unsafe fn slideshow_in_front() -> bool {
        let mut buf = [0u16; 32];
        let n = GetClassNameW(GetForegroundWindow(), buf.as_mut_ptr(), buf.len() as i32);
        n > 0 && String::from_utf16_lossy(&buf[..n as usize]) == "screenClass"
    }

    pub fn install(cb: Callback) {
        if CALLBACK.set(cb).is_err() {
            return; // уже установлен
        }
        // Горячая клавиша приходит сообщением в очередь потока, который её занял.
        std::thread::spawn(|| unsafe {
            let mut registered = false;
            let mut msg: Msg = std::mem::zeroed();
            loop {
                while PeekMessageW(&mut msg, 0, 0, 0, PM_REMOVE) != 0 {
                    if msg.message == WM_HOTKEY && msg.wparam == HOTKEY_ID as usize {
                        if let Some(cb) = CALLBACK.get() {
                            cb();
                        }
                    }
                }
                let grab = GRAB.load(Ordering::SeqCst);
                let want = grab && slideshow_in_front();
                if want && !registered {
                    // Не удалось (Esc занят другой программой) — попробуем на следующем круге.
                    registered = RegisterHotKey(0, HOTKEY_ID, MOD_NOREPEAT, VK_ESCAPE) != 0;
                } else if !want && registered {
                    UnregisterHotKey(0, HOTKEY_ID);
                    registered = false;
                }
                // Ждём сообщения или следующей проверки: пока показа нет — реже.
                MsgWaitForMultipleObjects(0, std::ptr::null(), 0, if grab { 50 } else { 250 }, QS_ALLINPUT);
            }
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

/// Общее у `Host` и потока моста.
struct Ctx {
    on_event: Callback,
    /// С какого момента (мс от старта программы) ждём ответа PowerPoint; 0 — не ждём.
    waiting: Arc<AtomicU64>,
    /// Процесс PowerPoint — чтобы завершить его, если он завис.
    pid: Arc<AtomicU32>,
}

struct Worker {
    tx: Sender<Req>,
    /// Номер запуска потока — чтобы конец старого потока не путать с новым.
    epoch: u64,
    /// Закрывается, когда поток моста завершился (закрыл показ и, если можно, PowerPoint).
    done: Receiver<()>,
}

/// Мост к PowerPoint: один поток и один PowerPoint на всё время работы программы.
/// Открыть показ = открыть файл в уже запущенном PowerPoint (~0,2 с вместо ~1,5 с).
/// Переключение — закрыть файл и открыть другой в том же PowerPoint, без перезапуска.
///
/// Поток выполняет запросы по очереди, раз в 400 мс (пока идёт показ) спрашивает у PowerPoint
/// позицию и состояние видео на слайде, а в паузах сохраняет слайды картинками для превью — по
/// одному, поэтому запрос ждёт не дольше одного слайда. Отдельный поток или процесс для превью не
/// годится: PowerPoint отказывает второму клиенту («занят»), пока первый с ним работает.
pub struct Host {
    worker: Arc<Mutex<Option<Worker>>>,
    waiting: Arc<AtomicU64>,
    /// Номер текущего показа (0 — показа нет). События чужих показов отбрасываются в `App`.
    current: Arc<AtomicU64>,
    epoch: AtomicU64,
    on_event: Callback,
    /// Курсор в слайд-шоу виден при движении («Показать курсор»).
    cursor: AtomicBool,
    pid: Arc<AtomicU32>,
}

impl Host {
    pub fn new(on_event: impl Fn(u64, PptEvent) + Send + Sync + 'static) -> Host {
        Host {
            worker: Arc::new(Mutex::new(None)),
            waiting: Arc::new(AtomicU64::new(0)),
            current: Arc::new(AtomicU64::new(0)),
            epoch: AtomicU64::new(0),
            on_event: Arc::new(on_event),
            cursor: AtomicBool::new(false),
            pid: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Запустить мост (и с ним PowerPoint) заранее, чтобы первый показ открылся быстро.
    pub fn prewarm(&self) {
        let _ = self.ensure();
    }

    fn ensure(&self) -> Result<(), String> {
        let mut g = self.worker.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_some() {
            return Ok(());
        }
        let (tx, rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel::<()>();
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let ctx = Ctx { on_event: self.on_event.clone(), waiting: self.waiting.clone(), pid: self.pid.clone() };
        let guard = EndGuard { _done: done_tx, epoch, worker: Arc::downgrade(&self.worker), current: self.current.clone(), on_event: self.on_event.clone() };
        std::thread::Builder::new()
            .name("powerpoint".into())
            .spawn(move || {
                let _guard = guard;
                bridge::run(rx, ctx);
            })
            .map_err(|e| format!("не удалось запустить мост к PowerPoint: {e}"))?;
        *g = Some(Worker { tx, epoch, done });
        Ok(())
    }

    fn post(&self, req: Req) {
        if let Some(w) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = w.tx.send(req);
        }
    }

    /// Открыть презентацию и запустить показ. Прежний показ (если был) мост закроет сам.
    /// `monitor` — системное имя монитора для показа (пусто — как настроено в PowerPoint).
    /// `slides` — папка, куда во время показа сохранить слайды картинками шириной `width` (превью).
    pub fn open(&self, sid: u64, src: &Path, windowed: bool, monitor: &str, slides: Option<&Path>, width: u32) -> Result<(), String> {
        self.ensure()?;
        self.current.store(sid, Ordering::SeqCst);
        // Мост мог перезапуститься — режим указателя напоминаем перед каждым показом.
        self.post(Req::Cursor(if self.cursor.load(Ordering::SeqCst) { 4 } else { 3 }));
        self.post(Req::Open { sid, path: src.to_path_buf(), windowed, monitor: monitor.to_string(), slides: slides.map(Path::to_path_buf), width });
        Ok(())
    }

    pub fn next(&self) {
        self.post(Req::Next);
    }

    pub fn prev(&self) {
        self.post(Req::Prev);
    }

    /// Перейти к слайду `n` (с единицы).
    pub fn goto(&self, n: usize) {
        self.post(Req::Goto(n));
    }

    /// Курсор в слайд-шоу: `true` — стрелка при движении мыши (PowerPoint прячет её сам),
    /// `false` — всегда скрыт. Действует сразу и на следующие показы.
    pub fn set_cursor(&self, on: bool) {
        self.cursor.store(on, Ordering::SeqCst);
        self.post(Req::Cursor(if on { 4 } else { 3 }));
    }

    /// Видео на слайде: `action` — `play`, `pause` или `stop`; `id` — номер фигуры (`Media::id`).
    pub fn media(&self, action: &str, id: i32) {
        self.post(Req::Media(action.to_string(), id));
    }

    /// Сохранить слайд `slide` (с единицы) картинкой шириной `width` в файл `path` — для увеличения.
    /// Готово — событие `Zoom(slide)`.
    pub fn export_zoom(&self, slide: usize, width: u32, path: &Path) {
        self.post(Req::Zoom(slide, width, path.to_path_buf()));
    }

    /// Для показателей сервера: сколько мс PowerPoint не отвечает на запросы моста
    /// (None — показа PPTX сейчас нет). Функцию можно вызывать из другого потока.
    pub fn lag_probe(&self) -> impl Fn() -> Option<u64> + Send + 'static {
        let (current, waiting) = (self.current.clone(), self.waiting.clone());
        move || {
            if current.load(Ordering::SeqCst) == 0 {
                return None;
            }
            let w = waiting.load(Ordering::SeqCst);
            Some(if w == 0 { 0 } else { now_ms().saturating_sub(w) })
        }
    }

    /// Закрыть текущий показ; PowerPoint остаётся запущенным для следующего.
    pub fn close(&self) {
        if self.current.swap(0, Ordering::SeqCst) != 0 {
            self.post(Req::Close);
        }
    }

    /// PowerPoint завис: бросаем поток моста и завершаем процесс PowerPoint (иначе и новый мост
    /// подключился бы к тому же зависшему PowerPoint). Следующий показ запустит всё заново.
    pub fn restart(&self) {
        self.current.store(0, Ordering::SeqCst);
        self.waiting.store(0, Ordering::SeqCst);
        // Без канала поток моста завершится сам, как только PowerPoint ответит или умрёт.
        self.worker.lock().unwrap_or_else(|e| e.into_inner()).take();
        let pid = self.pid.swap(0, Ordering::SeqCst);
        if pid != 0 {
            crate::jlog!("PowerPoint не отвечает — процесс {pid} завершён");
            kill(pid);
        }
    }

    /// Вывести слайд-шоу на экран (сцена уже не «поверх всех»). Порядок окон ставим явно:
    /// слайд-шоу наверх, сцену — прямо под него. Иначе со второго показа слайд-шоу, уже получившее
    /// фокус, оставалось под сценой: опущенная сцена встаёт выше всех обычных окон,
    /// а «вывести вперёд» окно, которое и так в фокусе, ничего не меняет.
    /// `target` — монитор для показа: если PowerPoint открыл слайд-шоу не там, переставляем окно.
    pub fn reveal(&self, slideshow: isize, target: Option<(i32, i32, i32, i32)>) {
        #[cfg(windows)]
        unsafe {
            const SWP: u32 = 0x0001 | 0x0002 | 0x0010; // NOSIZE | NOMOVE | NOACTIVATE
            // Разрешаем PowerPoint забрать фокус (если сейчас он у нас) — для кликера.
            AllowSetForegroundWindow(u32::MAX); // ASFW_ANY
            // PowerPoint не всегда отдаёт дескриптор окна показа — тогда ищем его по классу.
            let class: Vec<u16> = "screenClass".encode_utf16().chain(Some(0)).collect();
            let slideshow = if slideshow != 0 { slideshow } else { FindWindowW(class.as_ptr(), std::ptr::null()) };
            if slideshow != 0 {
                if let Some(r) = target {
                    crate::display::fit_hwnd(slideshow, r);
                }
                SetWindowPos(slideshow, 0, 0, 0, 0, 0, SWP); // HWND_TOP
                let title: Vec<u16> = STAGE_TITLE.encode_utf16().chain(Some(0)).collect();
                let stage = FindWindowW(std::ptr::null(), title.as_ptr());
                if stage != 0 {
                    SetWindowPos(stage, slideshow, 0, 0, 0, 0, SWP);
                }
            }
        }
        #[cfg(not(windows))]
        let _ = (slideshow, target);
        self.post(Req::Show);
    }

    /// Выход из программы: мост закрывает показ и PowerPoint (если в нём нет чужих файлов).
    /// Ждём его не дольше 4 с — иначе процесс завершится раньше, и скрытый PowerPoint
    /// останется висеть с открытым файлом.
    pub fn shutdown(&self) {
        let w = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(w) = w {
            let _ = w.tx.send(Req::Quit);
            let _ = w.done.recv_timeout(Duration::from_secs(4));
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Конец потока моста (в том числе при панике): если поток не убирали сами (`restart`,
/// `shutdown`), а показ шёл — сообщаем об обрыве.
struct EndGuard {
    _done: Sender<()>,
    epoch: u64,
    worker: Weak<Mutex<Option<Worker>>>,
    current: Arc<AtomicU64>,
    on_event: Callback,
}

impl Drop for EndGuard {
    fn drop(&mut self) {
        let Some(worker) = self.worker.upgrade() else { return };
        let mine = {
            let mut g = worker.lock().unwrap_or_else(|e| e.into_inner());
            let mine = g.as_ref().is_some_and(|w| w.epoch == self.epoch);
            if mine {
                *g = None;
            }
            mine
        };
        let sid = self.current.load(Ordering::SeqCst);
        if mine && sid != 0 {
            (self.on_event)(sid, PptEvent::Error("мост к PowerPoint завершился".into()));
        }
    }
}

/// Завершить зависший процесс PowerPoint.
fn kill(pid: u32) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
        if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
            let _ = TerminateProcess(h, 1);
            let _ = CloseHandle(h);
        }
    }
    #[cfg(not(windows))]
    let _ = pid;
}

/// Миллисекунды от первого вызова (монотонно; 0 зарезервирован под «не ждём»).
fn now_ms() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64 + 1
}

/// Вызовы объектов COM по имени (позднее связывание через `IDispatch`) — как в VBA и PowerShell:
/// у PowerPoint нет удобной библиотеки типов для Rust, а имена свойств и методов — документированные.
#[cfg(windows)]
mod com {
    use std::mem::ManuallyDrop;
    use std::time::{Duration, Instant};
    use windows::core::{Error, Interface, BSTR, GUID, HRESULT, IUnknown, PCWSTR, VARIANT};
    use windows::Win32::System::Com::{CLSIDFromProgID, CoCreateInstance, IDispatch, CLSCTX_LOCAL_SERVER, DISPATCH_FLAGS, DISPATCH_METHOD, DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, DISPPARAMS, EXCEPINFO};

    pub type R<T> = windows::core::Result<T>;

    const RPC_E_CALL_REJECTED: HRESULT = HRESULT(0x8001_0001u32 as i32);
    const RPC_E_SERVERCALL_RETRYLATER: HRESULT = HRESULT(0x8001_010Au32 as i32);
    const DISP_E_EXCEPTION: HRESULT = HRESULT(0x8002_0009u32 as i32);
    const E_POINTER: HRESULT = HRESULT(0x8000_4003u32 as i32);
    const E_FAIL: HRESULT = HRESULT(0x8000_4005u32 as i32);
    const DISPID_PROPERTYPUT: i32 = -3;
    const LCID: u32 = 0x0400; // LOCALE_USER_DEFAULT
    const VT_DISPATCH: u16 = 9;
    const VT_UNKNOWN: u16 = 13;

    /// PowerPoint занят (открывает файл, меняет слайд) и просит повторить позже.
    pub fn is_busy(e: &Error) -> bool {
        e.code() == RPC_E_CALL_REJECTED || e.code() == RPC_E_SERVERCALL_RETRYLATER
    }

    /// Ошибка файловой операции — в тот же тип, что ошибки COM.
    pub fn io(e: std::io::Error) -> Error {
        Error::new(E_FAIL, e.to_string())
    }

    /// Повторяем вызов, пока PowerPoint «занят», но не дольше 5 с.
    fn retry<T>(mut f: impl FnMut() -> R<T>) -> R<T> {
        let start = Instant::now();
        loop {
            match f() {
                Err(e) if is_busy(&e) && start.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(30)),
                r => return r,
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn i(n: i32) -> VARIANT {
        VARIANT::from(n)
    }

    pub fn s(t: &str) -> VARIANT {
        VARIANT::from(t)
    }

    #[derive(Clone)]
    pub struct Disp(IDispatch);

    impl Disp {
        /// Запустить сервер COM (или подключиться к уже запущенному — PowerPoint один на всех).
        pub fn create(progid: &str) -> R<Disp> {
            let w = wide(progid);
            unsafe {
                let clsid = CLSIDFromProgID(PCWSTR(w.as_ptr()))?;
                Ok(Disp(CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER)?))
            }
        }

        fn invoke(&self, name: &str, flags: DISPATCH_FLAGS, args: &[VARIANT]) -> R<VARIANT> {
            let w = wide(name);
            let names = [PCWSTR(w.as_ptr())];
            let mut id = 0i32;
            retry(|| unsafe { self.0.GetIDsOfNames(&GUID::zeroed(), names.as_ptr(), 1, LCID, &mut id) }).map_err(|e| Error::new(e.code(), format!("{name}: {}", e.message())))?;
            // Аргументы в DISPPARAMS — в обратном порядке; у записи свойства — именованный аргумент.
            let mut rev: Vec<VARIANT> = args.iter().rev().cloned().collect();
            let mut named = DISPID_PROPERTYPUT;
            let put = flags == DISPATCH_PROPERTYPUT;
            let params = DISPPARAMS {
                rgvarg: if rev.is_empty() { std::ptr::null_mut() } else { rev.as_mut_ptr() },
                rgdispidNamedArgs: if put { &mut named } else { std::ptr::null_mut() },
                cArgs: rev.len() as u32,
                cNamedArgs: put as u32,
            };
            retry(|| unsafe {
                let mut res = VARIANT::new();
                let mut exc = EXCEPINFO::default();
                let r = self.0.Invoke(id, &GUID::zeroed(), LCID, flags, &params, Some(&mut res), Some(&mut exc), None);
                match r {
                    Ok(()) => Ok(res),
                    Err(e) => {
                        let mut code = e.code();
                        let mut text = e.message();
                        if code == DISP_E_EXCEPTION {
                            // Ошибка самого PowerPoint — её текст в EXCEPINFO.
                            if let Some(fill) = exc.pfnDeferredFillIn {
                                let _ = fill(&mut exc);
                            }
                            if exc.scode != 0 {
                                code = HRESULT(exc.scode);
                            }
                            let d = exc.bstrDescription.to_string();
                            if !d.is_empty() {
                                text = d;
                            }
                        }
                        ManuallyDrop::drop(&mut exc.bstrSource);
                        ManuallyDrop::drop(&mut exc.bstrDescription);
                        ManuallyDrop::drop(&mut exc.bstrHelpFile);
                        Err(Error::new(code, format!("{name}: {}", text.split_whitespace().collect::<Vec<_>>().join(" "))))
                    }
                }
            })
        }

        /// Свойство или метод без аргументов.
        pub fn get(&self, name: &str) -> R<VARIANT> {
            self.call(name, &[])
        }

        /// Метод (или свойство с аргументами, например `Item(3)`).
        pub fn call(&self, name: &str, args: &[VARIANT]) -> R<VARIANT> {
            self.invoke(name, DISPATCH_FLAGS(DISPATCH_METHOD.0 | DISPATCH_PROPERTYGET.0), args)
        }

        pub fn put(&self, name: &str, v: VARIANT) -> R<()> {
            self.invoke(name, DISPATCH_PROPERTYPUT, &[v]).map(|_| ())
        }

        pub fn obj(&self, name: &str) -> R<Disp> {
            to_disp(&self.get(name)?)
        }

        pub fn obj_at(&self, name: &str, args: &[VARIANT]) -> R<Disp> {
            to_disp(&self.call(name, args)?)
        }

        pub fn int(&self, name: &str) -> R<i64> {
            i64::try_from(&self.get(name)?)
        }

        pub fn num(&self, name: &str) -> R<f64> {
            f64::try_from(&self.get(name)?)
        }

        pub fn text(&self, name: &str) -> R<String> {
            Ok(BSTR::try_from(&self.get(name)?)?.to_string())
        }
    }

    /// Объект из результата; пустой (`Nothing`) — ошибка.
    pub fn to_disp(v: &VARIANT) -> R<Disp> {
        unsafe {
            let raw = v.as_raw();
            let vt = raw.Anonymous.Anonymous.vt;
            let p = raw.Anonymous.Anonymous.Anonymous.pdispVal;
            if (vt == VT_DISPATCH || vt == VT_UNKNOWN) && !p.is_null() {
                if let Some(unk) = IUnknown::from_raw_borrowed(&p) {
                    return Ok(Disp(unk.cast::<IDispatch>()?));
                }
            }
        }
        Err(Error::new(E_POINTER, "пустой объект"))
    }
}

/// Поток моста: владеет объектами PowerPoint, выполняет запросы, опрашивает показ.
#[cfg(windows)]
mod bridge {
    use super::com::{self, i, s, Disp, R};
    use super::{now_ms, Ctx, Media, PptEvent, Req};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::time::{Duration, Instant};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    /// Как часто спрашивать у PowerPoint позицию и состояние видео, пока идёт показ.
    const POLL: Duration = Duration::from_millis(400);

    /// Видео или звук на текущем слайде; проигрыватель берём один раз на слайд.
    struct MShape {
        id: i32,
        name: String,
        video: bool,
        len: u32,
        left: f64,
        top: f64,
        w: f64,
        h: f64,
        player: Option<Disp>,
    }

    /// Сохранение слайдов картинками для превью: папка, размер и какие слайды ещё не сохранены.
    struct Export {
        dir: PathBuf,
        w: i32,
        h: i32,
        todo: Vec<usize>,
    }

    struct Bridge<'a> {
        ctx: &'a Ctx,
        sid: u64,
        app: Option<Disp>,
        pres: Option<Disp>,
        win: Option<Disp>,
        total: usize,
        /// Размер слайда в пунктах.
        size: (f64, f64),
        /// Указатель мыши в показе: 3 — всегда скрыт, 4 — стрелка при движении.
        cursor: i32,
        exp: Option<Export>,
        /// Видео и звук: для какого слайда собран список, сам список, что последним сообщили.
        mslide: i64,
        mlist: Vec<MShape>,
        mlast: Option<(usize, Vec<Media>)>,
        /// Папка слайдов презентации — туда же перечень фигур слайда (для разбора, если видео не нашлось).
        sdir: Option<PathBuf>,
        last_pos: Option<(usize, usize)>,
    }

    pub fn run(rx: Receiver<Req>, ctx: Ctx) {
        // Многопоточный «апартамент»: этому потоку не нужен цикл сообщений Windows.
        let inited = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        {
            let mut b = Bridge {
                ctx: &ctx,
                sid: 0,
                app: None,
                pres: None,
                win: None,
                total: 0,
                size: (4.0, 3.0),
                cursor: 3,
                exp: None,
                mslide: -1,
                mlist: Vec::new(),
                mlast: None,
                sdir: None,
                last_pos: None,
            };
            // PowerPoint запускаем сразу (скрыто) — к первому показу он уже готов.
            b.busy(|b| {
                let _ = b.app();
            });
            let mut next_poll = Instant::now();
            loop {
                // Ждём запрос; пока идёт показ — не дольше, чем до следующего опроса; пока есть
                // несохранённые слайды — не ждём совсем.
                let wait = if b.exp.is_some() {
                    Some(Duration::ZERO)
                } else if b.win.is_some() {
                    Some(next_poll.saturating_duration_since(Instant::now()))
                } else {
                    None
                };
                let got = match wait {
                    None => rx.recv().map_err(|_| true),
                    Some(d) => rx.recv_timeout(d).map_err(|e| e == RecvTimeoutError::Disconnected),
                };
                match got {
                    Ok(Req::Quit) | Err(true) => break,
                    Ok(req) => b.busy(|b| b.handle(req)),
                    Err(false) => {
                        if b.win.is_some() && Instant::now() >= next_poll {
                            b.busy(|b| b.command(None));
                            next_poll = Instant::now() + POLL;
                        } else if b.exp.is_some() {
                            b.busy(Bridge::slide_step);
                        }
                    }
                }
            }
            b.busy(|b| {
                b.close_show();
                // PowerPoint закрываем, только если в нём не открыто ничего чужого.
                if let Some(a) = &b.app {
                    if a.obj("Presentations").and_then(|p| p.int("Count")).is_ok_and(|n| n == 0) {
                        let _ = a.call("Quit", &[]);
                    }
                }
            });
        }
        if inited {
            unsafe { CoUninitialize() };
        }
    }

    impl Bridge<'_> {
        fn emit(&self, ev: PptEvent) {
            (self.ctx.on_event)(self.sid, ev);
        }

        /// Пока PowerPoint выполняет запрос — для показателя «PowerPoint не отвечает».
        fn busy(&mut self, f: impl FnOnce(&mut Self)) {
            self.ctx.waiting.store(now_ms(), Ordering::SeqCst);
            f(self);
            self.ctx.waiting.store(0, Ordering::SeqCst);
        }

        /// Живой PowerPoint; если его закрыли или он упал — запускаем заново.
        fn app(&mut self) -> R<Disp> {
            if let Some(a) = &self.app {
                if a.get("Version").is_ok() {
                    return Ok(a.clone());
                }
                self.app = None;
            }
            let a = Disp::create("PowerPoint.Application")?;
            // Номер процесса — чтобы завершить PowerPoint, если он зависнет. PowerPoint один на
            // пользователя, поэтому ищем его по имени: Application.HWND через IDispatch недоступен.
            let pid = crate::health::powerpoint_pid().unwrap_or(0);
            crate::jlog!("PowerPoint запущен (процесс {pid})");
            self.ctx.pid.store(pid, Ordering::SeqCst);
            self.app = Some(a.clone());
            Ok(a)
        }

        fn handle(&mut self, req: Req) {
            match req {
                Req::Open { sid, path, windowed, monitor, slides, width } => {
                    self.close_show();
                    self.sid = sid;
                    self.last_pos = None;
                    if let Err(e) = self.open(&path, windowed, &monitor, slides, width) {
                        self.emit(PptEvent::Error(e.message()));
                        self.close_show();
                    }
                }
                Req::Close => self.close_show(),
                Req::Cursor(c) => {
                    self.cursor = c;
                    if let Some(v) = self.win.as_ref().and_then(|w| w.obj("View").ok()) {
                        let _ = v.put("PointerType", i(c));
                    }
                }
                Req::Zoom(n, width, file) => match self.zoom(n, width, &file) {
                    Ok(()) => self.emit(PptEvent::Zoom(n)),
                    Err(e) => self.emit(PptEvent::ZoomFailed(e.message())),
                },
                Req::Quit => {}
                other => self.command(Some(other)),
            }
        }

        fn open(&mut self, path: &Path, windowed: bool, monitor: &str, slides: Option<PathBuf>, width: u32) -> R<()> {
            if self.open_show(path, windowed, monitor).is_err() {
                // Одна повторная попытка с заново запущенным PowerPoint.
                self.close_show();
                self.app = None;
                self.open_show(path, windowed, monitor)?;
            }
            let hwnd = self.win.as_ref().and_then(|w| w.int("HWND").ok()).unwrap_or(0);
            self.emit(PptEvent::Ready(self.total, hwnd as isize));
            self.sdir = slides;
            if let Some(dir) = self.sdir.clone() {
                let _ = self.plan_slides(&dir, width);
            }
            Ok(())
        }

        fn open_show(&mut self, path: &Path, windowed: bool, monitor: &str) -> R<()> {
            let app = self.app()?;
            set_monitor(&app, monitor);
            // Только для чтения, без заголовка «Без имени», без окна.
            let pres = app.obj("Presentations")?.obj_at("Open", &[s(&path.to_string_lossy()), i(-1), i(0), i(0)])?;
            self.pres = Some(pres.clone());
            self.total = pres.obj("Slides")?.int("Count")?.max(0) as usize;
            let ps = pres.obj("PageSetup")?;
            self.size = (ps.num("SlideWidth")?.max(1.0), ps.num("SlideHeight")?.max(1.0));
            let ss = pres.obj("SlideShowSettings")?;
            // 1 — докладчиком (на весь экран), 2 — в окне.
            ss.put("ShowType", i(if windowed { 2 } else { 1 }))?;
            let _ = ss.put("ShowPresenterView", i(0));
            let win = ss.obj("Run")?;
            self.win = Some(win.clone());
            // Настройки показа выше меняют презентацию. Помечаем её сохранённой: иначе при закрытии
            // PowerPoint спросит о сохранении файла, открытого только для чтения.
            let _ = pres.put("Saved", i(-1));
            // Указатель мыши: скрыт или стрелка при движении — как выбрано в программе.
            if let Ok(v) = win.obj("View") {
                let _ = v.put("PointerType", i(self.cursor));
            }
            Ok(())
        }

        fn close_show(&mut self) {
            // Exit у показа файла, открытого без окна, закрывает и сам файл; Close — на всякий случай.
            if let Some(w) = self.win.take() {
                if let Ok(v) = w.obj("View") {
                    let _ = v.call("Exit", &[]);
                }
            }
            if let Some(p) = self.pres.take() {
                let _ = p.put("Saved", i(-1));
                let _ = p.call("Close", &[]);
            }
            self.exp = None;
            self.mslide = -1;
            self.mlist.clear();
            self.mlast = None;
        }

        /// Команда показу (`None` — опрос). После неё — позиция и видео на слайде.
        fn command(&mut self, req: Option<Req>) {
            let Some(win) = self.win.clone() else { return };
            if let Err(_e) = self.command_inner(&win, req) {
                // PowerPoint мог быть просто занят; показ окончен, только если его окна больше нет.
                let alive = match self.app.as_ref().map(|a| a.obj("SlideShowWindows").and_then(|w| w.int("Count"))) {
                    Some(Ok(n)) => n > 0,
                    Some(Err(e)) => com::is_busy(&e),
                    None => false,
                };
                if !alive {
                    self.emit(PptEvent::Ended);
                    self.win = None;
                    self.close_show();
                }
            }
        }

        fn command_inner(&mut self, win: &Disp, req: Option<Req>) -> R<()> {
            let view = win.obj("View")?;
            match &req {
                Some(Req::Next) => {
                    // На последнем шаге последнего слайда не идём дальше, иначе показ закроется.
                    let cur = view.int("CurrentShowPosition")?.max(0) as usize;
                    let last = cur >= self.total
                        && match (view.int("GetClickIndex"), view.int("GetClickCount")) {
                            (Ok(a), Ok(b)) => a >= b,
                            _ => true,
                        };
                    if !last {
                        view.call("Next", &[])?;
                    }
                }
                Some(Req::Prev) => {
                    view.call("Previous", &[])?;
                }
                Some(Req::Goto(n)) => {
                    view.call("GotoSlide", &[i(*n as i32)])?;
                }
                Some(Req::Show) => {
                    // Сцена с фоном отпустила экран — выводим слайд-шоу на передний план.
                    let _ = win.call("Activate", &[]);
                }
                Some(Req::Media(action, id)) => {
                    // Видео на слайде — без мыши: через проигрыватель слайд-шоу.
                    let name = match action.as_str() {
                        "play" => "Play",
                        "pause" => "Pause",
                        _ => "Stop",
                    };
                    view.obj_at("Player", &[i(*id)])?.call(name, &[])?;
                }
                _ => {}
            }
            // Опрос PowerPoint лишний раз не трогаем — он может как раз менять слайд.
            if req.is_some() {
                if let Some(p) = &self.pres {
                    let _ = p.put("Saved", i(-1));
                }
            }
            let cur = view.int("CurrentShowPosition")?.max(0) as usize;
            // Ответ на опрос обычно тот же — без изменений GUI не будим.
            if self.last_pos != Some((cur, self.total)) {
                self.last_pos = Some((cur, self.total));
                self.emit(PptEvent::Pos(cur, self.total));
            }
            self.media_report(&view);
            Ok(())
        }

        /// Видео и звук на текущем слайде, их состояние и позиция — программе, только если что-то
        /// изменилось. Фигуры перебираем только при смене слайда, на опросе — лишь проигрыватели.
        fn media_report(&mut self, view: &Disp) {
            let report = self.media_list(view).unwrap_or((0, Vec::new()));
            if self.mlast.as_ref() != Some(&report) {
                self.mlast = Some(report.clone());
                self.emit(PptEvent::Media(report.0, report.1));
            }
        }

        fn media_list(&mut self, view: &Disp) -> R<(usize, Vec<Media>)> {
            let slide = view.obj("Slide")?;
            let idx = slide.int("SlideIndex")?;
            if idx != self.mslide {
                self.mslide = idx;
                let shapes = slide.obj("Shapes")?;
                let mut list = Vec::new();
                media_shapes(&shapes, &mut list);
                // «Видео 1, 2, 3» — в порядке чтения слайда: по строкам сверху вниз, в строке слева направо.
                list.sort_by(|a, b| ((a.top / 60.0).round_ties_even(), a.left).partial_cmp(&((b.top / 60.0).round_ties_even(), b.left)).unwrap_or(std::cmp::Ordering::Equal));
                self.mlist = list;
                if let Some(dir) = &self.sdir {
                    let mut lines = vec![format!("Слайд {idx}, найдено медиа: {}", self.mlist.len())];
                    inventory(&shapes, &mut lines, 0);
                    let _ = std::fs::write(dir.join(format!("shapes_{idx}.txt")), format!("\u{feff}{}\r\n", lines.join("\r\n")));
                }
            }
            let (sw, sh) = self.size;
            let mut out = Vec::with_capacity(self.mlist.len());
            for m in &mut self.mlist {
                let (mut state, mut pos) = (3u8, 0u32);
                if m.player.is_none() {
                    m.player = view.obj_at("Player", &[i(m.id)]).ok();
                }
                if let Some(p) = &m.player {
                    match (p.int("State"), p.num("CurrentPosition")) {
                        (Ok(st), Ok(c)) => {
                            state = st.clamp(0, 3) as u8;
                            pos = (c / 1000.0).floor().max(0.0) as u32;
                        }
                        _ => m.player = None,
                    }
                }
                out.push(Media {
                    id: m.id,
                    name: m.name.clone(),
                    video: m.video,
                    state,
                    pos,
                    dur: m.len,
                    rect: [(m.left / sw) as f32, (m.top / sh) as f32, ((m.left + m.w) / sw) as f32, ((m.top + m.h) / sh) as f32],
                });
            }
            Ok((idx.max(0) as usize, out))
        }

        /// Какие слайды сохранить картинками (уже сохранённые — пропускаем; все готовы — отметка done).
        fn plan_slides(&mut self, dir: &Path, width: u32) -> R<()> {
            if dir.join("done").exists() {
                return Ok(());
            }
            std::fs::create_dir_all(dir).map_err(com::io)?;
            let w = width as i32;
            let h = (w as f64 * self.size.1 / self.size.0).round() as i32;
            let todo = (1..=self.total).filter(|n| !dir.join(format!("{n}.jpg")).exists()).collect();
            self.exp = Some(Export { dir: dir.to_path_buf(), w, h, todo });
            Ok(())
        }

        /// Сохранить один слайд — сначала текущий и следующие за ним: они нужны превью раньше остальных.
        /// Пишем во временный файл и переименовываем: программа не прочитает недописанную картинку.
        fn slide_step(&mut self) {
            let cur = self.win.as_ref().and_then(|w| w.obj("View").ok()).and_then(|v| v.int("CurrentShowPosition").ok()).unwrap_or(1).max(1) as usize;
            let Some(e) = self.exp.as_mut() else { return };
            if e.todo.is_empty() {
                let _ = std::fs::write(e.dir.join("done"), "");
                self.exp = None;
                return;
            }
            let k = e.todo.iter().position(|&n| n >= cur).unwrap_or(0);
            let n = e.todo.remove(k);
            let (f, tmp, w, h) = (e.dir.join(format!("{n}.jpg")), e.dir.join(format!("{n}.part.jpg")), e.w, e.h);
            match self.export(n, &tmp, w, h).and_then(|_| replace(&tmp, &f)) {
                Ok(()) => self.emit(PptEvent::Slide(n)),
                Err(_) => self.exp = None,
            }
        }

        /// Слайд крупной картинкой — для увеличения: сцена покажет её поверх слайд-шоу.
        fn zoom(&mut self, n: usize, width: u32, file: &Path) -> R<()> {
            let w = width as i32;
            let h = (w as f64 * self.size.1 / self.size.0).round() as i32;
            let mut tmp = file.as_os_str().to_owned();
            tmp.push(".part.jpg");
            let tmp = PathBuf::from(tmp);
            self.export(n, &tmp, w, h)?;
            replace(&tmp, file)
        }

        fn export(&self, n: usize, file: &Path, w: i32, h: i32) -> R<()> {
            let pres = self.pres.as_ref().ok_or_else(|| com::io(std::io::Error::other("показ закрыт")))?;
            pres.obj("Slides")?.obj_at("Item", &[i(n as i32)])?.call("Export", &[s(&file.to_string_lossy()), s("JPG"), i(w), i(h)])?;
            Ok(())
        }
    }

    fn replace(tmp: &Path, file: &Path) -> R<()> {
        let _ = std::fs::remove_file(file);
        std::fs::rename(tmp, file).map_err(com::io)
    }

    /// Монитор для показа (\\.\DISPLAY2) — в собственную настройку PowerPoint «Показ на мониторе».
    fn set_monitor(app: &Disp, dev: &str) {
        if dev.is_empty() {
            return;
        }
        if let Ok(ver) = app.text("Version") {
            let _ = crate::registry::set(&format!(r"Software\Microsoft\Office\{ver}\PowerPoint\Options"), "DisplayMonitor", dev);
        }
    }

    /// Видео и звук среди фигур — и внутри групп на любой глубине. Медиа — фигура с типом «медиа» (16),
    /// заполнитель с медиа внутри или любая фигура с параметрами медиа (MediaFormat): по одному типу
    /// находилось не всё (замечание пользователя 16.10.2026).
    fn media_shapes(shapes: &Disp, list: &mut Vec<MShape>) {
        let count = shapes.int("Count").unwrap_or(0);
        for k in 1..=count {
            let Ok(sh) = shapes.obj_at("Item", &[i(k as i32)]) else { continue };
            let t = sh.int("Type").unwrap_or(0);
            if t == 6 {
                if let Ok(g) = sh.obj("GroupItems") {
                    media_shapes(&g, list);
                }
                continue;
            }
            let mut media = t == 16;
            if !media && t == 14 {
                media = sh.obj("PlaceholderFormat").and_then(|p| p.int("ContainedType")).is_ok_and(|c| c == 16);
            }
            // У обычных фигур MediaFormat пустой (без ошибки) — медиа только там, где он есть.
            let format = sh.obj("MediaFormat").ok();
            media |= format.is_some();
            if !media {
                continue;
            }
            // MediaType: 2 — звук, 3 — видео.
            let kind = sh.int("MediaType").unwrap_or(3);
            // Длительность — у самой фигуры: Player.Duration у PowerPoint пустой (проверено 18.10.2026).
            let len = format.and_then(|f| f.num("Length").ok()).map_or(0, |l| (l / 1000.0).round().max(0.0) as u32);
            list.push(MShape {
                id: sh.int("Id").unwrap_or(0) as i32,
                name: sh.text("Name").unwrap_or_default(),
                video: kind != 2,
                len,
                left: sh.num("Left").unwrap_or(0.0),
                top: sh.num("Top").unwrap_or(0.0),
                w: sh.num("Width").unwrap_or(0.0),
                h: sh.num("Height").unwrap_or(0.0),
                player: None,
            });
        }
    }

    /// Перечень всех фигур слайда (с группами) — в файл shapes_N.txt папки слайдов: если видео не
    /// нашлось, по нему видно, как оно вставлено.
    fn inventory(shapes: &Disp, lines: &mut Vec<String>, depth: usize) {
        let opt = |r: R<i64>| r.map(|x| x.to_string()).unwrap_or_default();
        let count = shapes.int("Count").unwrap_or(0);
        for k in 1..=count {
            let Ok(sh) = shapes.obj_at("Item", &[i(k as i32)]) else { continue };
            let t = sh.int("Type");
            let mf = match sh.get("MediaFormat") {
                Ok(v) => com::to_disp(&v).is_ok().to_string(),
                Err(_) => "ошибка".into(),
            };
            let link = sh.obj("LinkFormat").and_then(|l| l.text("SourceFullName")).unwrap_or_default();
            lines.push(format!(
                "{}{} | id {} | тип {} | в заполнителе {} | медиа {} | MediaFormat {} | {:.0},{:.0} {:.0}x{:.0} | {}",
                "  ".repeat(depth),
                sh.text("Name").unwrap_or_default(),
                opt(sh.int("Id")),
                t.as_ref().map(|x| x.to_string()).unwrap_or_default(),
                opt(sh.obj("PlaceholderFormat").and_then(|p| p.int("ContainedType"))),
                opt(sh.int("MediaType")),
                mf,
                sh.num("Left").unwrap_or(0.0),
                sh.num("Top").unwrap_or(0.0),
                sh.num("Width").unwrap_or(0.0),
                sh.num("Height").unwrap_or(0.0),
                link
            ));
            if t.is_ok_and(|t| t == 6) {
                if let Ok(g) = sh.obj("GroupItems") {
                    inventory(&g, lines, depth + 1);
                }
            }
        }
    }
}

/// Без Windows PowerPoint нет: на каждое открытие — ошибка.
#[cfg(not(windows))]
mod bridge {
    use super::{Ctx, PptEvent, Req};
    use std::sync::mpsc::Receiver;

    pub fn run(rx: Receiver<Req>, ctx: Ctx) {
        while let Ok(req) = rx.recv() {
            match req {
                Req::Open { sid, .. } => (ctx.on_event)(sid, PptEvent::Error("PowerPoint есть только в Windows".into())),
                Req::Quit => break,
                _ => {}
            }
        }
    }
}
