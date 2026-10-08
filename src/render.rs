//! Фоновая подготовка файлов (подсчёт страниц и слайдов) и рендер страниц PDF.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

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
}

pub struct RenderJob {
    pub key: PageKey,
    pub pdf: PathBuf,
}

pub enum Out {
    Prepared { id: u64, result: Result<(Option<PathBuf>, usize), String> },
    Ppt { gen: u64, ev: crate::ppt::PptEvent },
    /// Esc, нажатый в слайд-шоу PowerPoint (перехвачен до PowerPoint).
    ShowEsc,
    Video { gen: u64, ev: crate::video::VideoEvent },
    Page { key: PageKey, image: ColorImage },
    PageErr { key: PageKey, msg: String },
}

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

/// Поток рендера: держит открытым последний документ, устаревшие запросы пропускает.
pub fn spawn_renderer(jobs: Receiver<RenderJob>, out: Sender<Out>, wake: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let mut doc: Option<(PathBuf, Pdf)> = None;
        while let Ok(first) = jobs.recv() {
            let mut batch = vec![first];
            batch.extend(jobs.try_iter());
            // Если накопилось много (быстрое листание) — актуальны только последние запросы.
            let skip = batch.len().saturating_sub(4);
            for job in batch.into_iter().skip(skip).rev() {
                if doc.as_ref().map(|d| &d.0) != Some(&job.pdf) {
                    doc = match load_pdf(&job.pdf) {
                        Ok(p) => Some((job.pdf.clone(), p)),
                        Err(msg) => {
                            let _ = out.send(Out::PageErr { key: job.key, msg });
                            wake();
                            continue;
                        }
                    };
                }
                let pdf = &doc.as_ref().unwrap().1;
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render_page(pdf, job.key)))
                    .unwrap_or_else(|_| Err("ошибка рендера страницы".into()));
                let _ = out.send(match res {
                    Ok(image) => Out::Page { key: job.key, image },
                    Err(msg) => Out::PageErr { key: job.key, msg },
                });
                wake();
            }
        }
    });
}
