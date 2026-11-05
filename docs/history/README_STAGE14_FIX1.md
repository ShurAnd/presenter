# Presentator 2.4.1 — Stage 14 fix 1

Применить поверх установленного этапа 14, распаковав ZIP в корень проекта с заменой двух файлов src.

Исправлена проверка ожидания событий после Host::restart: события известного предыдущего SID пропускаются в пределах того же 30-секундного срока. Любой другой неожиданный SID и ошибка нового SID по-прежнему приводят к падению теста. После Ready нового показа дополнительно проверяется переход на слайд 2.

Добавлены две обычные проверки ожидания событий: устаревшие Ready/Error не заменяют новый Ready; ошибка нового показа не пропускается. Убран лишний импорт Duration из render.rs.

В PowerShell:

```powershell
cd D:\rust_projects\presenter
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify_stage14.ps1
```

На Windows ожидается 374 passed, 25 ignored; это прогноз до выполнения на вашей машине.

После успешного обычного прогона закройте PowerPoint и на тестовом рабочем столе выполните:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\coverage_report_stage14.ps1 -Native -IUnderstandWindowAutomation -StressLoopback
```

Пришлите target\Presentator_stage14_coverage.zip либо логи первого упавшего теста. Проверка восстановления завершает процесс PowerPoint, запущенный тестом, поэтому запускать только вне живого показа.

Сборка и Windows-тесты в среде подготовки не выполнены: cargo/Windows PowerPoint недоступны. Производственное поведение приложения этим исправлением не изменяется.
