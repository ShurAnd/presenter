# libmpv для встроенного видеопроигрывателя

`libmpv-2.zip` содержит `libmpv-2.dll` — движок проигрывателя mpv со встроенным FFmpeg.
Архив вшивается в exe (`include_bytes!` в `src/mpv.rs`); при первом показе видео DLL
распаковывается в `%LOCALAPPDATA%\Presentator\libmpv-<версия>\`.

| | |
|---|---|
| Источник | https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20260924 |
| Файл | `mpv-dev-x86_64-20260924-git-2a4eb8067c.7z` (SHA-256 `50caf10ddb6644da4c205ce67dfe37e32fc3715b33c82e933d6e1962ff61eac9`) |
| `libmpv-2.dll` | 120 797 184 байт, SHA-256 `d2d081547efea8a1530f4ba0bf58a0d72069b25c40d8b2a7071cea5aa82381c6` |
| API | libmpv client API 2.5 |
| Лицензия | mpv — LGPL-2.1+ / GPL-2.0+ (сборка shinchiro включает GPL-компоненты), FFmpeg — LGPL/GPL |

Обновление: скачать новый `mpv-dev-x86_64-*.7z`, упаковать `libmpv-2.dll` в zip (`7z a -tzip -mx9`),
поменять `MPV_TAG`, `MPV_DLL_SIZE` в `src/mpv.rs` и таблицу выше.
