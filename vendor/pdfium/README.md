# PDFium для одной исполняемой сборки Presentator

В режиме Windows x64 `pdfium.dll` **встраивается в EXE** через `include_bytes!`.
Запустите `scripts/build_release.ps1`: скрипт автоматически скачает официальную
PDFium 7881 (`pdfium-win-x64.tgz`) при первой сборке и вызовет Cargo.

Исходный релиз: https://github.com/bblanchon/pdfium-binaries/releases/tag/chromium%2F7881

Поставляемый файл: **только** `target/release/presentator.exe`.
На компьютере показа не нужно помещать DLL рядом с EXE, иметь интернет или
выполнять установку PDFium. При первом запуске приложение автоматически
распакует встроенную DLL в каталог `%LOCALAPPDATA%\Presentator\pdfium-7881-...`.
Если пользовательскому профилю запрещена запись в этот каталог, PDFium не
загрузится, но Hayro останется запасным вариантом.

Лицензии PDFium и сторонних компонентов необходимо учитывать при распространении.
