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
