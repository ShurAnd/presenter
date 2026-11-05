# Presentator 2.5.1 — видео над фоном, исправление 2

Заменить все три файла из src в актуальном проекте после предыдущих патчей 2.5.1.
Закройте обе запущенные копии приложения перед запуском нового EXE.

Исправление убирает HWND_TOPMOST у видео. Поток проигрывателя ставит сцену ниже
конкретного активного окна видео. Проверка порядка выполняется также после старта
и на паузе, но SetWindowPos вызывается только если сцена действительно оказалась выше.
При переключении повтора используется HWND нового проигрывателя.
При естественном завершении фон поднимается до опускания видео.

## Установка
Распаковать ZIP во временную папку. В PowerShell из корня проекта:

```powershell
$patch = 'D:\Downloads\Presentator_2.5.1_video_order_fix2'
$backup = Join-Path $PWD ('backup-video-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Path $backup | Out-Null
'ppt.rs', 'stage.rs', 'video.rs' | ForEach-Object {
    Copy-Item (Join-Path '.\src' $_) $backup
    Copy-Item (Join-Path "$patch\src" $_) '.\src' -Force
}
cargo build --release
```
Указать в $patch свою папку распакованного архива. Запускать именно новый EXE из target\release.

## Проверки

```powershell
cargo test --all-targets
$env:PRESENTATOR_STAGE12_NATIVE = 'I_UNDERSTAND'
try {
    cargo test --all-targets stage18_native_video_stays_above_stage_and_loop_spare -- --ignored --nocapture --test-threads=1
} finally {
    Remove-Item Env:\PRESENTATOR_STAGE12_NATIVE -ErrorAction SilentlyContinue
}
```

Нативный тест создаёт небольшие окна за пределами экрана и проверяет настоящие вызовы Win32.
Проверяет перекрытие сценой TOPMOST, поздний подъём сцены, смену окна при повторе и возврат фона.
Он не проверяет изображение, которое выводит libmpv/MF, и не заменяет просмотр на вашем компьютере.

Ручная проверка на втором мониторе:
1. Запустить видео впервые и убедиться, что виден кадр.
2. Пауза и продолжение.
3. Дождаться трёх циклов повторения — фон не должен мигать на стыке.
4. Закрыть видео и проверить возврат фона. Отдельно проверить естественный конец без повтора.
5. Повторить с двумя копиями приложения на одном ПК.

В среде подготовки нет Windows и компилятора Rust. Сборка и Rust-тесты здесь не запускались.
