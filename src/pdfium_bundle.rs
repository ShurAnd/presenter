//! Встроенная PDFium DLL — один распространяемый presentator.exe.
//!
//! Движок упакован в исполняемый файл на этапе сборки; при первом запуске
//! DLL извлекается в %LOCALAPPDATA%\Presentator\pdfium-7881-<hash>\.
//! Это именно внутренний кэш (аналогично libmpv), а не файл для ручной установки.
//! Предпочитаем версионированную директорию и проверяем SHA-256, чтобы
//! не загружать неполный, повреждённый или подменённый файл из кэша.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const PDFIUM_BUILD: &str = "7881";
static PDFIUM_DLL: &[u8] = include_bytes!("../vendor/pdfium/pdfium.dll");

fn matches_embedded(path: &Path, expected_sha256: &[u8]) -> bool {
    let Ok(bytes) = fs::read(path) else { return false };
    bytes.len() == PDFIUM_DLL.len() && Sha256::digest(&bytes).as_slice() == expected_sha256
}

/// Возвращает абсолютный путь к загруженной во внутренний кэш PDFium DLL.
/// Если кэш отсутствует, файл создаётся из данных, вшитых в exe.
/// Не трогает папку с исполняемым файлом и не требует доступа в интернет.
pub(crate) fn extract_pdfium() -> Result<PathBuf, String> {
    if PDFIUM_DLL.len() < 1024 || !PDFIUM_DLL.starts_with(b"MZ") {
        return Err("PDFium был некорректно встроен в EXE (неверный формат DLL)".into());
    }

    let expected = Sha256::digest(PDFIUM_DLL);
    let suffix = expected[..8].iter().map(|b| format!("{b:02x}")).collect::<String>();
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("Presentator").join(format!("pdfium-{PDFIUM_BUILD}-x64-{suffix}"));
    let dll = dir.join("pdfium.dll");

    if matches_embedded(&dll, &expected) {
        return Ok(dll);
    }

    fs::create_dir_all(&dir)
        .map_err(|e| format!("нет доступа к каталогу PDFium {}: {e}", dir.display()))?;
    // Если прошлый файл повреждён, заменяем его ДО загрузки. При запуске
    // двух экземпляров второй поток может уже получить правильную DLL.
    if dll.exists() && !matches_embedded(&dll, &expected) {
        fs::remove_file(&dll)
            .map_err(|e| format!("не удалось заменить повреждённую PDFium DLL: {e}"))?;
    }

    // У каждого процесса собственный временный файл. Если два Presentator
    // стартуют одновременно, один из них сохранит DLL, второй использует её.
    let tmp = dir.join(format!("pdfium.dll.{}.tmp", std::process::id()));
    let result = (|| -> Result<(), String> {
        let mut f = OpenOptions::new().write(true).create(true).truncate(true).open(&tmp)
            .map_err(|e| format!("не удалось создать {}: {e}", tmp.display()))?;
        f.write_all(PDFIUM_DLL)
            .map_err(|e| format!("не удалось распаковать PDFium: {e}"))?;
        f.sync_all().map_err(|e| format!("не удалось сохранить PDFium: {e}"))?;
        drop(f);

        if !matches_embedded(&tmp, &expected) {
            return Err("проверка SHA-256 PDFium после распаковки не пройдена".into());
        }

        match fs::rename(&tmp, &dll) {
            Ok(()) => Ok(()),
            Err(_) if matches_embedded(&dll, &expected) => Ok(()),
            Err(e) => Err(format!("не удалось разместить PDFium в кэше: {e}")),
        }
    })();
    let _ = fs::remove_file(&tmp);
    result?;

    if !matches_embedded(&dll, &expected) {
        return Err("файл PDFium в кэше не прошёл проверку SHA-256".into());
    }
    Ok(dll)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdfium_is_embedded_and_can_be_extracted() {
        assert!(PDFIUM_DLL.len() > 1024 * 1024, "библиотека PDFium не должна быть пустой");
        assert!(PDFIUM_DLL.starts_with(b"MZ"), "PDFium должен быть Windows DLL");
        let path = extract_pdfium().expect("не удалось извлечь PDFium из exe");
        assert!(matches_embedded(&path, &Sha256::digest(PDFIUM_DLL)));
    }
}
