# Presentator

Программа для показа презентаций на мониторе зала. Сервер выводит PDF, PowerPoint,
видео и картинки, оператор управляет показом с другого компьютера по сети.
Один exe, без установки и без Python.

- **Версия:** 3.0.1 (ветка `v3.0`). Стабильная v2.5.4 — тег `v2.5.4`, ветка `v2-stable`.
- **Платформа:** Windows 10/11 x64 (PowerPoint через COM, libmpv, PDFium).
- **Сборка:** `cargo build --release` → `target/release/presentator.exe`.

## Быстрый старт

```powershell
# PDFium не хранится в git; скрипт скачает его и вошьёт в exe вместе с иконкой
powershell -ExecutionPolicy Bypass -File scripts\build_release.ps1

# либо обычная сборка, если vendor\pdfium\pdfium.dll уже на месте
cargo build --release
```

Запуск:

```text
presentator.exe                 # автономный режим
presentator.exe --server        # сервер: показывает на мониторе зала
presentator.exe --connect       # клиент: подключается к сохранённому серверу
presentator.exe --net           # сразу открыть вкладку «Сеть»
presentator.exe --add file.pdf  # добавить файл в список
presentator.exe --watchdog --server  # сервер с перезапуском при сбое
```

## Документация

- [`PROJECT.md`](PROJECT.md) — полная сводка: цель, архитектура, протокол, решения,
  что проверено и что осталось.
- [`docs/README.md`](docs/README.md) — индекс документации.
- [`docs/releases/3.0.1.md`](docs/releases/3.0.1.md) — что нового в 3.0.1.
- [`tests/fixtures/3.0/README.md`](tests/fixtures/3.0/README.md) — тестовые файлы
  и чек-лист ручной проверки.

## Структура

| Папка | Что внутри |
|---|---|
| `src/` | исходники программы и тесты |
| `docs/` | релизы, тесты, история этапов, снимки |
| `scripts/` | сборка, проверки, генерация тестовых файлов |
| `tests/fixtures/` | тестовые медиафайлы (PDF, PPTX, видео, звук, картинки) |
| `vendor/` | вшитые libmpv и PDFium |
| `assets/` | иконка и стандартный фон |

Историю изменений по этапам смотрите в `docs/history/`; это архивные отчёты,
их команды и пути могут относиться к старым версиям.

## Проверки

```powershell
cargo check --all-targets
cargo test                       # 399 тестов; нативные/e2e — вручную
cargo clippy --all-targets
```

Нативные тесты PowerPoint, видео и реестра запускаются скриптами
`scripts/verify_*.ps1` на тестовом рабочем столе.

## Сторонние компоненты

- **libmpv / FFmpeg** — динамически загружаемая DLL, вшита в exe
  (`vendor/libmpv`). Сборка shinchiro распространяется по GPL; при передаче
  программы третьим лицам учитывайте требования лицензии.
- **PDFium** — BSD-3-Clause, DLL вшита в exe (`vendor/pdfium`).
- **egui / eframe**, **tokio**, **pdfium-render**, **hayro** и остальные крейты —
  см. `Cargo.toml` и лицензии соответствующих проектов.
