# Presentator 2.4.1 — этап 16

Распаковать ZIP поверх актуального проекта после этапа 15. Заменяются только исходники тестов и два скрипта этапа 16; production-код и зависимости не меняются. Создайте резервную копию перед распаковкой.

## Проверки

- PDF/рендеринг: повторные ошибки страницы, рендер фрагмента и целой страницы, а также восстановление миниатюры после ошибочного номера страницы.
- PowerPoint COM: новый Windows тест удерживает постоянно занятый COM-сервер в состоянии retry и проверяет, что ограничение около пяти секунд возвращает последнее сообщение ошибки. Может добавить около 5 секунд к безопасному прогону.
- Видео: нативный тест подтверждает переход позиции назад при включённом повторе, отключает повтор и проверяет обычное событие Ended.
- Сохранены этапы 14–15, включая проверку трёх перезапусков PowerPoint и управления перемоткой видео.

## Запуск в PowerShell

```powershell
cd D:\rust_projects\presenter
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify_stage16.ps1
```

Ожидаемое число безопасных тестов для Windows: 377 passed, 0 failed, 28 ignored. Это ожидаемое значение; Rust-компиляция пакета в среде подготовки не выполнялась.

Для полного покрытия ветвей закройте PowerPoint и запустите на отдельном Windows тестовом рабочем столе:

```powershell
rustup run nightly powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\coverage_report_stage16.ps1 -Branch -Native -IUnderstandWindowAutomation -StressLoopback
```

Скрипт запускает базовые тесты, 50 localhost переподключений и восемь выбранных нативных тестов. Нужны существующие `tests\fixtures\stage12_three_slides.pptx`, `tests\fixtures\stage12_video.mp4`, `vendor\pdfium\pdfium.dll`, nightly с llvm-tools-preview и cargo-llvm-cov. Графические приложения запускаются нативными тестами, автоматизации мыши и клавиатуры нет.

Архив результата: `target\Presentator_stage16_coverage.zip`. При ошибке пришлите архив и логи из `target\presentator-coverage\stage16-logs`.
