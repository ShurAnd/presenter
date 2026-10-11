//! Фон сцены: картинка или анимация (GIF, анимированные WebP и APNG).
//!
//! Откуда берётся (по порядку): выбранный в программе — `%APPDATA%\presenter\background.<расширение>`
//! (один файл: при новой смене прежний удаляется); `background.png` / `.jpg` / `.jpeg` рядом с exe;
//! вшитая `assets/background.png`.
//!
//! Видео как фон не используется: его пришлось бы держать в отдельном окне под сценой и
//! переставлять при каждой смене показа. Анимация рисуется прямо на сцене — кадры лежат в памяти
//! (уменьшенные, общий объём ограничен), на сцене одна текстура, в которую подставляется кадр.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage};
use image::{AnimationDecoder, ImageFormat, RgbaImage};

/// Расширения, которые можно выбрать фоном.
pub const EXTS: [&str; 6] = ["png", "jpg", "jpeg", "bmp", "gif", "webp"];
/// Файлы фона рядом с exe — как в прежних версиях (замена вшитой картинки без программы).
const EXE_FILES: [&str; 3] = ["background.png", "background.jpg", "background.jpeg"];
/// Имя выбранного фона в папке настроек (плюс расширение исходного файла).
const CHOSEN: &str = "background";
/// Сколько памяти можно отдать кадрам анимации.
const ANIM_BUDGET: u64 = 400 * 1024 * 1024;
/// Кадр анимации — не больше этого (меньше памяти; на экране растягивается).
const ANIM_MAX: (u32, u32) = (1920, 1080);
const MAX_FRAMES: usize = 2000;

/// Кадр и сколько он показывается.
pub type Frames = Vec<(Arc<ColorImage>, Duration)>;

/// Папка настроек программы — там лежит выбранный фон.
fn dir() -> PathBuf {
    crate::config_path().parent().map(Path::to_path_buf).unwrap_or_else(crate::app_dir)
}

/// Выбранный в программе фон, если есть.
fn chosen() -> Option<PathBuf> {
    let dir = dir();
    EXTS.iter().map(|e| dir.join(format!("{CHOSEN}.{e}"))).find(|p| p.is_file())
}

/// Сделать картинку фоном: копия в папку настроек, прежний выбранный фон удаляется.
pub fn set(src: &Path) -> Result<(), String> {
    let ext = src.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !EXTS.contains(&ext.as_str()) {
        return Err(format!("формат .{ext} не подходит для фона (нужен PNG, JPG, BMP, GIF или WebP)"));
    }
    let dir = dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // Сначала копия под временным именем: если копирование оборвётся, прежний фон останется.
    let tmp = dir.join(format!("{CHOSEN}.new.{ext}"));
    std::fs::copy(src, &tmp).map_err(|e| format!("не удалось скопировать: {e}"))?;
    reset();
    std::fs::rename(&tmp, dir.join(format!("{CHOSEN}.{ext}"))).map_err(|e| e.to_string())
}

/// Вернуть стандартный фон: выбранный удаляется.
pub fn reset() {
    let dir = dir();
    for e in EXTS {
        let _ = std::fs::remove_file(dir.join(format!("{CHOSEN}.{e}")));
    }
}

/// Загрузить фон (в отдельном потоке — анимация декодируется секунды). `max_side` — предел стороны
/// текстуры видеокарты. Err — ни одна картинка не прочиталась (сцена будет залита цветом).
pub fn load(max_side: u32) -> Result<Frames, String> {
    let file = chosen().or_else(|| EXE_FILES.iter().map(|n| crate::app_dir().join(n)).find(|p| p.is_file()));
    if let Some(f) = file {
        match std::fs::read(&f).map_err(|e| e.to_string()).and_then(|b| decode(&b, max_side)) {
            Ok(frames) => return Ok(frames),
            // Испорченный файл — не повод оставлять экран без фона: берём вшитый.
            Err(e) => eprintln!("фон {}: {e}", f.display()),
        }
    }
    decode(include_bytes!("../assets/background.png"), max_side)
}

/// Картинка из файла (для показа картинок как презентаций): кадры, как у фона.
pub fn load_file(path: &Path, max_side: u32) -> Result<Frames, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("не удалось прочитать файл: {e}"))?;
    decode(&bytes, max_side)
}

/// Картинка или анимация → кадры.
fn decode(bytes: &[u8], max_side: u32) -> Result<Frames, String> {
    let fmt = image::guess_format(bytes).map_err(|e| e.to_string())?;
    let cur = || std::io::Cursor::new(bytes);
    let animated = match fmt {
        ImageFormat::Gif => true,
        ImageFormat::Png => image::codecs::png::PngDecoder::new(cur()).and_then(|d| d.is_apng()).unwrap_or(false),
        ImageFormat::WebP => image::codecs::webp::WebPDecoder::new(cur()).map(|d| d.has_animation()).unwrap_or(false),
        _ => false,
    };
    if animated {
        let frames = || -> Result<image::Frames<'_>, String> {
            Ok(match fmt {
                ImageFormat::Gif => image::codecs::gif::GifDecoder::new(cur()).map_err(|e| e.to_string())?.into_frames(),
                ImageFormat::Png => image::codecs::png::PngDecoder::new(cur()).and_then(|d| d.apng()).map_err(|e| e.to_string())?.into_frames(),
                _ => image::codecs::webp::WebPDecoder::new(cur()).map_err(|e| e.to_string())?.into_frames(),
            })
        };
        let anim = decode_anim(frames, max_side)?;
        // Анимация из одного кадра — обычная картинка.
        if anim.len() > 1 {
            return Ok(anim);
        }
    }
    let img = image::load_from_memory_with_format(bytes, fmt).map_err(|e| e.to_string())?;
    let max = max_side.min(8192);
    let img = if img.width() > max || img.height() > max { img.resize(max, max, image::imageops::FilterType::Triangle) } else { img };
    Ok(vec![(Arc::new(color_image(&img.to_rgba8())), Duration::ZERO)])
}

/// Кадры анимации, уменьшенные так, чтобы каждый влез в ANIM_MAX, а все вместе — в ANIM_BUDGET.
/// Два прохода: сначала считаем кадры (иначе не узнать, до какого размера ужимать), потом
/// декодируем, сразу уменьшая каждый кадр, — полноразмерные кадры в памяти не копятся.
fn decode_anim<'a>(frames: impl Fn() -> Result<image::Frames<'a>, String>, max_side: u32) -> Result<Frames, String> {
    let mut count = 0usize;
    let mut size = (0u32, 0u32);
    for f in frames()?.take(MAX_FRAMES) {
        let f = f.map_err(|e| e.to_string())?;
        if count == 0 {
            size = f.buffer().dimensions();
        }
        count += 1;
    }
    if count == 0 || size.0 == 0 || size.1 == 0 {
        return Err("в анимации нет кадров".into());
    }
    let (w, h) = (size.0 as f64, size.1 as f64);
    let fit = (ANIM_MAX.0.min(max_side) as f64 / w).min(ANIM_MAX.1.min(max_side) as f64 / h).min(1.0);
    let budget = (ANIM_BUDGET as f64 / (count as f64 * w * h * 4.0)).sqrt().min(1.0);
    let k = fit.min(budget);
    let (tw, th) = (((w * k).round() as u32).max(1), ((h * k).round() as u32).max(1));
    let mut out = Vec::with_capacity(count);
    for f in frames()?.take(MAX_FRAMES) {
        let f = f.map_err(|e| e.to_string())?;
        let (num, den) = f.delay().numer_denom_ms();
        let ms = if den == 0 { 0.0 } else { num as f64 / den as f64 };
        // Как в браузерах: слишком короткие задержки (часто 0) — это 100 мс.
        let delay = Duration::from_millis(if ms < 20.0 { 100 } else { ms as u64 });
        let buf = f.into_buffer();
        let buf = if (tw, th) == buf.dimensions() { buf } else { image::imageops::resize(&buf, tw, th, image::imageops::FilterType::Triangle) };
        out.push((Arc::new(color_image(&buf)), delay));
    }
    Ok(out)
}

fn color_image(img: &RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

/// Фон на сцене (или картинка, открытая как презентация). Каждый кадр анимации загружается в видеокарту один раз — при первом показе,
/// дальше только выбирается, какую текстуру рисовать. Копия кадра в обычной памяти после загрузки
/// не нужна и освобождается. Основная цена анимации — не загрузка кадров, а перерисовка: сцена
/// рисуется вместе с главным окном, при 15 кадрах/с это ~15 % одного ядра (замер 11.10.2026).
pub struct Background {
    ctx: egui::Context,
    /// Кадр (пока не загружен в видеокарту), его текстура (когда загружен) и время показа.
    frames: Vec<(Option<Arc<ColorImage>>, Option<egui::TextureHandle>, Duration)>,
    total: Duration,
    start: Instant,
    shown: usize,
}

impl Background {
    pub fn new(ctx: &egui::Context, frames: Frames) -> Background {
        let total = frames.iter().map(|f| f.1).sum();
        let frames = frames.into_iter().map(|(img, d)| (Some(img), None, d)).collect();
        let mut bg = Background { ctx: ctx.clone(), frames, total, start: Instant::now(), shown: 0 };
        bg.upload(0);
        bg
    }

    fn upload(&mut self, i: usize) {
        let f = &mut self.frames[i];
        if let Some(img) = f.0.take() {
            f.1 = Some(self.ctx.load_texture(format!("background{i}"), egui::ImageData::Color(img), egui::TextureOptions::LINEAR));
        }
    }

    /// Текстура текущего кадра.
    pub fn texture(&self) -> &egui::TextureHandle {
        self.frames[self.shown].1.as_ref().expect("кадр загружается до показа")
    }

    pub fn animated(&self) -> bool {
        self.frames.len() > 1
    }

    /// Анимация: выбрать кадр по времени. Возвращает, через сколько сменится следующий
    /// (None — картинка неподвижна). Вызывать, только пока фон виден, — иначе зря тратится процессор.
    pub fn tick(&mut self) -> Option<Duration> {
        if self.frames.len() < 2 || self.total.is_zero() {
            return None;
        }
        let total = self.total.as_micros().max(1);
        let mut t = Duration::from_micros((self.start.elapsed().as_micros() % total) as u64);
        let mut i = 0;
        while t >= self.frames[i].2 && i + 1 < self.frames.len() {
            t -= self.frames[i].2;
            i += 1;
        }
        self.upload(i);
        self.shown = i;
        Some(self.frames[i].2.saturating_sub(t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Анимированный GIF: кадры, задержки, ужатие.
    #[test]
    fn gif_frames() {
        use image::codecs::gif::{GifEncoder, Repeat};
        use image::{Delay, Frame, Rgba};
        let mut bytes = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut bytes);
            enc.set_repeat(Repeat::Infinite).unwrap();
            for (i, ms) in [(0u8, 50u32), (1, 0), (2, 200)] {
                let img = RgbaImage::from_pixel(40, 20, Rgba([i * 100, 0, 0, 255]));
                enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(ms, 1))).unwrap();
            }
        }
        let f = decode(&bytes, 4096).unwrap();
        assert_eq!(f.len(), 3);
        assert_eq!(f[0].0.size, [40, 20]);
        // 0 мс — как в браузерах, 100 мс.
        let delays: Vec<u64> = f.iter().map(|x| x.1.as_millis() as u64).collect();
        assert_eq!(delays, [50, 100, 200]);
        // Предел стороны текстуры — кадры уменьшаются.
        let f = decode(&bytes, 10).unwrap();
        assert!(f[0].0.size[0] <= 10 && f[0].0.size[1] <= 10);
    }

    #[test]
    fn static_png_is_one_frame() {
        let f = decode(include_bytes!("../assets/icon_256.png"), 4096).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].0.size, [256, 256]);
    }
}
