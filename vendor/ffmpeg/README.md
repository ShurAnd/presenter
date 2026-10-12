# FFmpeg для записи мероприятия

`ffmpeg.zip` содержит `ffmpeg.exe` — им пишется видео и звук на вкладке «Запись».
Архив вшивается в exe (`include_bytes!` в `src/record.rs`); при первой записи (или первом
открытии вкладки) `ffmpeg.exe` распаковывается в `%LOCALAPPDATA%\Presentator\ffmpeg-<версия>\`
и запускается отдельным процессом без окна.

| | |
|---|---|
| Источник | https://github.com/BtbN/FFmpeg-Builds/releases (тег `latest`, 11.10.2026) |
| Файл | `ffmpeg-n8.1-latest-win64-lgpl-8.1.zip` (SHA-256 `b71cdee0f657737c0a1be21627a7bd1ee2a4872c4e046c3c017ce608fdbbbcad`) |
| Версия | `n8.1.3-17-gc389840894-20261011` |
| `ffmpeg.exe` | 134 092 288 байт, SHA-256 `f78e7aa28a9694e2aca4517de0c9636b463a933a43abe15bbe69557cfef06d4e` |
| `ffmpeg.zip` | `7z a -tzip -mx9`, 53 320 560 байт |
| Лицензия | LGPL-3.0 (сборка `lgpl`: без x264/x265 и других GPL-частей). H.264 сжимают NVENC, QuickSync, AMF, Media Foundation или OpenH264 |

Обновление: скачать новый `ffmpeg-n*-latest-win64-lgpl-*.zip`, упаковать `bin/ffmpeg.exe` в zip
(`7z a -tzip -mx9`), поменять `FF_TAG` и `FF_EXE_SIZE` в `src/record.rs` и таблицу выше.
