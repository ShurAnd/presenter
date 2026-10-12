//! Запись мероприятия (вкладка «Запись») — всегда на этом компьютере, по сети ничего не идёт.
//! Пишет FFmpeg: он вшит в exe в сжатом виде (`vendor/ffmpeg`), при первом использовании
//! распаковывается в `%LOCALAPPDATA%\Presentator\` и запускается отдельным процессом без окна.
//!
//! Источники — устройства DirectShow (карта захвата HDMI, камера, микрофон, линейный вход).
//! Видео сжимает видеокарта (NVENC / QuickSync / AMF), запасной — Media Foundation и OpenH264.
//! Пока идёт запись, файл — MKV (переживает сбой: записанное до сбоя читается); после «Стоп» он
//! перепаковывается в MP4 (только звук — в M4A) без пересжатия.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui::ColorImage;

const FF_TAG: &str = "n8.1.3-20261011";
const FF_EXE: &str = "ffmpeg.exe";
const FF_EXE_SIZE: u64 = 134_092_288;
static FF_ZIP: &[u8] = include_bytes!("../vendor/ffmpeg/ffmpeg.zip");

/// Кодировщики видео по порядку предпочтения: сначала видеокарты, потом программные.
const ENCODERS: [(&str, &str); 5] = [
    ("h264_nvenc", "видеокарта NVIDIA (NVENC)"),
    ("h264_qsv", "видеокарта Intel (QuickSync)"),
    ("h264_amf", "видеокарта AMD (AMF)"),
    ("h264_mf", "Windows Media Foundation"),
    ("libopenh264", "процессор (OpenH264)"),
];

/// Сколько ждать, пока FFmpeg сам допишет файл после «q», прежде чем завершить его силой.
const STOP_WAIT: Duration = Duration::from_secs(8);

static FFMPEG: OnceLock<Result<PathBuf, String>> = OnceLock::new();

/// Путь к ffmpeg.exe (распаковывается при первом вызове).
pub fn ffmpeg() -> Result<PathBuf, String> {
    FFMPEG.get_or_init(extract).clone()
}

fn extract() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join("Presentator").join(format!("ffmpeg-{FF_TAG}"));
    let exe = dir.join(FF_EXE);
    if std::fs::metadata(&exe).is_ok_and(|m| m.len() == FF_EXE_SIZE) {
        return Ok(exe);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("не удалось создать {}: {e}", dir.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(FF_ZIP)).map_err(|e| format!("архив FFmpeg повреждён: {e}"))?;
    let mut entry = zip.by_name(FF_EXE).map_err(|e| format!("в архиве нет {FF_EXE}: {e}"))?;
    // Во временный файл и переименование — недописанный exe не останется под настоящим именем.
    let tmp = dir.join(format!("{FF_EXE}.{}.tmp", std::process::id()));
    let mut f = std::fs::File::create(&tmp).map_err(|e| format!("не удалось записать {}: {e}", tmp.display()))?;
    std::io::copy(&mut entry, &mut f).map_err(|e| format!("не удалось распаковать FFmpeg: {e}"))?;
    drop(f);
    if std::fs::rename(&tmp, &exe).is_err() {
        let _ = std::fs::remove_file(&tmp);
        if !std::fs::metadata(&exe).is_ok_and(|m| m.len() == FF_EXE_SIZE) {
            return Err("не удалось сохранить FFmpeg".into());
        }
    }
    Ok(exe)
}

/// FFmpeg без консольного окна.
fn ff(exe: &Path) -> Command {
    let mut c = Command::new(exe);
    c.arg("-hide_banner");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// Формат, который отдаёт видеоустройство.
#[derive(Clone, Debug, PartialEq)]
pub struct Mode {
    pub w: u32,
    pub h: u32,
    pub fps: String,
    /// Сжатый поток (`mjpeg`, `h264`) — `-vcodec`; иначе несжатый с `pixel_format`.
    pub vcodec: Option<String>,
    pub pixfmt: Option<String>,
}

impl Mode {
    pub fn label(&self) -> String {
        let fps = self.fps.parse::<f64>().map(|f| format!("{}", (f * 100.0).round() / 100.0)).unwrap_or_else(|_| self.fps.clone());
        let kind = self.vcodec.clone().or_else(|| self.pixfmt.clone()).unwrap_or_default().to_uppercase();
        format!("{}×{}, {fps} к/с, {kind}", self.w, self.h)
    }

    fn fps_f(&self) -> f64 {
        self.fps.parse().unwrap_or(30.0)
    }
}

/// Лучший формат: наибольшее разрешение, затем наибольшая частота кадров, при равенстве — сжатый
/// (у USB-карт несжатый 1080p обычно идёт всего 5–10 к/с — это видно по частоте).
pub fn best_mode(modes: &[Mode]) -> Option<&Mode> {
    modes.iter().max_by(|a, b| {
        (a.w * a.h, a.fps_f() as u64 * 1000, a.vcodec.is_some()).partial_cmp(&(b.w * b.h, b.fps_f() as u64 * 1000, b.vcodec.is_some())).unwrap()
    })
}

pub enum RecEvent {
    /// Найденные устройства: видео и звук.
    Devices(Result<(Vec<String>, Vec<String>), String>),
    Modes { device: String, modes: Vec<Mode> },
    /// Каким кодировщиком будет сжиматься видео (имя FFmpeg, описание).
    Encoder(Result<(String, String), String>),
    /// События записи (или проверки сигнала) с номером сеанса.
    Session { gen: u64, ev: SessionEv },
}

pub enum SessionEv {
    Progress { secs: f64, bytes: u64, fps: f32, dropped: u64 },
    /// Громкость (мгновенная, LUFS; −120 — тишина).
    Level(f32),
    Preview(ColorImage),
    /// FFmpeg остановлен, файл перепаковывается.
    Saving,
    /// Сеанс закончен: путь к готовому файлу (для проверки сигнала — `None`) или ошибка.
    Done(Result<Option<PathBuf>, String>),
}

/// Список устройств, затем кодировщик — в отдельном потоке.
pub fn scan(out: Sender<RecEvent>, wake: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let _ = out.send(RecEvent::Devices(list_devices()));
        wake();
        let _ = out.send(RecEvent::Encoder(encoder().clone()));
        wake();
    });
}

/// Форматы видеоустройства — в отдельном потоке.
pub fn scan_modes(device: String, out: Sender<RecEvent>, wake: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let modes = list_modes(&device).unwrap_or_default();
        let _ = out.send(RecEvent::Modes { device, modes });
        wake();
    });
}

fn stderr_of(cmd: &mut Command) -> Result<String, String> {
    let o = cmd.stdin(Stdio::null()).output().map_err(|e| format!("не удалось запустить FFmpeg: {e}"))?;
    Ok(String::from_utf8_lossy(&o.stderr).into_owned())
}

/// Устройства DirectShow: строки вида `[in#0 @ …] "Имя" (video)`.
pub fn list_devices() -> Result<(Vec<String>, Vec<String>), String> {
    let exe = ffmpeg()?;
    let err = stderr_of(ff(&exe).args(["-list_devices", "true", "-f", "dshow", "-i", "dummy"]))?;
    Ok(parse_devices(&err))
}

fn parse_devices(text: &str) -> (Vec<String>, Vec<String>) {
    let (mut video, mut audio) = (Vec::new(), Vec::new());
    for line in text.lines() {
        let Some(start) = line.find("] \"") else { continue };
        let rest = &line[start + 3..];
        let Some(end) = rest.rfind("\" (") else { continue };
        let name = rest[..end].to_string();
        match &rest[end + 3..] {
            k if k.starts_with("video") => video.push(name),
            k if k.starts_with("audio") => audio.push(name),
            // «(none)» — устройство без потоков, пропускаем.
            _ => {}
        }
    }
    (video, audio)
}

/// Форматы устройства: `vcodec=mjpeg  min s=… fps=… max s=1920x1080 fps=30`.
pub fn list_modes(device: &str) -> Result<Vec<Mode>, String> {
    let exe = ffmpeg()?;
    let err = stderr_of(ff(&exe).args(["-list_options", "true", "-f", "dshow", "-i", &format!("video={device}")]))?;
    Ok(parse_modes(&err))
}

fn parse_modes(text: &str) -> Vec<Mode> {
    let mut modes: Vec<Mode> = Vec::new();
    for line in text.lines() {
        let Some(max) = line.find("max s=") else { continue };
        let (vcodec, pixfmt) = if let Some(i) = line.find("vcodec=") {
            (line[i + 7..].split_whitespace().next().map(str::to_string), None)
        } else if let Some(i) = line.find("pixel_format=") {
            (None, line[i + 13..].split_whitespace().next().map(str::to_string))
        } else {
            continue;
        };
        let mut it = line[max + 6..].split_whitespace();
        let Some((w, h)) = it.next().and_then(|s| s.split_once('x')) else { continue };
        let (Ok(w), Ok(h)) = (w.parse(), h.parse()) else { continue };
        let Some(fps) = it.next().and_then(|s| s.strip_prefix("fps=")) else { continue };
        let m = Mode { w, h, fps: fps.to_string(), vcodec, pixfmt };
        if !modes.contains(&m) {
            modes.push(m);
        }
    }
    modes
}

static ENCODER: OnceLock<Result<(String, String), String>> = OnceLock::new();

/// Первый кодировщик из `ENCODERS`, который на этом компьютере действительно сжимает
/// (проба — 10 кадров чёрного поля); результат запоминается.
pub fn encoder() -> &'static Result<(String, String), String> {
    ENCODER.get_or_init(|| {
        let exe = ffmpeg()?;
        for (name, label) in ENCODERS {
            let ok = ff(&exe)
                .args(["-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=640x360:r=30", "-frames:v", "10", "-c:v", name, "-f", "null", "-"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if ok {
                return Ok((name.to_string(), label.to_string()));
            }
        }
        Err("ни один кодировщик видео не работает".into())
    })
}

/// Что записывать.
pub struct Opts {
    pub video: Option<String>,
    /// Формат видео; `None` — какой отдаст устройство.
    pub mode: Option<Mode>,
    pub audio: Option<String>,
    /// Папка для записей; `None` — только проверка сигнала (превью и уровень, без файла).
    pub dir: Option<PathBuf>,
    pub encoder: String,
}

/// Идущая запись (или проверка сигнала). Остановка — `stop`; итог придёт событием `Done`.
pub struct Recorder {
    pub started: Instant,
    pub path: Option<PathBuf>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Recorder {
    pub fn start(opts: Opts, gen: u64, out: Sender<RecEvent>, wake: impl Fn() + Send + Sync + 'static) -> Result<Recorder, String> {
        if opts.video.is_none() && opts.audio.is_none() {
            return Err("не выбрано ни видео, ни звук".into());
        }
        let exe = ffmpeg()?;
        let path = match &opts.dir {
            Some(dir) => {
                std::fs::create_dir_all(dir).map_err(|e| format!("не удалось создать папку {}: {e}", dir.display()))?;
                Some(free_name(dir, if opts.video.is_some() { "mkv" } else { "mka" }))
            }
            None => None,
        };
        let preview = std::env::temp_dir().join(format!("presentator_rec_{}_{gen}.jpg", std::process::id()));
        let _ = std::fs::remove_file(&preview);
        let args = build_args(&opts, path.as_deref(), &preview);
        let mut child = ff(&exe).args(&args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("не удалось запустить FFmpeg: {e}"))?;
        // FFmpeg не должен пережить программу: при её сбое иначе писал бы до заполнения диска.
        kill_with_us(&child);
        let stdin = Arc::new(Mutex::new(child.stdin.take()));
        let stop = Arc::new(AtomicBool::new(false));
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let send = {
            let (out, wake) = (out.clone(), wake.clone());
            move |ev: SessionEv| {
                let _ = out.send(RecEvent::Session { gen, ev });
                wake();
            }
        };
        let send: Arc<dyn Fn(SessionEv) + Send + Sync> = Arc::new(send);

        // Ход записи: `-progress pipe:1` — строки «ключ=значение», блок раз в секунду.
        if let Some(stdout) = child.stdout.take() {
            let send = send.clone();
            std::thread::spawn(move || {
                let (mut secs, mut bytes, mut fps, mut dropped) = (0.0, 0u64, 0.0f32, 0u64);
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let Some((k, v)) = line.split_once('=') else { continue };
                    match k {
                        "out_time_us" => secs = v.trim().parse::<f64>().map(|u| u / 1e6).unwrap_or(secs),
                        "total_size" => bytes = v.trim().parse().unwrap_or(bytes),
                        "fps" => fps = v.trim().parse().unwrap_or(fps),
                        "drop_frames" => dropped = v.trim().parse().unwrap_or(dropped),
                        "progress" => send(SessionEv::Progress { secs: secs.max(0.0), bytes, fps, dropped }),
                        _ => {}
                    }
                }
            });
        }
        // Сообщения FFmpeg: уровень звука (фильтр ebur128, ~10 раз в секунду — шлём ~5) и ошибки.
        let errors = Arc::new(Mutex::new(Vec::<String>::new()));
        if let Some(stderr) = child.stderr.take() {
            let (send, errors) = (send.clone(), errors.clone());
            std::thread::spawn(move || {
                let mut last = Instant::now() - Duration::from_secs(1);
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if line.contains("Parsed_ebur128") {
                        if last.elapsed() >= Duration::from_millis(200) {
                            if let Some(m) = line.split("M:").nth(1).and_then(|s| s.split_whitespace().next()).and_then(|s| s.parse().ok()) {
                                last = Instant::now();
                                send(SessionEv::Level(m));
                            }
                        }
                        continue;
                    }
                    let mut e = errors.lock().unwrap_or_else(|e| e.into_inner());
                    e.push(line);
                    if e.len() > 40 {
                        e.remove(0);
                    }
                }
            });
        }
        // Превью: FFmpeg раз в полсекунды заменяет картинку (атомарно), мы её читаем.
        if opts.video.is_some() {
            let (send, stop, preview) = (send.clone(), stop.clone(), preview.clone());
            std::thread::spawn(move || {
                let mut seen = None;
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(500));
                    let modified = std::fs::metadata(&preview).and_then(|m| m.modified()).ok();
                    if modified.is_none() || modified == seen {
                        continue;
                    }
                    seen = modified;
                    let Ok(data) = std::fs::read(&preview) else { continue };
                    if let Ok(img) = image::load_from_memory_with_format(&data, image::ImageFormat::Jpeg) {
                        let rgba = img.to_rgba8();
                        let size = [rgba.width() as usize, rgba.height() as usize];
                        send(SessionEv::Preview(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())));
                    }
                }
            });
        }

        let thread = {
            let (stop, stdin, path) = (stop.clone(), stdin.clone(), path.clone());
            std::thread::spawn(move || {
                let code = wait_child(&mut child, &stop);
                stdin.lock().unwrap_or_else(|e| e.into_inner()).take();
                stop.store(true, Ordering::Relaxed);
                let _ = std::fs::remove_file(&preview);
                // Дать потоку сообщений дочитать последние строки.
                std::thread::sleep(Duration::from_millis(100));
                let tail = errors.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let result = finish(&exe, path, code, &tail, &*send);
                send(SessionEv::Done(result));
            })
        };
        Ok(Recorder { started: Instant::now(), path, stdin, stop, thread: Some(thread) })
    }

    /// Попросить FFmpeg закончить (как «q» в консоли): он допишет файл и выйдет.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(stdin) = self.stdin.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = stdin.write_all(b"q\n");
            let _ = stdin.flush();
        }
    }

    /// Остановить и дождаться конца (при выходе из программы).
    pub fn stop_and_wait(mut self) {
        self.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Ждём выхода FFmpeg; после «стоп» — не дольше `STOP_WAIT`, потом завершаем силой
/// (файл MKV при этом остаётся читаемым). `None` — завершён силой.
fn wait_child(child: &mut Child, stop: &AtomicBool) -> Option<i32> {
    let mut stop_at: Option<Instant> = None;
    loop {
        if let Ok(Some(st)) = child.try_wait() {
            return st.code();
        }
        if stop.load(Ordering::Relaxed) {
            let at = *stop_at.get_or_insert_with(Instant::now);
            if at.elapsed() > STOP_WAIT {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// После FFmpeg: проверить файл и перепаковать MKV → MP4 (MKA → M4A).
fn finish(exe: &Path, path: Option<PathBuf>, code: Option<i32>, tail: &[String], send: &(dyn Fn(SessionEv) + Send + Sync)) -> Result<Option<PathBuf>, String> {
    let Some(path) = path else {
        // Проверка сигнала: ошибка, только если FFmpeg не смог открыть устройства.
        return if code.is_some_and(|c| c != 0) { Err(explain(tail)) } else { Ok(None) };
    };
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if size < 1024 {
        let _ = std::fs::remove_file(&path);
        return Err(explain(tail));
    }
    send(SessionEv::Saving);
    let ext = if path.extension().is_some_and(|e| e == "mka") { "m4a" } else { "mp4" };
    let target = free_name_like(&path, ext);
    let ok = ff(exe)
        .args(["-loglevel", "error", "-y", "-i"])
        .arg(&path)
        .args(["-map", "0", "-c", "copy", "-movflags", "+faststart"])
        .arg(&target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
        && std::fs::metadata(&target).is_ok_and(|m| m.len() > 0);
    if ok {
        let _ = std::fs::remove_file(&path);
        Ok(Some(target))
    } else {
        // Не перепаковалось — оставляем MKV: он полноценный, просто другой формат.
        let _ = std::fs::remove_file(&target);
        Ok(Some(path))
    }
}

/// Понятная причина по последним сообщениям FFmpeg.
fn explain(tail: &[String]) -> String {
    let all = tail.join("\n");
    if all.contains("Could not run graph") || all.contains("Could not set video options") || all.contains("I/O error") {
        return "устройство занято другой программой (например, OBS) или не поддерживает выбранный формат".into();
    }
    if all.contains("Could not find") && all.contains("device") {
        return "устройство не найдено — отключено? Нажмите «Обновить»".into();
    }
    let last = tail.iter().rev().find(|l| !l.trim().is_empty() && !l.starts_with("  ")).cloned().unwrap_or_default();
    if last.is_empty() { "FFmpeg завершился с ошибкой".into() } else { format!("FFmpeg: {}", last.trim()) }
}

/// Аргументы FFmpeg: вход DirectShow, основной файл (если пишем), превью, уровень звука.
fn build_args(opts: &Opts, path: Option<&Path>, preview: &Path) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let mut push = |s: &[&str]| a.extend(s.iter().map(|s| s.to_string()));
    push(&["-loglevel", "info", "-f", "dshow", "-rtbufsize", "512M"]);
    if let Some(m) = opts.video.as_ref().and(opts.mode.as_ref()) {
        match (&m.vcodec, &m.pixfmt) {
            (Some(c), _) => push(&["-vcodec", c]),
            (None, Some(p)) => push(&["-pixel_format", p]),
            _ => {}
        }
        push(&["-video_size", &format!("{}x{}", m.w, m.h), "-framerate", &m.fps]);
    }
    if opts.audio.is_some() {
        // Маленький буфер звука — меньше расхождение звука и видео.
        push(&["-audio_buffer_size", "50"]);
    }
    let input = match (&opts.video, &opts.audio) {
        (Some(v), Some(au)) => format!("video={v}:audio={au}"),
        (Some(v), None) => format!("video={v}"),
        (None, Some(au)) => format!("audio={au}"),
        (None, None) => String::new(),
    };
    push(&["-i", &input]);
    let level = "ebur128=peak=none";
    match path {
        Some(path) => {
            let path = path.to_string_lossy().to_string();
            if opts.video.is_some() {
                push(&["-map", "0:v"]);
            }
            if opts.audio.is_some() {
                push(&["-map", "0:a", "-c:a", "aac", "-b:a", "192k", "-af", level]);
            }
            if opts.video.is_some() {
                let (w, h, fps) = opts.mode.as_ref().map_or((1920, 1080, 30.0), |m| (m.w, m.h, m.fps_f()));
                // ~0,13 бита на точку: 1080p30 — 8 Мбит/с.
                let mbit = ((w as f64 * h as f64 * fps * 0.13) / 1e6).clamp(3.0, 40.0).round();
                push(&["-c:v", &opts.encoder, "-b:v", &format!("{mbit}M"), "-maxrate", &format!("{}M", mbit * 1.5), "-bufsize", &format!("{}M", mbit * 2.0)]);
                // Без B-кадров: в MKV (метки времени с точностью до 1 мс) они дают повторы меток.
                push(&["-g", &format!("{}", (fps * 2.0).round() as u32), "-bf", "0"]);
            }
            // Данные — на диск сразу, кластеры по 1 с: при сбое программы теряется не больше секунды.
            push(&["-flush_packets", "1", "-cluster_time_limit", "1000", "-f", "matroska", &path]);
        }
        None => {
            // Проверка сигнала: звук только меряем, файла нет.
            if opts.audio.is_some() {
                push(&["-map", "0:a", "-af", level, "-f", "null", "-"]);
            }
        }
    }
    if opts.video.is_some() {
        let p = preview.to_string_lossy().to_string();
        push(&["-map", "0:v", "-vf", "fps=2,scale=640:-2", "-q:v", "5", "-update", "1", "-atomic_writing", "1", "-f", "image2", &p]);
    }
    push(&["-progress", "pipe:1", "-stats_period", "1"]);
    a
}

/// Имя файла по местному времени: «2026-10-12 14-30-05.mkv», без совпадений с готовыми файлами.
fn free_name(dir: &Path, ext: &str) -> PathBuf {
    let base = crate::watchdog::local_stamp().replace(':', "-");
    let mut n = 1;
    loop {
        let stem = if n == 1 { base.clone() } else { format!("{base} ({n})") };
        let taken = ["mkv", "mka", "mp4", "m4a"].iter().any(|e| dir.join(format!("{stem}.{e}")).exists());
        if !taken {
            return dir.join(format!("{stem}.{ext}"));
        }
        n += 1;
    }
}

fn free_name_like(path: &Path, ext: &str) -> PathBuf {
    let target = path.with_extension(ext);
    if !target.exists() {
        return target;
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    (2..).map(|n| dir.join(format!("{stem} ({n}).{ext}"))).find(|p| !p.exists()).unwrap_or(target)
}

/// Свободное место на диске с папкой (байт).
pub fn free_space(dir: &Path) -> Option<u64> {
    #[cfg(windows)]
    unsafe {
        use windows::core::HSTRING;
        use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        // Папки ещё может не быть — меряем по ближайшей существующей.
        let probe = dir.ancestors().find(|p| p.exists())?;
        let mut free = 0u64;
        GetDiskFreeSpaceExW(&HSTRING::from(probe.as_os_str()), Some(&mut free), None, None).ok()?;
        Some(free)
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        None
    }
}

/// Байт в секунду записи — для оценки «хватит на N часов».
pub fn bytes_per_sec(video: bool, mode: Option<&Mode>) -> f64 {
    let audio = 192_000.0 / 8.0;
    if !video {
        return audio;
    }
    let (w, h, fps) = mode.map_or((1920, 1080, 30.0), |m| (m.w, m.h, m.fps_f()));
    let mbit = ((w as f64 * h as f64 * fps * 0.13) / 1e6).clamp(3.0, 40.0).round();
    mbit * 1e6 / 8.0 + audio
}

/// Привязать процесс к «заданию», которое закрывается вместе с программой (и убивает его).
fn kill_with_us(child: &Child) {
    #[cfg(windows)]
    unsafe {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        static JOB: OnceLock<usize> = OnceLock::new();
        let job = *JOB.get_or_init(|| {
            let Ok(job) = CreateJobObjectW(None, None) else { return 0 };
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let _ = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            job.0 as usize
        });
        if job != 0 {
            let _ = AssignProcessToJobObject(HANDLE(job as _), HANDLE(child.as_raw_handle()));
        }
    }
    #[cfg(not(windows))]
    let _ = child;
}

/// Прочитать файл целиком, не больше `limit` байт (для проверки в тестах).
#[allow(dead_code)]
fn read_head(path: &Path, limit: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Ok(f) = std::fs::File::open(path) {
        let _ = f.take(limit as u64).read_to_end(&mut buf);
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_devices_and_modes() {
        let text = r#"[in#0 @ 000002c8] "USB Capture HDMI" (video)
[in#0 @ 000002c8]   Alternative name "@device_pnp_\\?\usb#vid"
[in#0 @ 000002c8] "Цифровой аудиоинтерфейс (USB Capture HDMI)" (audio)
[in#0 @ 000002c8] "Набор микрофонов" (audio)
[in#0 @ 000002c8] "Пустое" (none)"#;
        let (v, a) = parse_devices(text);
        assert_eq!(v, vec!["USB Capture HDMI"]);
        assert_eq!(a, vec!["Цифровой аудиоинтерфейс (USB Capture HDMI)", "Набор микрофонов"]);

        let text = "[in#0 @ 0]   pixel_format=yuyv422  min s=1920x1080 fps=5 max s=1920x1080 fps=5\n\
                    [in#0 @ 0]   vcodec=mjpeg  min s=1920x1080 fps=10 max s=1920x1080 fps=30\n\
                    [in#0 @ 0]   vcodec=mjpeg  min s=1280x720 fps=10 max s=1280x720 fps=60\n\
                    [in#0 @ 0]   vcodec=mjpeg  min s=1920x1080 fps=10 max s=1920x1080 fps=30";
        let m = parse_modes(text);
        assert_eq!(m.len(), 3);
        let best = best_mode(&m).unwrap();
        assert_eq!((best.w, best.h, best.fps.as_str(), best.vcodec.as_deref()), (1920, 1080, "30", Some("mjpeg")));
        assert_eq!(best.label(), "1920×1080, 30 к/с, MJPEG");
    }

    /// Устройства этого компьютера и 4 с записи с первых видео и звука:
    /// cargo test --release record_live -- --ignored --nocapture
    #[test]
    #[ignore]
    fn record_live() {
        let (v, a) = list_devices().unwrap();
        println!("видео: {v:?}\nзвук: {a:?}");
        // TEST_AUDIO_ONLY=1 — без видео.
        let v = if std::env::var("TEST_AUDIO_ONLY").is_ok() { Vec::new() } else { v };
        let modes = v.first().map(|d| list_modes(d).unwrap()).unwrap_or_default();
        let mode = best_mode(&modes).cloned();
        println!("формат: {:?}", mode.as_ref().map(Mode::label));
        let (enc, label) = encoder().clone().unwrap();
        println!("кодировщик: {enc} ({label})");
        let dir = std::env::temp_dir().join("presentator_rec_test");
        let (tx, rx) = std::sync::mpsc::channel();
        let opts = Opts { video: v.first().cloned(), mode, audio: a.first().cloned(), dir: Some(dir.clone()), encoder: enc };
        let r = Recorder::start(opts, 1, tx, || {}).unwrap();
        let t = Instant::now();
        let mut stopped = false;
        loop {
            match rx.recv_timeout(Duration::from_secs(30)).unwrap() {
                RecEvent::Session { ev: SessionEv::Progress { secs, bytes, fps, dropped }, .. } => println!("{:.1} с: {secs:.1} с, {bytes} байт, {fps} к/с, пропущено {dropped}", t.elapsed().as_secs_f32()),
                RecEvent::Session { ev: SessionEv::Level(l), .. } => print!("[{l}] "),
                RecEvent::Session { ev: SessionEv::Preview(p), .. } => println!("превью {}×{}", p.size[0], p.size[1]),
                RecEvent::Session { ev: SessionEv::Saving, .. } => println!("перепаковка…"),
                RecEvent::Session { ev: SessionEv::Done(r), .. } => {
                    let p = match r {
                        Ok(p) => p.unwrap(),
                        Err(e) => panic!("ОШИБКА ЗАПИСИ: {e}"),
                    };
                    println!("готово: {} ({} байт)", p.display(), std::fs::metadata(&p).unwrap().len());
                    assert!(read_head(&p, 12)[4..8] == *b"ftyp");
                    break;
                }
                _ => {}
            }
            if !stopped && t.elapsed() > Duration::from_secs(4) {
                r.stop();
                stopped = true;
            }
        }
    }
}
