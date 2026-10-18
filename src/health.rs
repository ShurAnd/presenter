//! Состояние компьютера-сервера: сбор показателей (раз в 2 с, в своём потоке) и их оценка
//! «норма / внимание / проблема». Сервер рассылает показатели клиентам, клиент показывает их
//! оператору и поднимает тревогу, если что-то идёт не так.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Показатели сервера. Поля, которые не удалось получить, — `None`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct HealthInfo {
    /// Загрузка процессора, % — в среднем за последние 10 с (короткие всплески не тревожат).
    pub cpu: f32,
    pub ram_used_mb: u64,
    pub ram_total_mb: u64,
    /// Память программы Presentator и PowerPoint (если запущен), МБ.
    pub app_mb: u64,
    pub ppt_mb: Option<u64>,
    /// Диск, куда сохраняются присланные файлы.
    pub disk_free_mb: u64,
    pub disk_total_mb: u64,
    /// Загрузка видеокарты (3D), % — в среднем за 10 с; видеопамять, МБ.
    pub gpu: Option<f32>,
    pub vram_used_mb: Option<u64>,
    pub vram_total_mb: Option<u64>,
    /// Видеокарта встроенная: память общая с ОЗУ (показываем её, а не почти пустую «свою»).
    pub vram_shared: bool,
    /// Мониторы («1920×1080, основной») и наибольшее их число с запуска сервера.
    pub monitors: Vec<String>,
    pub monitors_max: u32,
    /// Интерфейс программы на сервере отвечает (не завис).
    pub ui_ok: bool,
    /// Сколько PowerPoint не отвечает мосту, мс (None — показа PPTX нет).
    pub ppt_lag_ms: Option<u64>,
    pub app_uptime_s: u64,
    pub pc_uptime_s: u64,
}

#[derive(Clone, Copy, PartialEq, PartialOrd, Debug)]
pub enum Level {
    Ok,
    Warn,
    Bad,
}

pub struct Row {
    pub name: &'static str,
    pub value: String,
    pub level: Level,
}

/// Оценка для оператора. `rtt` — задержка связи, `silent` — сколько от сервера нет вестей
/// (только для клиента; на самом сервере оба `None`).
pub fn assess(h: Option<&HealthInfo>, rtt: Option<u32>, silent: Option<Duration>) -> Vec<Row> {
    let mut rows = Vec::new();
    if let Some(s) = silent {
        let lost = s > Duration::from_secs(6);
        let value = match (lost, rtt) {
            (true, _) => format!("нет ответа {} с", s.as_secs()),
            (false, Some(ms)) => format!("задержка {ms} мс"),
            (false, None) => "есть".into(),
        };
        let level = if lost || rtt.is_some_and(|ms| ms > 1000) {
            Level::Bad
        } else if rtt.is_some_and(|ms| ms > 300) {
            Level::Warn
        } else {
            Level::Ok
        };
        rows.push(Row { name: "Связь с сервером", value, level });
    }
    let Some(h) = h else { return rows };
    let pct = |used: u64, total: u64| if total == 0 { 0.0 } else { used as f32 * 100.0 / total as f32 };
    let lvl = |v: f32, warn: f32, bad: f32| if v >= bad { Level::Bad } else if v >= warn { Level::Warn } else { Level::Ok };
    rows.push(Row {
        name: "Программа на сервере",
        value: if h.ui_ok { "отвечает".into() } else { "НЕ ОТВЕЧАЕТ (зависла)".into() },
        level: if h.ui_ok { Level::Ok } else { Level::Bad },
    });
    if let Some(lag) = h.ppt_lag_ms {
        rows.push(Row {
            name: "PowerPoint",
            value: if lag < 1500 { "отвечает".into() } else { format!("не отвечает {:.0} с", lag as f64 / 1000.0) },
            level: if lag >= 8000 { Level::Bad } else if lag >= 3000 { Level::Warn } else { Level::Ok },
        });
    }
    rows.push(Row { name: "Процессор (за 10 с)", value: format!("{:.0} %", h.cpu), level: lvl(h.cpu, 85.0, 97.0) });
    let ram = pct(h.ram_used_mb, h.ram_total_mb);
    rows.push(Row {
        name: "Оперативная память",
        value: format!("{:.0} % ({} из {})", ram, gb(h.ram_used_mb), gb(h.ram_total_mb)),
        level: lvl(ram, 85.0, 95.0),
    });
    let mut apps = format!("Presentator {}", mb(h.app_mb));
    if let Some(p) = h.ppt_mb {
        apps += &format!(", PowerPoint {}", mb(p));
    }
    rows.push(Row { name: "Память программ", value: apps, level: Level::Ok });
    if let Some(g) = h.gpu {
        rows.push(Row { name: "Видеокарта (за 10 с)", value: format!("{g:.0} %"), level: lvl(g, 90.0, 99.0) });
    }
    if let (Some(u), Some(t)) = (h.vram_used_mb, h.vram_total_mb.filter(|&t| t > 0)) {
        let v = pct(u, t);
        let name = if h.vram_shared { "Видеопамять (общая с ОЗУ)" } else { "Видеопамять" };
        rows.push(Row { name, value: format!("{:.0} % ({} из {})", v, gb(u), gb(t)), level: lvl(v, 90.0, 97.0) });
    }
    let disk_level = if h.disk_free_mb < 1024 { Level::Bad } else if h.disk_free_mb < 5 * 1024 { Level::Warn } else { Level::Ok };
    rows.push(Row { name: "Свободно на диске", value: format!("{} из {}", gb(h.disk_free_mb), gb(h.disk_total_mb)), level: disk_level });
    let n = h.monitors.len() as u32;
    rows.push(Row {
        name: "Мониторы",
        value: if n < h.monitors_max {
            format!("ОТКЛЮЧИЛСЯ МОНИТОР: {} из {} — {}", n, h.monitors_max, h.monitors.join("; "))
        } else {
            h.monitors.join("; ")
        },
        level: if n == 0 || n < h.monitors_max { Level::Bad } else { Level::Ok },
    });
    rows.push(Row {
        name: "Работает",
        value: format!("программа {}, компьютер {}", dur(h.app_uptime_s), dur(h.pc_uptime_s)),
        level: Level::Ok,
    });
    rows
}

/// Общий уровень — худший из строк.
pub fn overall(rows: &[Row]) -> Level {
    rows.iter().map(|r| r.level).fold(Level::Ok, |a, b| if b > a { b } else { a })
}

fn gb(mb_: u64) -> String {
    if mb_ >= 1024 { format!("{:.1} ГБ", mb_ as f64 / 1024.0) } else { mb(mb_) }
}

fn mb(mb: u64) -> String {
    format!("{mb} МБ")
}

fn dur(s: u64) -> String {
    let (d, h, m) = (s / 86400, s % 86400 / 3600, s % 3600 / 60);
    if d > 0 { format!("{d} д {h} ч") } else if h > 0 { format!("{h} ч {m} мин") } else { format!("{m} мин") }
}

// ---------------------------------------------------------------- сбор

/// Пока жив — раз в 2 с собирает показатели и отдаёт их в `out`.
pub struct Collector {
    stop: Arc<AtomicBool>,
}

impl Drop for Collector {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Раз в сколько замеров будить интерфейс, чтобы проверить, что он не завис (5 × 2 с = 10 с).
/// Чаще не надо: каждое пробуждение перерисовывает и полноэкранную сцену.
const WAKE_EVERY: u32 = 5;

/// `ui_frames` — счётчик кадров интерфейса: если после `wake` он не вырос, интерфейс завис.
/// `ppt_lag` — сколько PowerPoint не отвечает (None — показа нет).
/// `out` получает каждый замер (раз в 2 с) — без пробуждения интерфейса.
pub fn start(
    work_dir: PathBuf,
    ui_frames: Arc<AtomicU64>,
    ppt_lag: impl Fn() -> Option<u64> + Send + 'static,
    out: impl Fn(HealthInfo) + Send + 'static,
    wake: impl Fn() + Send + 'static,
) -> Collector {
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let mut sys = win::Sampler::new();
        let mut monitors_max = 0u32;
        // Проверка интерфейса: будим его и на следующем замере смотрим, был ли кадр.
        let (mut tick, mut ui_ok, mut woke_at): (u32, bool, Option<u64>) = (0, true, None);
        // Последние 5 замеров (10 с) процессора и видеокарты — для среднего.
        let (mut cpu_hist, mut gpu_hist) = (std::collections::VecDeque::new(), std::collections::VecDeque::new());
        let avg = |q: &mut std::collections::VecDeque<f32>, v: f32| {
            q.push_back(v);
            if q.len() > 5 {
                q.pop_front();
            }
            q.iter().sum::<f32>() / q.len() as f32
        };
        while !st.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_secs(2));
            if st.load(Ordering::SeqCst) {
                break;
            }
            let mut h = sys.sample(&work_dir);
            h.cpu = avg(&mut cpu_hist, h.cpu);
            h.gpu = h.gpu.map(|g| avg(&mut gpu_hist, g));
            monitors_max = monitors_max.max(h.monitors.len() as u32);
            h.monitors_max = monitors_max;
            if let Some(f) = woke_at.take() {
                ui_ok = ui_frames.load(Ordering::SeqCst) != f;
            }
            h.ui_ok = ui_ok;
            h.ppt_lag_ms = ppt_lag();
            h.app_uptime_s = started.elapsed().as_secs();
            out(h);
            tick += 1;
            if tick % WAKE_EVERY == 0 {
                woke_at = Some(ui_frames.load(Ordering::SeqCst));
                wake();
            }
        }
    });
    Collector { stop }
}

/// Номер процесса PowerPoint, если он запущен.
pub fn powerpoint_pid() -> Option<u32> {
    #[cfg(windows)]
    return win::powerpoint_pid();
    #[cfg(not(windows))]
    None
}

#[cfg(not(windows))]
mod win {
    use super::HealthInfo;
    use std::path::Path;
    pub struct Sampler;
    impl Sampler {
        pub fn new() -> Self {
            Sampler
        }
        pub fn sample(&mut self, _: &Path) -> HealthInfo {
            HealthInfo::default()
        }
    }
}

#[cfg(windows)]
mod win {
    use super::HealthInfo;
    use std::path::Path;
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, BOOL, FILETIME, LPARAM, RECT};
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows::Win32::System::Performance::*;
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetExitCodeProcess, GetSystemTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    const MB: u64 = 1024 * 1024;

    pub struct Sampler {
        cpu_prev: Option<(u64, u64)>,
        /// Процесс PowerPoint из прошлого замера — поиск по списку процессов дорогой (~4 мс).
        ppt_pid: Option<u32>,
        /// Если PowerPoint не запущен — ищем его не на каждом замере, а раз в 5 (10 с).
        ppt_skip: u32,
        gpu: Option<Gpu>,
        /// (своя видеопамять, лимит общей памяти), МБ — у самой «большой» видеокарты.
        vram: Option<(u64, u64)>,
    }

    /// Меньше этого «своей» видеопамяти — видеокарта встроенная, работает на общей памяти.
    const INTEGRATED_MB: u64 = 512;

    /// Счётчики видеокарты (PDH): загрузка 3D-движков и занятая видеопамять.
    struct Gpu {
        query: isize,
        engine: isize,
        dedicated: isize,
        shared: isize,
    }

    impl Drop for Gpu {
        fn drop(&mut self) {
            unsafe {
                PdhCloseQuery(self.query);
            }
        }
    }

    fn ft(f: FILETIME) -> u64 {
        (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64
    }

    impl Sampler {
        pub fn new() -> Self {
            Sampler { cpu_prev: None, ppt_pid: None, ppt_skip: 0, gpu: Gpu::open(), vram: vram_mb() }
        }

        pub fn sample(&mut self, dir: &Path) -> HealthInfo {
            let mut h = HealthInfo::default();
            unsafe {
                // Процессор: доля «не простоя» за время с прошлого замера.
                let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
                if GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).is_ok() {
                    let (busy_total, idle_now) = (ft(kernel) + ft(user), ft(idle));
                    if let Some((t0, i0)) = self.cpu_prev {
                        let (dt, di) = (busy_total.saturating_sub(t0), idle_now.saturating_sub(i0));
                        if dt > 0 {
                            h.cpu = ((dt.saturating_sub(di)) as f32 * 100.0 / dt as f32).clamp(0.0, 100.0);
                        }
                    }
                    self.cpu_prev = Some((busy_total, idle_now));
                }
                let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
                if GlobalMemoryStatusEx(&mut m).is_ok() {
                    h.ram_total_mb = m.ullTotalPhys / MB;
                    h.ram_used_mb = (m.ullTotalPhys - m.ullAvailPhys) / MB;
                }
                h.app_mb = process_private_mb(GetCurrentProcess()).unwrap_or(0);
                let mem = |pid: u32| {
                    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
                    // Процесс завершился (а pid мог достаться другому) — код выхода уже не STILL_ACTIVE.
                    let mut code = 0u32;
                    let alive = GetExitCodeProcess(handle, &mut code).is_ok() && code == 259;
                    let r = if alive { process_private_mb(handle) } else { None };
                    let _ = CloseHandle(handle);
                    r
                };
                h.ppt_mb = self.ppt_pid.and_then(mem);
                if h.ppt_mb.is_none() {
                    if self.ppt_skip == 0 {
                        self.ppt_pid = find_process("POWERPNT.EXE");
                        h.ppt_mb = self.ppt_pid.and_then(mem);
                        self.ppt_skip = if h.ppt_mb.is_none() { 4 } else { 0 };
                    } else {
                        self.ppt_skip -= 1;
                    }
                }
                // Диск — тот, где лежит папка для присланных файлов (или её родитель, если её ещё нет).
                let probe = dir.ancestors().find(|p| p.exists()).unwrap_or(dir);
                let (mut free, mut total) = (0u64, 0u64);
                if GetDiskFreeSpaceExW(&HSTRING::from(probe.as_os_str()), Some(&mut free), Some(&mut total), None).is_ok() {
                    h.disk_free_mb = free / MB;
                    h.disk_total_mb = total / MB;
                }
                h.monitors = monitors();
                h.pc_uptime_s = GetTickCount64() / 1000;
            }
            if let Some(g) = &self.gpu {
                let (load, dedicated, shared) = g.sample();
                h.gpu = load;
                match self.vram {
                    Some((own, _)) if own >= INTEGRATED_MB => (h.vram_used_mb, h.vram_total_mb) = (dedicated, Some(own)),
                    Some((_, limit)) if limit > 0 => (h.vram_used_mb, h.vram_total_mb, h.vram_shared) = (shared, Some(limit), true),
                    _ => {}
                }
            }
            h
        }
    }

    #[cfg(test)]
    pub fn cost_probe_gpu(s: &Sampler) -> Option<f32> {
        s.gpu.as_ref().and_then(|g| g.sample().0)
    }

    #[cfg(test)]
    pub fn cost_probe_ppt() -> Option<u32> {
        unsafe { find_process("POWERPNT.EXE") }
    }

    /// Номер процесса PowerPoint (он запускается в одном экземпляре на пользователя).
    pub fn powerpoint_pid() -> Option<u32> {
        unsafe { find_process("POWERPNT.EXE") }
    }

    unsafe fn process_private_mb(handle: windows::Win32::Foundation::HANDLE) -> Option<u64> {
        let mut c = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
        GetProcessMemoryInfo(handle, &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb).ok()?;
        Some(c.PrivateUsage as u64 / MB)
    }

    unsafe fn find_process(exe: &str) -> Option<u32> {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = None;
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                if String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case(exe) {
                    found = Some(e.th32ProcessID);
                    break;
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        found
    }

    unsafe extern "system" fn monitor_cb(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let list = &mut *(data.0 as *mut Vec<String>);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(m, &mut info).as_bool() {
            let r = info.rcMonitor;
            let primary = info.dwFlags & 1 != 0; // MONITORINFOF_PRIMARY
            list.push(format!("{}×{}{}", r.right - r.left, r.bottom - r.top, if primary { ", основной" } else { "" }));
        }
        true.into()
    }

    unsafe fn monitors() -> Vec<String> {
        let mut list: Vec<String> = Vec::new();
        let _ = EnumDisplayMonitors(None, None, Some(monitor_cb), LPARAM(&mut list as *mut _ as isize));
        list
    }

    /// (своя видеопамять, лимит общей памяти) самой «большой» видеокарты (DXGI), МБ.
    fn vram_mb() -> Option<(u64, u64)> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
            let mut best: Option<(u64, u64)> = None;
            let mut i = 0;
            while let Ok(a) = factory.EnumAdapters1(i) {
                if let Ok(d) = a.GetDesc1() {
                    // Программный «Microsoft Basic Render Driver» (флаг SOFTWARE = 2) не считаем.
                    if d.Flags & 2 == 0 {
                        let cur = (d.DedicatedVideoMemory as u64 / MB, d.SharedSystemMemory as u64 / MB);
                        if best.is_none_or(|b| cur.0 > b.0) {
                            best = Some(cur);
                        }
                    }
                }
                i += 1;
            }
            best
        }
    }

    impl Gpu {
        fn open() -> Option<Gpu> {
            unsafe {
                let mut query = 0isize;
                if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                    return None;
                }
                let (mut engine, mut dedicated, mut shared) = (0isize, 0isize, 0isize);
                let engine_ok = PdhAddEnglishCounterW(query, w!("\\GPU Engine(*engtype_3D)\\Utilization Percentage"), 0, &mut engine) == 0;
                let dedicated_ok = PdhAddEnglishCounterW(query, w!("\\GPU Adapter Memory(*)\\Dedicated Usage"), 0, &mut dedicated) == 0;
                let shared_ok = PdhAddEnglishCounterW(query, w!("\\GPU Adapter Memory(*)\\Shared Usage"), 0, &mut shared) == 0;
                let ok = engine_ok && dedicated_ok && shared_ok;
                let g = Gpu { query, engine, dedicated, shared };
                if !ok {
                    return None;
                }
                PdhCollectQueryData(query); // первый замер — точка отсчёта для процентов
                Some(g)
            }
        }

        /// (загрузка 3D, %; занятая своя видеопамять, МБ; занятая общая память, МБ)
        fn sample(&self) -> (Option<f32>, Option<u64>, Option<u64>) {
            unsafe {
                if PdhCollectQueryData(self.query) != 0 {
                    return (None, None, None);
                }
                let load = sum(self.engine).map(|v| v.clamp(0.0, 100.0) as f32);
                let mb = |c| sum(c).map(|v| (v / MB as f64) as u64);
                (load, mb(self.dedicated), mb(self.shared))
            }
        }
    }

    /// Сумма значений счётчика по всем экземплярам (процессам, движкам, видеокартам).
    unsafe fn sum(counter: isize) -> Option<f64> {
        let (mut size, mut count) = (0u32, 0u32);
        let r = PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None);
        if r != PDH_MORE_DATA || size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
            return None;
        }
        let items = std::slice::from_raw_parts(items, count as usize);
        Some(items.iter().filter(|i| i.FmtValue.CStatus == 0).map(|i| i.FmtValue.Anonymous.doubleValue).sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Печатает два замера показателей этого компьютера:
    /// cargo test --release health_sample -- --ignored --nocapture
    #[test]
    #[ignore]
    fn health_sample() {
        let mut s = win::Sampler::new();
        let _ = s.sample(&std::env::temp_dir());
        std::thread::sleep(Duration::from_secs(2));
        let mut h = s.sample(&std::env::temp_dir());
        h.ui_ok = true;
        h.monitors_max = h.monitors.len() as u32;
        println!("{h:#?}");
        for r in assess(Some(&h), Some(12), Some(Duration::from_secs(1))) {
            println!("{:?}  {}: {}", r.level, r.name, r.value);
        }
    }

    #[test]
    fn assess_levels() {
        let h = HealthInfo { cpu: 50.0, ram_used_mb: 15_500, ram_total_mb: 16_000, disk_free_mb: 500, disk_total_mb: 100_000, monitors: vec!["1920×1080".into()], monitors_max: 2, ui_ok: true, ..Default::default() };
        let rows = assess(Some(&h), Some(20), Some(Duration::from_secs(1)));
        let lvl = |name: &str| rows.iter().find(|r| r.name == name).unwrap().level;
        assert_eq!(lvl("Оперативная память"), Level::Bad); // 97 %
        assert_eq!(lvl("Свободно на диске"), Level::Bad); // 0,5 ГБ
        assert_eq!(lvl("Мониторы"), Level::Bad); // было 2, стал 1
        assert_eq!(lvl("Процессор (за 10 с)"), Level::Ok);
        assert_eq!(overall(&rows), Level::Bad);
        // Сервер молчит 10 с — тревога по связи.
        let rows = assess(None, None, Some(Duration::from_secs(10)));
        assert_eq!(overall(&rows), Level::Bad);
    }
}

#[cfg(all(test, windows))]
mod cost {
    /// Сколько стоит каждый замер: cargo test --release health_cost -- --ignored --nocapture
    #[test]
    #[ignore]
    fn health_cost() {
        use std::time::Instant;
        let mut s = super::win::Sampler::new();
        let dir = std::env::temp_dir();
        let _ = s.sample(&dir);
        let t = Instant::now();
        for _ in 0..10 {
            let _ = s.sample(&dir);
        }
        println!("полный замер: {:.1} мс", t.elapsed().as_secs_f64() * 100.0);
        let t = Instant::now();
        for _ in 0..10 {
            let _ = super::win::cost_probe_gpu(&s);
        }
        println!("видеокарта (PDH): {:.1} мс", t.elapsed().as_secs_f64() * 100.0);
        let t = Instant::now();
        for _ in 0..10 {
            let _ = super::win::cost_probe_ppt();
        }
        println!("поиск PowerPoint: {:.1} мс", t.elapsed().as_secs_f64() * 100.0);
    }
}
