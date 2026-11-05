# Stage 17: диагностика HWND PowerPoint

Этот пакет применяется поверх stage17_fix2_ppt_hwnd. Он не объявляет проблему решённой: запасной поиск на вашей машине тоже вернул 0. Пакет добавляет вывод ошибки COM, записанного PID и списка окон PowerPoint непосредственно в stdout/stderr нативного теста. Если класс оконного показа отличается или PID неверен, вывод это покажет. Проверка ненулевого HWND сохранена.

Распакуйте src/ppt.rs поверх проекта с заменой. Выполните из папки проекта:

```powershell
$env:PRESENTATOR_STAGE12_NATIVE = 'I_UNDERSTAND'
cargo test --all-targets stage12_native_ppt_three_slides_navigation_and_close -- --ignored --nocapture 2>&1 | Tee-Object -FilePath .\ppt-hwnd-diagnostic.txt
Remove-Item Env:\PRESENTATOR_STAGE12_NATIVE
```

Пришлите ppt-hwnd-diagnostic.txt. Повторное падение ожидаемо; полезны строки PPT HWND COM ERROR, PPT HWND FALLBACK, PPT HWND DIAGNOSTICS и PPT WINDOW. Заголовки окон PowerPoint могут содержать названия открытых файлов.

Сборка и нативный запуск в среде подготовки недоступны: нет Rust/Cargo и Windows/Office. Проверен состав и целостность ZIP.
