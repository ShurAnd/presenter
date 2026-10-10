//! Экраны сцены без показа: часы и таймер (обратный отсчёт или секундомер).

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32};

use crate::net::TimerInfo;

/// Таймер на сервере. Время считается от `Instant`, поэтому не зависит от частоты кадров.
pub struct Timer {
    countdown: bool,
    total: u32,
    /// Сколько набежало до последней остановки, мс.
    base: u64,
    started: Option<Instant>,
}

impl Default for Timer {
    fn default() -> Self {
        Self { countdown: true, total: 5 * 60, base: 0, started: None }
    }
}

/// Больше 99 часов таймер не показывает.
const MAX_SECS: u32 = 99 * 3600 + 59 * 60 + 59;

impl Timer {
    fn elapsed(&self) -> u64 {
        self.base + self.started.map_or(0, |t| t.elapsed().as_millis() as u64)
    }

    pub fn info(&self) -> TimerInfo {
        TimerInfo { countdown: self.countdown, total: self.total, elapsed: self.elapsed(), running: self.started.is_some() }
    }

    /// Смена режима сбрасывает таймер; новая длительность того же режима время не трогает
    /// (так можно добавить минуту идущему отсчёту).
    pub fn set(&mut self, countdown: bool, secs: u32) {
        if countdown != self.countdown {
            self.reset();
        }
        self.countdown = countdown;
        self.total = secs.clamp(1, MAX_SECS);
    }

    pub fn run(&mut self, run: bool) {
        match (run, self.started) {
            (true, None) => self.started = Some(Instant::now()),
            (false, Some(_)) => {
                self.base = self.elapsed();
                self.started = None;
            }
            _ => {}
        }
    }

    pub fn reset(&mut self) {
        self.base = 0;
        self.started = None;
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    Normal,
    /// Обратный отсчёт: осталась последняя минута.
    Warn,
    /// Обратный отсчёт: время вышло (дальше идёт перерасход со знаком «+»).
    Over,
}

/// Текст таймера («04:59», «1:02:03», «+00:12») и его фаза. `extra` — сколько мс прошло с момента,
/// когда сняли `info` (клиент досчитывает идущий таймер сам).
pub fn timer_text(info: &TimerInfo, extra: u64) -> (String, Phase) {
    let elapsed = info.elapsed + if info.running { extra } else { 0 };
    let hms = |s: u64| if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{:02}:{:02}", s / 60, s % 60) };
    if !info.countdown {
        return (hms(elapsed / 1000), Phase::Normal);
    }
    let total = info.total as u64 * 1000;
    if elapsed < total {
        // Оставшиеся секунды округляем вверх: «00:01» до самого конца, затем сразу «00:00».
        let left = (total - elapsed).div_ceil(1000);
        (hms(left), if left <= 60 { Phase::Warn } else { Phase::Normal })
    } else if elapsed - total < 1000 {
        (hms(0), Phase::Over)
    } else {
        (format!("+{}", hms((elapsed - total) / 1000)), Phase::Over)
    }
}

/// Через сколько сменится текст идущего таймера (чтобы перерисовать ровно тогда).
pub fn timer_tick(info: &TimerInfo, extra: u64) -> Duration {
    let elapsed = info.elapsed + extra;
    Duration::from_millis(1000 - elapsed % 1000 + 5)
}

pub fn phase_color(phase: Phase) -> Color32 {
    match phase {
        Phase::Normal => Color32::WHITE,
        Phase::Warn => Color32::from_rgb(0xff, 0xc1, 0x07),
        Phase::Over => Color32::from_rgb(0xff, 0x4d, 0x4d),
    }
}

/// Местное время: часы, минуты, секунды, день, месяц (1–12), год, день недели (0 — воскресенье).
pub struct LocalTime {
    pub h: u32,
    pub m: u32,
    pub s: u32,
    pub day: u32,
    pub month: u32,
    pub year: u32,
    pub dow: u32,
    /// Миллисекунды текущей секунды — чтобы перерисовать часы ровно в начале следующей.
    pub ms: u32,
}

pub fn local_time() -> LocalTime {
    #[cfg(windows)]
    unsafe {
        let t = windows::Win32::System::SystemInformation::GetLocalTime();
        LocalTime {
            h: t.wHour as u32,
            m: t.wMinute as u32,
            s: t.wSecond as u32,
            day: t.wDay as u32,
            month: t.wMonth as u32,
            year: t.wYear as u32,
            dow: t.wDayOfWeek as u32,
            ms: t.wMilliseconds as u32,
        }
    }
    #[cfg(not(windows))]
    {
        // Не Windows (только для отладки): всемирное время.
        let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let secs = d.as_secs();
        let days = secs / 86400;
        let (year, month, day) = civil_from_days(days as i64);
        LocalTime {
            h: (secs / 3600 % 24) as u32,
            m: (secs / 60 % 60) as u32,
            s: (secs % 60) as u32,
            day,
            month,
            year,
            dow: ((days + 4) % 7) as u32,
            ms: d.subsec_millis(),
        }
    }
}

#[cfg(not(windows))]
fn civil_from_days(z: i64) -> (u32, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y as u32, m as u32, d as u32)
}

const WEEKDAYS: [&str; 7] = ["воскресенье", "понедельник", "вторник", "среда", "четверг", "пятница", "суббота"];
const MONTHS: [&str; 12] = ["января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря"];

/// «суббота, 10 октября 2026»
pub fn date_text(t: &LocalTime) -> String {
    let wd = WEEKDAYS.get(t.dow as usize).copied().unwrap_or("");
    let mon = MONTHS.get(t.month.saturating_sub(1) as usize).copied().unwrap_or("");
    format!("{wd}, {} {mon} {}", t.day, t.year)
}

/// Экран часов: крупно время, под ним дата. Возвращает, через сколько перерисовать.
pub fn draw_clock(p: &egui::Painter, rect: egui::Rect) -> Duration {
    let t = local_time();
    dim(p, rect);
    let time = format!("{:02}:{:02}:{:02}", t.h, t.m, t.s);
    let size = fit_size(p, rect, &time, 0.34);
    let center = rect.center() - egui::vec2(0.0, size * 0.18);
    draw_digits(p, center, &time, size, Color32::WHITE);
    let date = date_text(&t);
    let font = egui::FontId::proportional(size * 0.22);
    let pos = center + egui::vec2(0.0, size * 0.75);
    p.text(pos + egui::vec2(0.0, size * 0.012), egui::Align2::CENTER_CENTER, &date, font.clone(), Color32::from_black_alpha(110));
    p.text(pos, egui::Align2::CENTER_CENTER, &date, font, Color32::from_white_alpha(230));
    Duration::from_millis((1000 - t.ms.min(999)) as u64 + 5)
}

/// Экран таймера. Возвращает, через сколько перерисовать (None — таймер стоит).
pub fn draw_timer(p: &egui::Painter, rect: egui::Rect, info: &TimerInfo) -> Option<Duration> {
    dim(p, rect);
    let (text, phase) = timer_text(info, 0);
    // Размер шрифта — по самому длинному виду текста, чтобы цифры не прыгали при смене «59:59» → «1:00:00».
    let widest = if text.len() > 6 { "+00:00:00" } else { "+00:00" };
    let size = fit_size(p, rect, widest, 0.42);
    draw_digits(p, rect.center(), &text, size, phase_color(phase));
    info.running.then(|| timer_tick(info, 0))
}

/// Затемнение фона под крупными цифрами — чтобы они читались на любой картинке.
fn dim(p: &egui::Painter, rect: egui::Rect) {
    p.rect_filled(rect, 0.0, Color32::from_black_alpha(120));
}

/// Размер шрифта, при котором `text` займёт не больше 85 % ширины и `part` высоты экрана.
fn fit_size(p: &egui::Painter, rect: egui::Rect, text: &str, part: f32) -> f32 {
    let w100 = digits_width(p, text, 100.0);
    (rect.height() * part).min(rect.width() * 0.85 / w100.max(1.0) * 100.0).max(8.0)
}

/// Ширина ячейки каждого символа: цифры — одинаковой ширины (по «0»), чтобы текст не дрожал.
fn cells(p: &egui::Painter, text: &str, size: f32) -> Vec<f32> {
    let font = egui::FontId::proportional(size);
    let w = |s: String| p.layout_no_wrap(s, font.clone(), Color32::WHITE).size().x;
    let digit = w("0".into());
    text.chars().map(|c| if c.is_ascii_digit() { digit } else { w(c.to_string()) }).collect()
}

fn digits_width(p: &egui::Painter, text: &str, size: f32) -> f32 {
    cells(p, text, size).iter().sum()
}

/// Цифры по центру `center`, каждая в своей ячейке, с лёгкой тенью.
fn draw_digits(p: &egui::Painter, center: egui::Pos2, text: &str, size: f32, color: Color32) {
    let font = egui::FontId::proportional(size);
    let cells = cells(p, text, size);
    let mut x = center.x - cells.iter().sum::<f32>() / 2.0;
    let shadow = egui::vec2(0.0, size * 0.02);
    for (c, w) in text.chars().zip(cells) {
        let pos = egui::pos2(x + w / 2.0, center.y);
        p.text(pos + shadow, egui::Align2::CENTER_CENTER, c, font.clone(), Color32::from_black_alpha(110));
        p.text(pos, egui::Align2::CENTER_CENTER, c, font.clone(), color);
        x += w;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(countdown: bool, total: u32, elapsed: u64, running: bool) -> TimerInfo {
        TimerInfo { countdown, total, elapsed, running }
    }

    #[test]
    fn countdown_text() {
        assert_eq!(timer_text(&info(true, 300, 0, false), 0), ("05:00".into(), Phase::Normal));
        assert_eq!(timer_text(&info(true, 300, 1, true), 0), ("05:00".into(), Phase::Normal));
        assert_eq!(timer_text(&info(true, 300, 1000, true), 0), ("04:59".into(), Phase::Normal));
        assert_eq!(timer_text(&info(true, 300, 240_000, true), 0), ("01:00".into(), Phase::Warn));
        assert_eq!(timer_text(&info(true, 300, 299_500, true), 0), ("00:01".into(), Phase::Warn));
        assert_eq!(timer_text(&info(true, 300, 300_000, true), 0), ("00:00".into(), Phase::Over));
        assert_eq!(timer_text(&info(true, 300, 312_400, true), 0), ("+00:12".into(), Phase::Over));
        assert_eq!(timer_text(&info(true, 7200, 0, false), 0), ("2:00:00".into(), Phase::Normal));
        // Клиент досчитывает идущий таймер, стоящий — нет.
        assert_eq!(timer_text(&info(true, 300, 0, true), 2000).0, "04:58");
        assert_eq!(timer_text(&info(true, 300, 0, false), 2000).0, "05:00");
    }

    #[test]
    fn stopwatch_text() {
        assert_eq!(timer_text(&info(false, 300, 0, false), 0).0, "00:00");
        assert_eq!(timer_text(&info(false, 300, 61_999, true), 0).0, "01:01");
        assert_eq!(timer_text(&info(false, 300, 3_723_000, true), 0).0, "1:02:03");
    }

    #[test]
    fn timer_logic() {
        let mut t = Timer::default();
        assert!(t.info().countdown && t.info().total == 300 && !t.info().running);
        t.base = 10_000;
        t.set(true, 600); // та же «обратная» — время не сбрасывается
        assert_eq!((t.info().total, t.info().elapsed), (600, 10_000));
        t.set(false, 600); // смена режима — сброс
        assert_eq!(t.info().elapsed, 0);
        t.run(true);
        assert!(t.info().running);
        t.run(false);
        assert!(!t.info().running);
        t.reset();
        assert_eq!(t.info().elapsed, 0);
    }
}
