//! Журнал событий: `%APPDATA%\presenter\presentator.log` (рядом с настройками). Что открывали,
//! ответы PowerPoint, ошибки видео, обрывы связи, сообщения оператору, сбои программы — чтобы
//! разбирать «что-то не сработало» по файлу, а не по памяти. Дорос до 2 МБ — прежний уходит в
//! `presentator.old.log`. В каждой строке — время и номер процесса: на одном компьютере могут
//! работать сразу сервер и клиент, они пишут в один файл.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX: u64 = 2 * 1024 * 1024;

static FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// Путь к журналу.
pub fn path() -> PathBuf {
    crate::config_path().parent().map(|p| p.join("presentator.log")).unwrap_or_else(|| crate::app_dir().join("presentator.log"))
}

/// Начало работы: строка о запуске и запись сбоев (паник) в журнал.
pub fn init() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    write(&format!("=== Запуск Presentator v{} {}", env!("CARGO_PKG_VERSION"), args.join(" ")));
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write(&format!("СБОЙ ПРОГРАММЫ: {info}"));
        default(info);
    }));
}

/// Записать строку. Ошибки записи молча пропускаем — журнал не должен мешать показу.
pub fn write(msg: &str) {
    let t = crate::screen::local_time();
    let line = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} [{}] {}\r\n",
        t.year, t.month, t.day, t.h, t.m, t.s, t.ms, std::process::id(), msg.replace('\n', " / ")
    );
    let mut g = FILE.lock().unwrap_or_else(|e| e.into_inner());
    let path = path();
    // Большой — в .old, начинаем новый.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX) {
        *g = None;
        let old = path.with_file_name("presentator.old.log");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(&path, &old);
    }
    if g.is_none() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        *g = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok();
    }
    if let Some(f) = g.as_mut() {
        if f.write_all(line.as_bytes()).and_then(|_| f.flush()).is_err() {
            *g = None; // файл переименовали или удалили — откроем заново в следующий раз
        }
    }
}

/// Запись в журнал в духе `format!`.
#[macro_export]
macro_rules! jlog {
    ($($t:tt)*) => { $crate::journal::write(&format!($($t)*)) };
}
