# Применение доработки PDFium «один EXE»

## Как выбрать патч

- **Если ранее PDFium-патч УЖЕ применён**, используйте `presentator_single_exe.patch`.
- **Если исправления ещё НЕ применялись**, используйте `presentator_all_in_one.patch`:
  он включает контроль фокуса PowerPoint, PDFium и встраивание PDFium в EXE.
- **Если ранее установлен только контроль фокуса**, используйте
  `presentator_pdfium_single_exe_after_focus.patch`.

## Установка

Выполнять только в **полном рабочем репозитории** Presentator. Не поверх неполного
AI-экспорта, из которого были исключены бинарные ресурсы.

```powershell
git switch -c feature/pdfium-single-exe
git apply --check .\presentator_all_in_one.patch
git apply .\presentator_all_in_one.patch
powershell -ExecutionPolicy Bypass -File .\scripts\build_release.ps1
```

Вместо `presentator_all_in_one.patch` подставьте нужный вариант.
Не применяйте патчи несколько раз и не смешивайте их друг с другом.
При ошибке `git apply --check` не продолжайте: пришлите сообщение об ошибке,
чтобы сверить патч с фактическими исходниками.

**На компьютерах показа и управления** нужно распространять только
`target\release\presentator.exe` — без `pdfium.dll` и без установщика.
Внутренний кэш `%LOCALAPPDATA%\Presentator` приложение создаст само.

Подробная проверка PDF: `PDFIUM_TESTS.md`.
