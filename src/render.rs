//! Фоновая подготовка файлов (подсчёт страниц и слайдов) и многопоточный рендер страниц PDF.

use std::path::{Path, PathBuf};
use std::collections::HashSet;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};

use eframe::egui::ColorImage;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use pdfium_render::prelude::{PdfDocument, Pdfium, PdfRenderConfig};

/// Исполнитель PDF (сохранён в локальной конфигурации сервера показа).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PdfEngine {
    #[default]
    Pdfium,
    Hayro,
}

impl PdfEngine {
    pub fn from_config(value: &str) -> Self {
        if value.eq_ignore_ascii_case("hayro") { Self::Hayro } else { Self::Pdfium }
    }
    pub fn as_str(self) -> &'static str {
        match self { Self::Pdfium => "pdfium", Self::Hayro => "hayro" }
    }
    pub fn label(self) -> &'static str {
        match self { Self::Pdfium => "PDFium", Self::Hayro => "Hayro" }
    }
}


#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PageKey {
    pub id: u64,
    pub page: usize,
    pub w: u32,
    pub h: u32,
    /// Миниатюра для превью у оператора (готовится и в JPEG — для клиентов).
    pub thumb: bool,
    /// Какая часть страницы: (лево, верх, право, низ) в десятитысячных долях; `FULL_VIEW` — вся.
    /// При увеличении рендерится только видимый кусок — в разрешении экрана, поэтому чётко.
    pub view: [u16; 4],
}

/// Вся страница.
pub const FULL_VIEW: [u16; 4] = [0, 0, 10000, 10000];

/// Ступени увеличения слайда.
pub const ZOOMS: [f32; 7] = [1.0, 1.25, 1.5, 2.0, 2.5, 3.0, 4.0];

/// Видимая часть страницы при увеличении `zoom` вокруг центра `center` (доли страницы):
/// (лево, верх, право, низ) в долях страницы. `aspect` — ширина страницы к высоте, `screen` — размер
/// экрана. При 100 % — вся страница; центр сдвигается так, чтобы за край страницы не выходить.
pub fn viewport(aspect: f32, screen: (f32, f32), zoom: f32, center: (f32, f32)) -> [f32; 4] {
    let (sw, sh) = (screen.0.max(1.0), screen.1.max(1.0));
    let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { sw / sh };
    // Размер страницы на экране при 100 % (вписана целиком) и при увеличении.
    let (fw, fh) = if aspect > sw / sh { (sw, sw / aspect) } else { (sh * aspect, sh) };
    let (vw, vh) = ((sw / (fw * zoom)).min(1.0), (sh / (fh * zoom)).min(1.0));
    let cx = center.0.clamp(vw / 2.0, 1.0 - vw / 2.0);
    let cy = center.1.clamp(vh / 2.0, 1.0 - vh / 2.0);
    [cx - vw / 2.0, cy - vh / 2.0, cx + vw / 2.0, cy + vh / 2.0]
}

/// Видимая часть — в ключ страницы (десятитысячные доли).
pub fn view_key(v: [f32; 4]) -> [u16; 4] {
    v.map(|x| (x.clamp(0.0, 1.0) * 10000.0).round() as u16)
}

/// Размер миниатюры превью: страница вписывается в эту рамку.
pub const THUMB_W: u32 = 480;
pub const THUMB_H: u32 = 270;

pub enum Out {
    /// `linked` — у PPTX: видео и звук по ссылке на файлы, которых нет на этом компьютере.
    Prepared { id: u64, result: Result<(Option<PathBuf>, usize), String>, linked: Vec<String> },
    Ppt { gen: u64, ev: crate::ppt::PptEvent },
    /// Esc, нажатый в слайд-шоу PowerPoint (перехвачен до PowerPoint).
    ShowEsc,
    /// Свежие показатели этого компьютера (сборщик работает, пока запущен сервер).
    Health(crate::health::HealthInfo),
    Video { gen: u64, ev: crate::video::VideoEvent },
    /// Фон сцены загружен (номер загрузки — устаревшие отбрасываются).
    Background { gen: u64, result: Result<crate::background::Frames, String> },
    /// Картинка, открытая как презентация, загружена (номер показа — устаревшие отбрасываются).
    Image { gen: u64, result: Result<crate::background::Frames, String> },
    /// События плеера (музыка и вставки).
    Audio(crate::audio::AudioEvent),
    Page { key: PageKey, image: ColorImage },
    PageErr { key: PageKey, msg: String },
    /// Миниатюра слайда для превью: картинка и она же в JPEG (для клиентов).
    Thumb { id: u64, page: usize, image: ColorImage, jpg: Vec<u8> },
    /// Слайд PowerPoint крупной картинкой (для увеличения) прочитан с диска.
    PptZoom { gen: u64, page: usize, image: Result<ColorImage, String> },
    ThumbErr { id: u64, page: usize },
}

/// Расширения звуковых файлов — они идут в плеер, а не в список презентаций.
pub const AUDIO_EXTS: [&str; 12] = ["mp3", "wav", "flac", "ogg", "oga", "opus", "m4a", "aac", "wma", "aif", "aiff", "mka"];

/// Расширения видеофайлов, которые отдаём встроенному проигрывателю.
pub const VIDEO_EXTS: [&str; 12] = ["mp4", "m4v", "mov", "wmv", "avi", "mkv", "mpg", "mpeg", "ts", "m2ts", "webm", "3gp"];

fn load_pdf(path: &Path) -> Result<Pdf, String> {
    let data = std::fs::read(path).map_err(|e| format!("не удалось прочитать файл: {e}"))?;
    Pdf::new(data).map_err(|e| format!("не удалось открыть PDF: {e:?}"))
}

/// Готовит остальные форматы в отдельном потоке, PDFium-файлы обрабатывает
/// PDFium-поток (чтобы не обращаться к PDFium одновременно из разных потоков).
fn prepare_in_background(id: u64, src: PathBuf, out: Sender<Out>, wake: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(|| prepare_inner(&src))
            .unwrap_or_else(|_| Err("внутренняя ошибка при обработке файла".into()));
        let ext = src.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let linked = if result.is_ok() && (ext == "pptx" || ext == "ppsx") { linked_media(&src) } else { Vec::new() };
        let _ = out.send(Out::Prepared { id, result, linked });
        wake();
    });
}

/// Миниатюра слайда PPTX из картинки, которую сохранил мост к PowerPoint, — в отдельном потоке.
pub fn load_thumb(id: u64, page: usize, file: PathBuf, out: Sender<Out>, wake: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let loaded = std::fs::read(&file).ok().and_then(|jpg| Some((crate::net::decode_image(&jpg)?, jpg)));
        let _ = out.send(match loaded {
            Some((image, jpg)) => Out::Thumb { id, page, image, jpg },
            None => Out::ThumbErr { id, page },
        });
        wake();
    });
}

/// Картинка, открытая как презентация: загрузить в отдельном потоке (кадры — на сцену) и сделать
/// миниатюру для превью.
pub fn load_image(id: u64, gen: u64, path: PathBuf, max_side: u32, out: Sender<Out>, wake: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let result = crate::background::load_file(&path, max_side);
        let thumb = result.as_ref().ok().and_then(|f| f.first()).and_then(|f| thumb_of(&f.0));
        let _ = out.send(Out::Image { gen, result });
        if let Some((image, jpg)) = thumb {
            let _ = out.send(Out::Thumb { id, page: 0, image, jpg });
        }
        wake();
    });
}

/// Миниатюра (вписана в THUMB_W×THUMB_H) и она же в JPEG.
fn thumb_of(img: &ColorImage) -> Option<(ColorImage, Vec<u8>)> {
    let (w, h) = (img.size[0] as u32, img.size[1] as u32);
    let rgba = image::RgbaImage::from_raw(w, h, img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect())?;
    let k = (THUMB_W as f32 / w as f32).min(THUMB_H as f32 / h as f32).min(1.0);
    let (tw, th) = (((w as f32 * k) as u32).max(1), ((h as f32 * k) as u32).max(1));
    let small = image::imageops::resize(&rgba, tw, th, image::imageops::FilterType::Triangle);
    let thumb = ColorImage::from_rgba_unmultiplied([tw as usize, th as usize], small.as_raw());
    let jpg = to_jpeg(&thumb).ok()?;
    Some((thumb, jpg))
}

/// Изображение страницы → JPEG для отправки клиентам.
fn to_jpeg(image: &ColorImage) -> Result<Vec<u8>, String> {
    let rgb: Vec<u8> = image.pixels.iter().flat_map(|c| [c.r(), c.g(), c.b()]).collect();
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 82)
        .encode(&rgb, image.size[0] as u32, image.size[1] as u32, image::ExtendedColorType::Rgb8)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

fn prepare_inner(src: &Path) -> Result<(Option<PathBuf>, usize), String> {
    let ext = src.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "pdf" => {
            let pages = load_pdf(src)?.pages().len();
            if pages == 0 {
                return Err("в файле нет страниц".into());
            }
            Ok((Some(src.to_path_buf()), pages))
        }
        // PPTX показывает сам PowerPoint; здесь только считаем слайды для списка.
        "pptx" | "ppsx" => Ok((None, count_slides(src)?)),
        // Картинка — презентация из одного слайда; проверяем, что она читается.
        e if crate::background::EXTS.contains(&e) => {
            image::image_dimensions(src).map_err(|e| format!("не удалось прочитать картинку: {e}"))?;
            Ok((None, 1))
        }
        "ppt" | "pps" => {
            std::fs::File::open(src).map_err(|e| format!("не удалось прочитать файл: {e}"))?;
            Ok((None, 0))
        }
        // Видео открывает сам проигрыватель; здесь только проверяем, что файл читается.
        e if VIDEO_EXTS.contains(&e) => {
            std::fs::File::open(src).map_err(|e| format!("не удалось прочитать файл: {e}"))?;
            Ok((None, 0))
        }
        _ => Err("неподдерживаемый формат".into()),
    }
}

/// Видео и звук, вставленные в PPTX **ссылкой** на файл (а не внутрь презентации), которых нет на
/// этом компьютере: «имя файла (слайд N)», по порядку слайдов. На сервер уходит только сам PPTX —
/// такие видео на экране не появятся. Ссылки — связи слайда с `TargetMode="External"` и типом
/// video / audio / media; онлайн-видео (http) — не файл, их не считаем.
pub fn linked_media(src: &Path) -> Vec<String> {
    let Ok(f) = std::fs::File::open(src) else { return Vec::new() };
    let Ok(mut zip) = zip::ZipArchive::new(f) else { return Vec::new() };
    let rels: Vec<String> = zip.file_names().filter(|n| n.starts_with("ppt/slides/_rels/slide") && n.ends_with(".xml.rels")).map(String::from).collect();
    let mut found: Vec<(usize, String)> = Vec::new();
    for name in rels {
        let slide: usize = name.trim_start_matches("ppt/slides/_rels/slide").trim_end_matches(".xml.rels").parse().unwrap_or(0);
        let mut xml = String::new();
        let Ok(mut entry) = zip.by_name(&name) else { continue };
        if std::io::Read::read_to_string(&mut entry, &mut xml).is_err() {
            continue;
        }
        for rel in xml.split("<Relationship ").skip(1) {
            let attr = |a: &str| rel.split(&format!("{a}=\"")).nth(1).and_then(|s| s.split('"').next()).map(xml_unescape);
            let kind = attr("Type").unwrap_or_default();
            let external = attr("TargetMode").is_some_and(|m| m == "External");
            if !external || !(kind.ends_with("/video") || kind.ends_with("/audio") || kind.ends_with("/media")) {
                continue;
            }
            let target = attr("Target").unwrap_or_default();
            if target.starts_with("http://") || target.starts_with("https://") {
                continue;
            }
            let path = link_path(&target, src);
            if path.is_file() {
                continue;
            }
            let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(target);
            if !found.iter().any(|(s, f)| *s == slide && *f == file) {
                found.push((slide, file));
            }
        }
    }
    found.sort();
    found.into_iter().map(|(s, f)| format!("{f} (слайд {s})")).collect()
}

fn xml_unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'")
}

/// Цель ссылки → путь: `file:///D:/Видео/a%20b.mp4`, `D:\Видео\a.mp4` или путь относительно PPTX.
fn link_path(target: &str, pptx: &Path) -> PathBuf {
    let t = target.strip_prefix("file:///").or_else(|| target.strip_prefix("file://")).unwrap_or(target);
    // %XX → байты UTF-8.
    let bytes = t.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let p = PathBuf::from(String::from_utf8_lossy(&out).into_owned());
    let absolute = p.is_absolute() || t.as_bytes().get(1) == Some(&b':') || t.starts_with("\\\\");
    if absolute { p } else { pptx.parent().map_or(p.clone(), |d| d.join(&p)) }
}

/// PPTX — это zip-архив; слайды лежат в нём как ppt/slides/slideN.xml.
fn count_slides(src: &Path) -> Result<usize, String> {
    let f = std::fs::File::open(src).map_err(|e| format!("не удалось прочитать файл: {e}"))?;
    let zip = zip::ZipArchive::new(f).map_err(|_| "файл повреждён или это не PPTX".to_string())?;
    let n = zip
        .file_names()
        .filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml"))
        .count();
    if n == 0 {
        return Err("в презентации нет слайдов".into());
    }
    Ok(n)
}

/// Рендерит страницу так, чтобы она целиком вписалась в w×h пикселей.
pub fn render_page(pdf: &Pdf, key: PageKey) -> Result<ColorImage, String> {
    let pages = pdf.pages();
    let page = pages.get(key.page).ok_or("нет такой страницы")?;
    let (pw, ph) = page.render_dimensions();
    if pw <= 0.0 || ph <= 0.0 {
        return Err("страница нулевого размера".into());
    }
    if key.view != FULL_VIEW {
        return render_region(page, pw, ph, key);
    }
    let scale = (key.w as f32 / pw).min(key.h as f32 / ph).min(65000.0 / pw.max(ph)).max(0.01);
    let settings = PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE };
    let cache = RenderCache::new();
    let pix = hayro::render(page, &cache, &InterpreterSettings::default(), &RenderSettings::default(), &settings);
    let size = [pix.width() as usize, pix.height() as usize];
    Ok(ColorImage::from_rgba_premultiplied(size, pix.data_as_u8_slice()))
}

/// Кусок страницы (`key.view`), вписанный в `key.w`×`key.h`: только он и рендерится — в разрешении
/// экрана, поэтому при любом увеличении чётко, а время и память — как у обычной страницы.
fn render_region(page: &hayro::hayro_syntax::page::Page, pw: f32, ph: f32, key: PageKey) -> Result<ColorImage, String> {
    use hayro::hayro_interpret::util::TransformExt;
    use hayro::kurbo::Affine;
    use hayro::vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};
    let v = key.view.map(|x| x as f64 / 10000.0);
    let (rx, ry) = (v[0] * pw as f64, v[1] * ph as f64);
    let (rw, rh) = (((v[2] - v[0]) * pw as f64).max(1.0), ((v[3] - v[1]) * ph as f64).max(1.0));
    let s = (key.w as f64 / rw).min(key.h as f64 / rh);
    let (w, h) = ((rw * s).round().clamp(1.0, 16384.0) as u16, (rh * s).round().clamp(1.0, 16384.0) as u16);
    let transform = Affine::translate((-rx * s, -ry * s)) * Affine::scale(s) * page.initial_transform(true).to_kurbo();
    let cache = RenderCache::new();
    let mut ctx = RenderContext::new(w, h);
    hayro::render_into(page, &cache, &InterpreterSettings::default(), &RenderSettings::default(), &mut ctx, transform);
    ctx.flush();
    let mut pix = Pixmap::new(w, h);
    ctx.render_with(&mut pix, &mut Resources::default(), RasterizerSettings { target_init: TargetInit::Clear(WHITE), ..Default::default() });
    Ok(ColorImage::from_rgba_premultiplied([w as usize, h as usize], pix.data_as_u8_slice()))
}

/// Прочитать крупную картинку слайда PowerPoint (для увеличения) в отдельном потоке.
pub fn load_ppt_zoom(gen: u64, page: usize, path: PathBuf, out: Sender<Out>, wake: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let image = image::open(&path).map_err(|e| e.to_string()).map(|img| {
            let rgba = img.to_rgba8();
            ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw())
        });
        let _ = out.send(Out::PptZoom { gen, page, image });
        wake();
    });
}

/// Подготовка PDFium и рендер идут в одном выделенном потоке. Это важно:
/// глобальное состояние PDFium небезопасно для параллельных вызовов из разных
/// потоков. Для Hayro остаётся прежний пул до четырёх потоков.
pub struct Renderer {
    shared: Arc<Shared>,
    pdfium_ready: bool,
    pdfium_error: Option<String>,
}

struct Shared {
    queue: Mutex<Queue>,
    cv: Condvar,
    /// Открытый документ — один на все потоки Hayro.
    hayro_doc: Mutex<Option<(PathBuf, Arc<Pdf>)>>,
}

struct Queue {
    pdf: Option<PathBuf>,
    jobs: Vec<PageKey>,
    busy: HashSet<(u64, PageKey)>,
    /// Версия очереди: прежние результаты после переключения PDF/движка не показываются.
    epoch: u64,
    engine: PdfEngine,
    pdfium_prepares: Vec<(u64, PathBuf)>,
}

impl Default for Queue {
    fn default() -> Self {
        Self {
            pdf: None, jobs: Vec::new(), busy: HashSet::new(),
            epoch: 0, engine: PdfEngine::Hayro, pdfium_prepares: Vec::new(),
        }
    }
}

impl Renderer {
    pub fn new(out: Sender<Out>, wake: impl Fn() + Send + Sync + 'static, preferred: &str) -> Renderer {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()), cv: Condvar::new(),
            hayro_doc: Mutex::new(None),
        });
        let wake = Arc::new(wake);
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(1, 4);
        for _ in 0..workers {
            let (shared, out, wake) = (shared.clone(), out.clone(), wake.clone());
            std::thread::spawn(move || hayro_worker(&shared, &out, &*wake));
        }

        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        {
            let (shared, out, wake) = (shared.clone(), out.clone(), wake.clone());
            std::thread::spawn(move || pdfium_worker(&shared, &out, &*wake, ready_tx));
        }
        // PDFium загружается только в PDFium-потоке, один раз за сеанс.
        let availability = ready_rx.recv().unwrap_or_else(|_| Err("поток PDFium не запустился".into()));
        let pdfium_error = availability.err();
        let pdfium_ready = pdfium_error.is_none();
        {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            if pdfium_ready && PdfEngine::from_config(preferred) == PdfEngine::Pdfium {
                q.engine = PdfEngine::Pdfium;
            }
        }
        Renderer { shared, pdfium_ready, pdfium_error }
    }

    pub fn engine(&self) -> PdfEngine {
        self.shared.queue.lock().unwrap_or_else(|e| e.into_inner()).engine
    }

    pub fn pdfium_error(&self) -> Option<&str> {
        self.pdfium_error.as_deref()
    }

    pub fn set_engine(&self, engine: PdfEngine) -> Result<(), String> {
        if engine == PdfEngine::Pdfium && !self.pdfium_ready {
            return Err(format!("PDFium недоступен: {}", self.pdfium_error.as_deref().unwrap_or("библиотека не загружена")));
        }
        let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.engine != engine {
            q.engine = engine;
            q.epoch = q.epoch.wrapping_add(1);
            q.jobs.clear();
            self.shared.cv.notify_all();
        }
        Ok(())
    }

    /// Подготовка файлов для списка. Для PDF используется тот же движок, что и
    /// для показа: Hayro может не распознать PDF, который PDFium умеет открыть.
    pub fn prepare(&self, id: u64, src: PathBuf, out: Sender<Out>, wake: impl Fn() + Send + 'static) {
        let pdf = src.extension().is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("pdf"));
        if pdf && self.engine() == PdfEngine::Pdfium {
            let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            q.pdfium_prepares.push((id, src));
            self.shared.cv.notify_all();
        } else {
            prepare_in_background(id, src, out, wake);
        }
    }

    /// Какие страницы нужны сейчас, по убыванию важности.
    pub fn want(&self, pdf: &Path, keys: Vec<PageKey>) {
        let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.pdf.as_deref() != Some(pdf) {
            q.epoch = q.epoch.wrapping_add(1);
            q.pdf = Some(pdf.to_path_buf());
        }
        let jobs: Vec<PageKey> = keys.into_iter().filter(|k| !q.busy.contains(&(q.epoch, *k))).collect();
        if q.jobs == jobs { return; }
        q.jobs = jobs;
        if !q.jobs.is_empty() { self.shared.cv.notify_all(); }
    }

    pub fn cancel(&self) {
        let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        q.epoch = q.epoch.wrapping_add(1);
        q.jobs.clear();
        q.pdf = None;
    }
}

fn next_job(shared: &Shared, engine: PdfEngine) -> (PageKey, PathBuf, u64) {
    let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
    while q.jobs.is_empty() || q.engine != engine {
        q = shared.cv.wait(q).unwrap_or_else(|e| e.into_inner());
    }
    let key = q.jobs.remove(0);
    let epoch = q.epoch;
    q.busy.insert((epoch, key));
    (key, q.pdf.clone().unwrap_or_default(), epoch)
}

fn deliver(shared: &Shared, out: &Sender<Out>, wake: &(dyn Fn() + Send + Sync),
           key: PageKey, path: &Path, epoch: u64, result: Result<ColorImage, String>) {
    let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
    q.busy.remove(&(epoch, key));
    let current = q.epoch == epoch && q.pdf.as_deref() == Some(path);
    drop(q);
    if !current { return; }
    let (id, page) = (key.id, key.page);
    let ev = match result {
        Ok(image) if key.thumb => match to_jpeg(&image) {
            Ok(jpg) => Out::Thumb { id, page, image, jpg },
            Err(_) => Out::ThumbErr { id, page },
        },
        Ok(image) => Out::Page { key, image },
        Err(_) if key.thumb => Out::ThumbErr { id, page },
        Err(msg) => Out::PageErr { key, msg },
    };
    let _ = out.send(ev);
    wake();
}

fn hayro_worker(shared: &Shared, out: &Sender<Out>, wake: &(dyn Fn() + Send + Sync)) {
    loop {
        let (key, path, epoch) = next_job(shared, PdfEngine::Hayro);
        let doc = {
            let mut d = shared.hayro_doc.lock().unwrap_or_else(|e| e.into_inner());
            match d.as_ref().filter(|(p, _)| *p == path) {
                Some((_, pdf)) => Ok(pdf.clone()),
                None => load_pdf(&path).map(|pdf| {
                    let pdf = Arc::new(pdf);
                    *d = Some((path.clone(), pdf.clone()));
                    pdf
                }),
            }
        };
        let res = doc.and_then(|pdf| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render_page(&pdf, key)))
                .unwrap_or_else(|_| Err("ошибка Hayro при отрисовке страницы".into()))
        });
        deliver(shared, out, wake, key, &path, epoch, res);
    }
}

/// PDFium используется только отсюда — его документ остаётся открытым между
/// слайдами, чтобы дорогие PDF не разбирались заново при каждом перелистывании.
fn pdfium_worker(shared: &Shared, out: &Sender<Out>, wake: &(dyn Fn() + Send + Sync),
                 ready: std::sync::mpsc::SyncSender<Result<(), String>>) {
    // Windows: PDFium поставляется ВНУТРИ presentator.exe; никакие DLL возле
    // программы не нужны. Как и libmpv, он извлекается в служебный каталог
    // LOCALAPPDATA, а загружается по полному пути. Системные DLL не ищем:
    // это исключает случайную загрузку другой несовместимой версии PDFium.
    #[cfg(target_os = "windows")]
    let bindings = crate::pdfium_bundle::extract_pdfium()
        .and_then(|path| Pdfium::bind_to_library(&path)
            .map_err(|e| format!("не удалось загрузить встроенный PDFium: {e:?}")));
    #[cfg(not(target_os = "windows"))]
    let bindings = Pdfium::bind_to_system_library()
        .map_err(|e| format!("PDFium не найден: {e:?}"));
    let bindings = match bindings {
        Ok(bindings) => bindings,
        Err(e) => {
            let _ = ready.send(Err(format!("{e:?}")));
            return;
        }
    };
    let pdfium = Pdfium::new(bindings);
    let _ = ready.send(Ok(()));
    let mut cached: Option<(PathBuf, PdfDocument<'_>)> = None;
    loop {
        enum Work { Prepare(u64, PathBuf), Page(PageKey, PathBuf, u64) }
        let task = {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                // Показ важнее предварительного подсчёта страниц других файлов.
                if !q.jobs.is_empty() && q.engine == PdfEngine::Pdfium {
                    let key = q.jobs.remove(0);
                    let epoch = q.epoch;
                    q.busy.insert((epoch, key));
                    break Work::Page(key, q.pdf.clone().unwrap_or_default(), epoch);
                }
                if !q.pdfium_prepares.is_empty() {
                    let (id, path) = q.pdfium_prepares.remove(0);
                    break Work::Prepare(id, path);
                }
                q = shared.cv.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        let path = match &task { Work::Prepare(_, p) | Work::Page(_, p, _) => p.clone() };
        if cached.as_ref().is_none_or(|(p, _)| p != &path) {
            cached = None; // Сначала закрываем прошлый документ, освобождая память.
            if let Ok(doc) = pdfium.load_pdf_from_file(&path, None) {
                cached = Some((path.clone(), doc));
            }
        }
        let opened = cached.as_ref().filter(|(p, _)| p == &path).map(|(_, d)| d);
        match task {
            Work::Prepare(id, _) => {
                let result = opened.ok_or_else(|| "PDFium не смог открыть документ".to_string())
                    .and_then(|doc| {
                        let pages = doc.pages().len() as usize;
                        if pages == 0 { Err("в PDF нет страниц".into()) }
                        else { Ok((Some(path.clone()), pages)) }
                    });
                let _ = out.send(Out::Prepared { id, result, linked: Vec::new() });
                wake();
            }
            Work::Page(key, _, epoch) => {
                let result = opened.ok_or_else(|| "PDFium не смог открыть документ".to_string())
                    .and_then(|doc| {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render_pdfium_page(doc, key)))
                            .unwrap_or_else(|_| Err("ошибка PDFium при отрисовке страницы".into()))
                    });
                deliver(shared, out, wake, key, &path, epoch, result);
            }
        }
    }
}

/// Одна страница или её видимый фрагмент, без огромных промежуточных битмапов.
/// Для увеличения используем set_origin() и маленький конечный битмап.
fn render_pdfium_page(doc: &PdfDocument<'_>, key: PageKey) -> Result<ColorImage, String> {
    let index = i32::try_from(key.page).map_err(|_| "номер страницы слишком большой")?;
    let page = doc.pages().get(index).map_err(|e| format!("нет такой страницы: {e:?}"))?;
    let (pw, ph) = (page.width().value as f64, page.height().value as f64);
    if !pw.is_finite() || !ph.is_finite() || pw <= 0.0 || ph <= 0.0 {
        return Err("страница PDF нулевого размера".into());
    }
    let v = key.view.map(|n| n as f64 / 10000.0);
    let (rx, ry) = (v[0] * pw, v[1] * ph);
    let (rw, rh) = ((v[2] - v[0]) * pw, (v[3] - v[1]) * ph);
    if rw <= 0.0 || rh <= 0.0 || key.w == 0 || key.h == 0 {
        return Err("неверная видимая часть PDF".into());
    }
    let scale = (key.w as f64 / rw).min(key.h as f64 / rh);
    // Ограничение памяти и размера PdfiumBitmap при аномальных PDF.
    let (w, h) = ((rw * scale).round().clamp(1.0, 8192.0) as i32,
                  (rh * scale).round().clamp(1.0, 8192.0) as i32);
    if (w as u64) * (h as u64) > 33_554_432 {
        return Err("страница слишком велика для рендера (лимит 32 Мпикс)".into());
    }
    // Важно: масштаб вычисляем после округления пикселей, чтобы края совпадали.
    let actual_scale = (w as f64 / rw).min(h as f64 / rh) as f32;
    let config = PdfRenderConfig::new()
        .scale_page_by_factor(actual_scale)
        .set_fixed_size(w, h)
        .set_origin(-(rx * actual_scale as f64).round() as i32,
                    -(ry * actual_scale as f64).round() as i32);
    let bitmap = page.render_with_config(&config)
        .map_err(|e| format!("ошибка PDFium при отрисовке страницы: {e:?}"))?;
    let width = bitmap.width() as usize;
    let height = bitmap.height() as usize;
    let rgba = bitmap.as_rgba_bytes();
    if rgba.len() != width.saturating_mul(height).saturating_mul(4) {
        return Err("PDFium вернул буфер неожиданного размера".into());
    }
    Ok(ColorImage::from_rgba_unmultiplied([width, height], &rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pptx_slide_count_ignores_related_files_and_notes() {
        use std::io::Write;
        let path = std::env::temp_dir().join(format!("presentator-slides-{}.pptx", crate::net::unique_prefix()));
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            for name in [
                "ppt/slides/slide1.xml",
                "ppt/slides/slide2.xml",
                "ppt/slides/_rels/slide1.xml.rels",
                "ppt/notesSlides/slide3.xml",
                "docProps/app.xml",
            ] {
                zip.start_file(name, opts).unwrap();
                zip.write_all(b"<xml/>").unwrap();
            }
            zip.finish().unwrap();
        }
        let result = count_slides(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(result.unwrap(), 2);
    }

    #[test]
    fn pptx_slide_count_reports_invalid_archives() {
        let path = std::env::temp_dir().join(format!("presentator-bad-{}.pptx", crate::net::unique_prefix()));
        std::fs::write(&path, b"not a zip").unwrap();
        let result = count_slides(&path);
        let _ = std::fs::remove_file(&path);
        assert!(result.is_err());
    }

    #[test]
    fn media_link_path_decodes_percent_encoding() {
        let pptx = Path::new("C:/presentations/session.pptx");
        let target = link_path("../media/clip%20one.mp4", pptx);
        assert_eq!(target.file_name().unwrap().to_string_lossy(), "clip one.mp4");
        let target = link_path("file:///D:/Media/a%20b.mov", pptx);
        assert_eq!(target.file_name().unwrap().to_string_lossy(), "a b.mov");
    }

    #[test]
    fn pdf_engine_preference() {
        assert_eq!(PdfEngine::from_config("pdfium"), PdfEngine::Pdfium);
        assert_eq!(PdfEngine::from_config(""), PdfEngine::Pdfium);
        assert_eq!(PdfEngine::from_config("Hayro"), PdfEngine::Hayro);
        assert_eq!(PdfEngine::Hayro.as_str(), "hayro");
    }

    /// Проверка PDFium на Windows со встроенной в EXE библиотекой:
    /// $env:TEST_PDF="C:\\temp\\complex.pdf"
    /// cargo test --release pdfium_smoke -- --ignored --nocapture
    #[test]
    #[ignore]
    fn pdfium_smoke() {
        let file = PathBuf::from(std::env::var("TEST_PDF").expect("TEST_PDF = путь к PDF"));
        let (tx, rx) = std::sync::mpsc::channel();
        let renderer = Renderer::new(tx.clone(), || {}, "pdfium");
        assert_eq!(renderer.engine(), PdfEngine::Pdfium, "{:?}", renderer.pdfium_error());
        renderer.prepare(100, file.clone(), tx, || {});
        let count = match rx.recv_timeout(std::time::Duration::from_secs(60)).expect("PDF prepare timeout") {
            Out::Prepared { result: Ok((_, pages)), .. } => pages,
            Out::Prepared { result: Err(e), .. } => panic!("PDFium prepare: {e}"),
            _ => panic!("неожиданное событие при подготовке PDF"),
        };
        assert!(count > 0);
        renderer.want(&file, vec![PageKey { id: 100, page: 0, w: 1920, h: 1080, thumb: false, view: FULL_VIEW }]);
        match rx.recv_timeout(std::time::Duration::from_secs(90)).expect("PDF render timeout") {
            Out::Page { image, .. } => assert!(image.size[0] > 0 && image.size[1] > 0),
            Out::PageErr { msg, .. } => panic!("PDFium render: {msg}"),
            _ => panic!("неожиданное событие при отрисовке PDF"),
        }
    }

    /// Видео по ссылке: находится только ссылка на отсутствующий файл (не онлайн-видео, не вставленное
    /// внутрь, не существующий файл); %20 в пути раскодируется.
    #[test]
    fn finds_linked_media() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("presentator_linked_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let present = dir.join("есть.mp4");
        std::fs::write(&present, b"x").unwrap();
        let rels = format!(
            r#"<?xml version="1.0"?><Relationships>
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/video" Target="file:///C:/нет/ролик%201.mp4" TargetMode="External"/>
<Relationship Id="rId2" Type="http://schemas.microsoft.com/office/2007/relationships/media" Target="file:///C:/нет/ролик%201.mp4" TargetMode="External"/>
<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/video" Target="file:///{}" TargetMode="External"/>
<Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/video" Target="https://www.youtube.com/embed/x" TargetMode="External"/>
<Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/video" Target="../media/media1.mp4"/>
</Relationships>"#,
            present.display().to_string().replace('\\', "/")
        );
        let pptx = dir.join("t.pptx");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&pptx).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        z.start_file("ppt/slides/_rels/slide3.xml.rels", opts).unwrap();
        z.write_all(rels.as_bytes()).unwrap();
        z.start_file("ppt/slides/slide3.xml", opts).unwrap();
        z.write_all(b"<p/>").unwrap();
        z.finish().unwrap();
        assert_eq!(linked_media(&pptx), vec!["ролик 1.mp4 (слайд 3)".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
    use std::time::Instant;

    /// Только рендер, без окна и текстур: чередуем два PDF, как при переключении презентаций.
    /// TEST_PDF="a.pdf;b.pdf" TEST_ROUNDS=60 cargo test --release soak_render -- --ignored --nocapture
    #[test]
    #[ignore]
    fn soak_render() {
        let files: Vec<PathBuf> = std::env::var("TEST_PDF").unwrap().split(';').map(PathBuf::from).collect();
        let rounds: usize = std::env::var("TEST_ROUNDS").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
        let (tx, rx) = std::sync::mpsc::channel();
        let r = Renderer::new(tx, || {}, "hayro");
        for i in 0..rounds {
            let pdf = &files[i % files.len()];
            let keys: Vec<PageKey> = (0..5).map(|page| PageKey { id: i as u64, page, w: 1920, h: 1080, thumb: false, view: FULL_VIEW }).collect();
            r.want(pdf, keys);
            for _ in 0..5 {
                rx.recv().unwrap();
            }
            println!("ROUND {i}");
        }
    }

    #[test]
    fn viewport_math() {
        let eq = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4);
        // 100 % — вся страница, центр не важен.
        assert!(eq(viewport(16.0 / 9.0, (1920.0, 1080.0), 1.0, (0.9, 0.1)), [0.0, 0.0, 1.0, 1.0]));
        // 16:9 на 16:9, 200 % — половина по каждой оси, в центре.
        assert!(eq(viewport(16.0 / 9.0, (1920.0, 1080.0), 2.0, (0.5, 0.5)), [0.25, 0.25, 0.75, 0.75]));
        // Центр у самого края — видимая часть прижимается к краю, а не выходит за него.
        assert!(eq(viewport(16.0 / 9.0, (1920.0, 1080.0), 2.0, (0.0, 1.0)), [0.0, 0.5, 0.5, 1.0]));
        // Страница 4:3 на экране 16:9 при 125 %: по высоте видна часть, по ширине — вся (поля по бокам).
        let v = viewport(4.0 / 3.0, (1920.0, 1080.0), 1.25, (0.5, 0.5));
        assert!(eq(v, [0.0, 0.1, 1.0, 0.9]), "{v:?}");
    }

    /// Увеличенный кусок совпадает с тем же местом страницы, отрендеренной целиком крупнее:
    /// TEST_PDF="a.pdf" cargo test --release zoom_region_matches -- --ignored --nocapture
    #[test]
    #[ignore]
    fn zoom_region_matches() {
        let pdf = load_pdf(Path::new(&std::env::var("TEST_PDF").unwrap())).unwrap();
        let (w, h) = (960u32, 540u32);
        let fit = render_page(&pdf, PageKey { id: 0, page: 0, w, h, thumb: false, view: FULL_VIEW }).unwrap();
        let aspect = fit.size[0] as f32 / fit.size[1] as f32;
        let v = viewport(aspect, (w as f32, h as f32), 3.0, (0.3, 0.6));
        let region = render_page(&pdf, PageKey { id: 0, page: 0, w, h, thumb: false, view: view_key(v) }).unwrap();
        let big = render_page(&pdf, PageKey { id: 0, page: 0, w: w * 3, h: h * 3, thumb: false, view: FULL_VIEW }).unwrap();
        println!("страница {}×{}, кусок {:?} → {}×{}, крупная {}×{}", fit.size[0], fit.size[1], v, region.size[0], region.size[1], big.size[0], big.size[1]);
        // Сравниваем по сетке точек: кусок против того же места крупной страницы.
        let (mut diff, mut n, mut inked) = (0u64, 0u64, 0u32);
        for gy in 1..40 {
            for gx in 1..40 {
                let (rx, ry) = (region.size[0] * gx / 40, region.size[1] * gy / 40);
                let bx = ((v[0] + (v[2] - v[0]) * gx as f32 / 40.0) * big.size[0] as f32) as usize;
                let by = ((v[1] + (v[3] - v[1]) * gy as f32 / 40.0) * big.size[1] as f32) as usize;
                let a = region.pixels[ry * region.size[0] + rx];
                let b = big.pixels[by.min(big.size[1] - 1) * big.size[0] + bx.min(big.size[0] - 1)];
                inked += (a.r() < 240 || a.g() < 240 || a.b() < 240) as u32;
                diff += (a.r() as i32 - b.r() as i32).unsigned_abs() as u64 + (a.g() as i32 - b.g() as i32).unsigned_abs() as u64 + (a.b() as i32 - b.b() as i32).unsigned_abs() as u64;
                n += 3;
            }
        }
        let mean = diff as f64 / n as f64;
        println!("средняя разница цвета: {mean:.1} из 255; точек не на белом: {inked} из {}", n / 3);
        assert!(mean < 12.0, "кусок не совпадает со страницей: {mean}");
    }

    /// Время рендера каждой страницы под 1920×1080 (или TEST_SIZE):
    /// TEST_PDF="a.pdf" cargo test --release bench_render -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_render() {
        let path = PathBuf::from(std::env::var("TEST_PDF").unwrap());
        let t = Instant::now();
        let pdf = load_pdf(&path).unwrap();
        println!("загрузка: {} мс", t.elapsed().as_millis());
        // TEST_SIZE="3840x2160" — другой размер экрана.
        let size = std::env::var("TEST_SIZE").unwrap_or_else(|_| "1920x1080".into());
        let (w, h) = size.split_once('x').map(|(w, h)| (w.parse().unwrap(), h.parse().unwrap())).unwrap();
        for page in 0..pdf.pages().len() {
            let t = Instant::now();
            render_page(&pdf, PageKey { id: 0, page, w, h, thumb: false, view: FULL_VIEW }).unwrap();
            let full = t.elapsed().as_millis();
            // Увеличение 300 % — кусок в центре.
            let tz = Instant::now();
            let view = view_key(viewport(16.0 / 9.0, (w as f32, h as f32), 3.0, (0.5, 0.5)));
            let img = render_page(&pdf, PageKey { id: 0, page, w, h, thumb: false, view }).unwrap();
            println!("  целиком {full} мс; 300 %: {} мс, {}×{}", tz.elapsed().as_millis(), img.size[0], img.size[1]);
            println!("страница {}: {} мс", page + 1, t.elapsed().as_millis());
        }
    }
}

// Stage 8: preparation, image decoding, caching, and queue event delivery.
// These tests drive real file formats using isolated temporary files, without native windows.
#[cfg(test)]
mod stage8_tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("presentator_stage8_render_{}", crate::net::unique_prefix()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn file(&self, name: &str) -> PathBuf { self.0.join(name) }
    }
    impl Drop for Scratch {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    fn small_img() -> ColorImage {
        ColorImage::from_rgba_unmultiplied([2, 2], &[
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ])
    }

    fn test_key(thumb: bool) -> PageKey {
        PageKey { id: 2, page: 3, w: 12, h: 8, thumb, view: FULL_VIEW }
    }

    #[test]
    fn stage8_prepare_image_reports_one_page() {
        let scratch = Scratch::new();
        let path = scratch.file("slide.png");
        std::fs::write(&path, include_bytes!("../assets/icon_256.png")).unwrap();
        let (pdf, pages) = prepare_inner(&path).unwrap();
        assert!(pdf.is_none());
        assert_eq!(pages, 1);
    }

    #[test]
    fn stage8_prepare_image_invalid_pixels_are_rejected() {
        let scratch = Scratch::new();
        let path = scratch.file("broken.png");
        std::fs::write(&path, b"not an image").unwrap();
        assert!(prepare_inner(&path).is_err());
    }

    #[test]
    fn stage8_prepare_ppt_and_video_only_need_readable_files() {
        let scratch = Scratch::new();
        for name in ["legacy.ppt", "legacy.pps", "movie.mp4", "movie.webm"] {
            let path = scratch.file(name);
            std::fs::write(&path, b"fixture").unwrap();
            assert!(matches!(prepare_inner(&path), Ok((None, 0))), "{name}");
        }
    }

    #[test]
    fn stage8_prepare_missing_video_and_unknown_extension_rejected() {
        let scratch = Scratch::new();
        assert!(prepare_inner(&scratch.file("missing.mp4")).is_err());
        assert!(prepare_inner(&scratch.file("unsupported.xyz")).is_err());
    }

    #[test]
    fn stage8_prepare_invalid_pptx_and_pdf_report_errors() {
        let scratch = Scratch::new();
        for name in ["invalid.pptx", "invalid.pdf"] {
            let path = scratch.file(name);
            std::fs::write(&path, b"not a valid document").unwrap();
            assert!(prepare_inner(&path).is_err(), "{name}");
        }
    }

    #[test]
    fn stage8_background_preparation_reports_result_from_thread() {
        let scratch = Scratch::new();
        let path = scratch.file("unreadable.png");
        let (tx, rx) = mpsc::channel();
        prepare_in_background(47, path, tx, || {});
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Out::Prepared { id: 47, result: Err(_), linked } => assert!(linked.is_empty()),
            _ => panic!("expected preparation error"),
        }
    }

    #[test]
    fn stage8_background_preparation_valid_image_returns_ready_page() {
        let scratch = Scratch::new();
        let path = scratch.file("valid.png");
        std::fs::write(&path, include_bytes!("../assets/icon_256.png")).unwrap();
        let (tx, rx) = mpsc::channel();
        prepare_in_background(48, path, tx, || {});
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Out::Prepared { id: 48, result: Ok((None, 1)), linked } => assert!(linked.is_empty()),
            _ => panic!("expected ready image"),
        }
    }

    #[test]
    fn stage8_load_image_returns_frames_and_thumbnail() {
        let scratch = Scratch::new();
        let path = scratch.file("photo.png");
        std::fs::write(&path, include_bytes!("../assets/icon_256.png")).unwrap();
        let (tx, rx) = mpsc::channel();
        load_image(81, 11, path, 1024, tx, || {});
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Out::Image { gen: 11, result: Ok(frames) } => assert_eq!(frames.len(), 1),
            _ => panic!("expected decoded image"),
        }
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Out::Thumb { id: 81, page: 0, image, jpg } => {
                assert!(!jpg.is_empty());
                assert!(image.size[0] <= THUMB_W as usize);
                assert!(image.size[1] <= THUMB_H as usize);
            }
            _ => panic!("expected image thumbnail"),
        }
    }

    #[test]
    fn stage8_load_image_failure_emits_no_thumbnail() {
        let scratch = Scratch::new();
        let (tx, rx) = mpsc::channel();
        load_image(81, 12, scratch.file("missing.png"), 1024, tx, || {});
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)), Ok(Out::Image { gen: 12, result: Err(_), .. })));
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn stage8_load_thumb_supports_valid_bytes_without_extension_assumption() {
        let scratch = Scratch::new();
        let path = scratch.file("slide.jpg");
        std::fs::write(&path, include_bytes!("../assets/icon_256.png")).unwrap();
        let (tx, rx) = mpsc::channel();
        load_thumb(17, 5, path, tx, || {});
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Out::Thumb { id: 17, page: 5, image, jpg } => {
                assert_eq!(image.size, [256, 256]);
                assert!(!jpg.is_empty());
            }
            _ => panic!("expected valid thumbnail"),
        }
    }

    #[test]
    fn stage8_load_thumb_reports_error_for_missing_file() {
        let scratch = Scratch::new();
        let (tx, rx) = mpsc::channel();
        load_thumb(17, 4, scratch.file("missing.jpg"), tx, || {});
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)), Ok(Out::ThumbErr { id: 17, page: 4 })));
    }

    #[test]
    fn stage8_load_ppt_zoom_can_decode_png_image() {
        let scratch = Scratch::new();
        let path = scratch.file("zoom.png");
        std::fs::write(&path, include_bytes!("../assets/icon_256.png")).unwrap();
        let (tx, rx) = mpsc::channel();
        load_ppt_zoom(18, 3, path, tx, || {});
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Out::PptZoom { gen: 18, page: 3, image: Ok(img) } => assert_eq!(img.size, [256, 256]),
            _ => panic!("expected decoded zoom image"),
        }
    }

    #[test]
    fn stage8_load_ppt_zoom_reports_missing_file() {
        let scratch = Scratch::new();
        let (tx, rx) = mpsc::channel();
        load_ppt_zoom(19, 1, scratch.file("missing.jpg"), tx, || {});
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)), Ok(Out::PptZoom { gen: 19, page: 1, image: Err(_) })));
    }

    #[test]
    fn stage8_thumbnail_encoding_and_resize_preserve_dimensions() {
        let thumb = thumb_of(&small_img()).unwrap();
        assert_eq!(thumb.0.size, [2, 2]);
        assert!(!thumb.1.is_empty());
        assert!(to_jpeg(&small_img()).unwrap().len() > 10);
    }

    #[test]
    fn stage8_thumbnail_large_image_is_downscaled() {
        let img = ColorImage::from_rgba_unmultiplied([1000, 500], &vec![255; 1000 * 500 * 4]);
        let (thumb, jpg) = thumb_of(&img).unwrap();
        assert!(thumb.size[0] <= THUMB_W as usize);
        assert!(thumb.size[1] <= THUMB_H as usize);
        assert!(!jpg.is_empty());
    }

    #[test]
    fn stage8_deliver_page_event_only_for_current_epoch() {
        let shared = Shared { queue: Mutex::new(Queue::default()), cv: Condvar::new(), hayro_doc: Mutex::new(None) };
        let path = PathBuf::from("stage8.pdf");
        {
            let mut q = shared.queue.lock().unwrap();
            q.epoch = 5;
            q.pdf = Some(path.clone());
        }
        let (tx, rx) = mpsc::channel();
        deliver(&shared, &tx, &|| {}, test_key(false), &path, 4, Ok(small_img()));
        assert!(rx.try_recv().is_err());
        deliver(&shared, &tx, &|| {}, test_key(false), &path, 5, Ok(small_img()));
        assert!(matches!(rx.try_recv(), Ok(Out::Page { key, .. }) if key.page == 3));
    }

    #[test]
    fn stage8_deliver_page_failure_keeps_key_and_message() {
        let shared = Shared { queue: Mutex::new(Queue::default()), cv: Condvar::new(), hayro_doc: Mutex::new(None) };
        let path = PathBuf::from("stage8.pdf");
        shared.queue.lock().unwrap().pdf = Some(path.clone());
        let (tx, rx) = mpsc::channel();
        deliver(&shared, &tx, &|| {}, test_key(false), &path, 0, Err("bad page".into()));
        assert!(matches!(rx.try_recv(), Ok(Out::PageErr { key, msg }) if key.page == 3 && msg == "bad page"));
    }

    #[test]
    fn stage8_deliver_thumbnail_on_success_and_error() {
        let shared = Shared { queue: Mutex::new(Queue::default()), cv: Condvar::new(), hayro_doc: Mutex::new(None) };
        let path = PathBuf::from("stage8.pdf");
        shared.queue.lock().unwrap().pdf = Some(path.clone());
        let (tx, rx) = mpsc::channel();
        deliver(&shared, &tx, &|| {}, test_key(true), &path, 0, Ok(small_img()));
        assert!(matches!(rx.try_recv(), Ok(Out::Thumb { id: 2, page: 3, .. })));
        deliver(&shared, &tx, &|| {}, test_key(true), &path, 0, Err("bad thumb".into()));
        assert!(matches!(rx.try_recv(), Ok(Out::ThumbErr { id: 2, page: 3 })));
    }

    #[test]
    fn stage8_xml_entity_decoding_handles_all_standard_escapes() {
        assert_eq!(xml_unescape("A&amp;B &lt;C&gt; &quot;D&quot; &apos;E&apos;"), "A&B <C> \"D\" 'E'");
    }

    #[test]
    fn stage8_relative_links_decode_utf8_while_preserving_bad_escapes() {
        let target = link_path("../media/%D1%82%D0%B5%D1%81%D1%82%20clip.mp4", Path::new("deck/one.pptx"));
        assert_eq!(target.file_name().unwrap().to_string_lossy(), "тест clip.mp4");
        assert!(link_path("a%Q2.mp4", Path::new("deck/one.pptx")).to_string_lossy().contains("%Q2"));
    }

    #[test]
    fn stage8_linked_media_missing_archive_returns_empty_list() {
        let scratch = Scratch::new();
        assert!(linked_media(&scratch.file("gone.pptx")).is_empty());
        let invalid = scratch.file("invalid.pptx");
        std::fs::write(&invalid, b"not zip").unwrap();
        assert!(linked_media(&invalid).is_empty());
    }

    #[test]
    fn stage8_viewport_rejects_nan_aspect_and_tiny_screens() {
        let view = viewport(f32::NAN, (0.0, 0.0), 2.0, (0.5, 0.5));
        assert!(view.iter().all(|v| v.is_finite() && *v >= 0.0 && *v <= 1.0));
        let key = view_key([-1.0, 0.49999, 2.0, 1.0]);
        assert_eq!(key, [0, 5000, 10000, 10000]);
    }

    #[test]
    fn stage8_pdf_engine_labels_match_serialized_preference() {
        assert_eq!(PdfEngine::Pdfium.label(), "PDFium");
        assert_eq!(PdfEngine::Hayro.label(), "Hayro");
        assert_eq!(PdfEngine::Hayro.as_str(), "hayro");
    }

    fn minimal_test_pdf() -> Vec<u8> {
        use std::fmt::Write as _;

        let stream = b"0.2 0.4 0.8 rg 0 0 72 54 re f";
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 72 54] /Resources << >> /Contents 4 0 R >>".to_vec(),
            [format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes(), stream.to_vec(), b"\nendstream".to_vec()].concat(),
        ];

        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::with_capacity(objects.len());
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            pdf.extend_from_slice(object);
            pdf.extend_from_slice(b"\nendobj\n");
        }

        let xref = pdf.len();
        let mut tail = String::new();
        writeln!(&mut tail, "xref\n0 {}", objects.len() + 1).unwrap();
        tail.push_str("0000000000 65535 f \n");
        for offset in offsets {
            writeln!(&mut tail, "{offset:010} 00000 n ").unwrap();
        }
        write!(&mut tail, "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).unwrap();
        pdf.extend_from_slice(tail.as_bytes());
        pdf
    }

    #[test]
    fn stage14_hayro_render_recovers_after_corrupt_pdf() {
        use std::sync::mpsc;

        let scratch = Scratch::new();
        let broken = scratch.file("broken.pdf");
        let valid = scratch.file("valid.pdf");
        std::fs::write(&broken, b"not a PDF document").unwrap();
        std::fs::write(&valid, minimal_test_pdf()).unwrap();

        let (tx, rx) = mpsc::channel();
        let renderer = Renderer::new(tx.clone(), || {}, "hayro");
        assert_eq!(renderer.engine(), PdfEngine::Hayro);

        let bad_key = PageKey { id: 1401, page: 0, w: 144, h: 108, thumb: false, view: FULL_VIEW };
        renderer.want(&broken, vec![bad_key]);
        match rx.recv_timeout(Duration::from_secs(15)).expect("corrupt PDF render timeout") {
            Out::PageErr { key, msg } => {
                assert_eq!(key, bad_key);
                assert!(!msg.trim().is_empty());
            }
            _ => panic!("corrupt PDF must report PageErr"),
        }

        let good_key = PageKey { id: 1402, page: 0, w: 144, h: 108, thumb: false, view: FULL_VIEW };
        renderer.want(&valid, vec![good_key]);
        match rx.recv_timeout(Duration::from_secs(15)).expect("render recovery timeout") {
            Out::Page { key, image } => {
                assert_eq!(key, good_key);
                assert!(image.size[0] > 0 && image.size[1] > 0);
                assert!(image.pixels.iter().any(|pixel| pixel.r() != 255 || pixel.g() != 255 || pixel.b() != 255));
            }
            Out::PageErr { msg, .. } => panic!("valid PDF failed after recovery: {msg}"),
            _ => panic!("valid PDF must produce a rendered page"),
        }
        renderer.cancel();
    }
    #[test]
    fn stage15_hayro_bad_page_then_crop_and_full_render_on_same_renderer() {
        let scratch = Scratch::new();
        let path = scratch.file("recovery.pdf");
        std::fs::write(&path, minimal_test_pdf()).unwrap();
        let (tx, rx) = mpsc::channel();
        let renderer = Renderer::new(tx, || {}, "hayro");
        for cycle in 0..3 {
            let bad = PageKey { id: 1500 + cycle, page: 99, w: 144, h: 108, thumb: false, view: FULL_VIEW };
            renderer.want(&path, vec![bad]);
            match rx.recv_timeout(Duration::from_secs(15)).expect("bad-page timeout") {
                Out::PageErr { key, msg } => { assert_eq!(key, bad); assert!(!msg.trim().is_empty()); }
                _ => panic!("out-of-range page must fail"),
            }
            for view in [[2500, 2500, 7500, 7500], FULL_VIEW] {
                let good = PageKey { page: 0, view, ..bad };
                renderer.want(&path, vec![good]);
                match rx.recv_timeout(Duration::from_secs(15)).expect("recovery render timeout") {
                    Out::Page { key, image } => {
                        assert_eq!(key, good);
                        assert_eq!(image.size, [144, 108]);
                        assert_eq!(image.pixels.len(), 144 * 108);
                        let center = image.pixels[54 * 144 + 72];
                        assert!(center.b() > center.r(), "expected blue fixture content");
                    }
                    _ => panic!("worker must render crop and full page after page error"),
                }
            }
        }
        renderer.cancel();
    }

    #[test]
    fn stage15_hayro_thumbnail_error_then_jpeg_delivery() {
        let scratch = Scratch::new();
        let path = scratch.file("thumbnail.pdf");
        std::fs::write(&path, minimal_test_pdf()).unwrap();
        let (tx, rx) = mpsc::channel();
        let renderer = Renderer::new(tx, || {}, "hayro");
        let bad = PageKey { id: 1550, page: 99, w: 144, h: 108, thumb: true, view: FULL_VIEW };
        renderer.want(&path, vec![bad]);
        assert!(matches!(rx.recv_timeout(Duration::from_secs(15)), Ok(Out::ThumbErr { id: 1550, page: 99 })));
        renderer.want(&path, vec![PageKey { page: 0, ..bad }]);
        match rx.recv_timeout(Duration::from_secs(15)).expect("thumbnail recovery timeout") {
            Out::Thumb { id, page, image, jpg } => {
                assert_eq!((id, page), (1550, 0));
                assert_eq!(image.size, [144, 108]);
                let decoded = image::load_from_memory(&jpg).expect("thumbnail must be valid JPEG");
                assert_eq!((decoded.width(), decoded.height()), (144, 108));
            }
            _ => panic!("worker must deliver a JPEG thumbnail after ThumbErr"),
        }
        renderer.cancel();
    }

}
