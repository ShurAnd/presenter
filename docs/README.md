# Документация Presentator

Актуальная документация — в [`../PROJECT.md`](../PROJECT.md). Здесь — релизы,
отчёты о тестировании, дизайн-заметки и архив этапов разработки.
## Релизы

- [`releases/3.0.1.md`](releases/3.0.1.md) — визуальные правки после проверки
  3.0.0 на двух компьютерах: компактные шапка, список и превью, иконка клавиатуры.
- [`releases/3.0.0.md`](releases/3.0.0.md) — новый интерфейс, две темы,
  разделение `App`, исправления по ревью.
- [`releases/2.5.2.md`](releases/2.5.2.md) — порядок окон «видео — сцена».
- [`releases/2.5.1-update.md`](releases/2.5.1-update.md) — обновление 2.5.1.
- [`releases/2.4.1-update.txt`](releases/2.4.1-update.txt) — обновление 2.4.1.

## Тестирование

- [`testing/`](testing/) — отчёты по этапам 2–10, аудит, инвентарь тестов.
- [`testing/PPT_FOCUS_TESTS.md`](testing/PPT_FOCUS_TESTS.md) — проверка фокуса
  PowerPoint на живом показе.
- [`testing/PDFIUM_TESTS.md`](testing/PDFIUM_TESTS.md) — проверка PDFium.
- [`testing/AUDIT_STAGE10.md`](testing/AUDIT_STAGE10.md) — аудит и план.
- [`../tests/fixtures/3.0/README.md`](../tests/fixtures/3.0/README.md) — тестовые
  файлы 3.0 и чек-лист ручной проверки.

## Дизайн и сборка

- [`design/APPLY_SINGLE_EXE.md`](design/APPLY_SINGLE_EXE.md) — как применялись
  патчи «один exe» (историческая инструкция).
- [`design/SINGLE_EXE_BUILD.md`](design/SINGLE_EXE_BUILD.md) — сборка одного exe
  с PDFium и libmpv.

## История

- [`history/`](history/) — отчёты этапов 11–17 и точечные исправления
  (фокус, окна PowerPoint, видео, фон).
- [`history/patches/`](history/patches/) — патчи этапов 11 и 4.

Это архивные документы. Команды, пути и номера версий в них относятся ко времени
написания; актуальные инструкции — в корневом `README.md` и `PROJECT.md`.

## Backlog

- [`backlog.md`](backlog.md) — что осталось после проверки 3.0.1: квадратики
  вместо символов в тексте и раздельные настройки «свой сервер / чужой сервер».

## Снимки

- [`screenshots/light.png`](screenshots/light.png) — светлая тема.
- [`screenshots/dark.png`](screenshots/dark.png) — тёмная тема.
- [`screenshots/empty.png`](screenshots/empty.png) — пустой список.
- [`screenshots/network_client_connected.png`](screenshots/network_client_connected.png) —
  клиент 3.0.0 подключён к серверу, список и состояние пришли по сети.
- [`screenshots/network_client_next.png`](screenshots/network_client_next.png) —
  после команды «Вперёд» с клиента: слайд 2 из 10.
- [`screenshots/network_server_slide2.png`](screenshots/network_server_slide2.png) —
  сцена сервера после удалённой команды.
