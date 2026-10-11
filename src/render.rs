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

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PageKey {
    pub id: u64,
    pub page: usize,
    pub w: u32,
    pub h: u32,
    /// Миниатюра для превью у оператора (готовится и в JPEG — для клиентов).
    pub thumb: bool,
}

/// Размер миниатюры превью: страница вписывается в эту рамку.
pub const THUMB_W: u32 = 480;
pub const THUMB_H: u32 = 270;

pub enum Out {
    Prepared { id: u64, result: Result<(Option<PathBuf>, usize), String> },
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

/// Готовит файл к показу в отдельном потоке: считаются страницы или слайды.
pub fn prepare(id: u64, src: PathBuf, out: Sender<Out>, wake: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(|| prepare_inner(&src))
            .unwrap_or_else(|_| Err("внутренняя ошибка при обработке файла".into()));
        let _ = out.send(Out::Prepared { id, result });
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
    let scale = (key.w as f32 / pw).min(key.h as f32 / ph).min(65000.0 / pw.max(ph)).max(0.01);
    let settings = PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE };
    let cache = RenderCache::new();
    let pix = hayro::render(page, &cache, &InterpreterSettings::default(), &RenderSettings::default(), &settings);
    let size = [pix.width() as usize, pix.height() as usize];
    Ok(ColorImage::from_rgba_premultiplied(size, pix.data_as_u8_slice()))
}

/// Рендер страниц PDF в нескольких потоках. GUI каждый кадр сообщает, какие страницы нужны
/// (самые важные — первыми: текущая, следующие, предыдущая), рабочие потоки берут их по порядку.
/// Новый список заменяет старый — устаревшие заказы при быстром листании просто отбрасываются,
/// а страницы, которые уже рендерятся, повторно не заказываются.
pub struct Renderer {
    shared: Arc<Shared>,
}

struct Shared {
    queue: Mutex<Queue>,
    cv: Condvar,
    /// Открытый документ — один на все потоки (hayro::Pdf можно читать параллельно).
    doc: Mutex<Option<(PathBuf, Arc<Pdf>)>>,
}

#[derive(Default)]
struct Queue {
    pdf: Option<PathBuf>,
    jobs: Vec<PageKey>,
    busy: HashSet<PageKey>,
}

impl Renderer {
    pub fn new(out: Sender<Out>, wake: impl Fn() + Send + Sync + 'static) -> Renderer {
        let shared = Arc::new(Shared { queue: Mutex::new(Queue::default()), cv: Condvar::new(), doc: Mutex::new(None) });
        let wake = Arc::new(wake);
        // Половина ядер, но не больше 4: остальное — интерфейсу, видео и PowerPoint.
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(1, 4);
        for _ in 0..workers {
            let (shared, out, wake) = (shared.clone(), out.clone(), wake.clone());
            std::thread::spawn(move || worker(&shared, &out, &*wake));
        }
        Renderer { shared }
    }

    /// Какие страницы нужны сейчас, по убыванию важности. Пустой список — ничего не рендерить.
    pub fn want(&self, pdf: &Path, keys: Vec<PageKey>) {
        let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        let jobs: Vec<PageKey> = keys.into_iter().filter(|k| !q.busy.contains(k)).collect();
        if q.jobs == jobs && q.pdf.as_deref() == Some(pdf) {
            return;
        }
        q.pdf = Some(pdf.to_path_buf());
        q.jobs = jobs;
        if !q.jobs.is_empty() {
            self.shared.cv.notify_all();
        }
    }

    /// Отменить всё, что ещё не начато (показ закрыт).
    pub fn cancel(&self) {
        self.shared.queue.lock().unwrap_or_else(|e| e.into_inner()).jobs.clear();
    }
}

fn worker(shared: &Shared, out: &Sender<Out>, wake: &(dyn Fn() + Send + Sync)) {
    loop {
        let (key, path) = {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            while q.jobs.is_empty() {
                q = shared.cv.wait(q).unwrap_or_else(|e| e.into_inner());
            }
            let key = q.jobs.remove(0);
            q.busy.insert(key);
            (key, q.pdf.clone().unwrap_or_default())
        };
        let doc = {
            let mut d = shared.doc.lock().unwrap_or_else(|e| e.into_inner());
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
                .unwrap_or_else(|_| Err("ошибка рендера страницы".into()))
        });
        shared.queue.lock().unwrap_or_else(|e| e.into_inner()).busy.remove(&key);
        let (id, page) = (key.id, key.page);
        let _ = out.send(match res {
            Ok(image) if key.thumb => match to_jpeg(&image) {
                Ok(jpg) => Out::Thumb { id, page, image, jpg },
                Err(_) => Out::ThumbErr { id, page },
            },
            Ok(image) => Out::Page { key, image },
            Err(_) if key.thumb => Out::ThumbErr { id, page },
            Err(msg) => Out::PageErr { key, msg },
        });
        wake();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Только рендер, без окна и текстур: чередуем два PDF, как при переключении презентаций.
    /// TEST_PDF="a.pdf;b.pdf" TEST_ROUNDS=60 cargo test --release soak_render -- --ignored --nocapture
    #[test]
    #[ignore]
    fn soak_render() {
        let files: Vec<PathBuf> = std::env::var("TEST_PDF").unwrap().split(';').map(PathBuf::from).collect();
        let rounds: usize = std::env::var("TEST_ROUNDS").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
        let (tx, rx) = std::sync::mpsc::channel();
        let r = Renderer::new(tx, || {});
        for i in 0..rounds {
            let pdf = &files[i % files.len()];
            let keys: Vec<PageKey> = (0..5).map(|page| PageKey { id: i as u64, page, w: 1920, h: 1080, thumb: false }).collect();
            r.want(pdf, keys);
            for _ in 0..5 {
                rx.recv().unwrap();
            }
            println!("ROUND {i}");
        }
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
            render_page(&pdf, PageKey { id: 0, page, w, h, thumb: false }).unwrap();
            println!("страница {}: {} мс", page + 1, t.elapsed().as_millis());
        }
    }
}
