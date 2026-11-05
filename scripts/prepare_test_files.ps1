# Пересоздаёт тестовые файлы Presentator 3.0.
# Запуск из корня репозитория:
#   powershell -ExecutionPolicy Bypass -File scripts\prepare_test_files.ps1
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$venv = Join-Path $root '.venv-testfiles'
$python = Join-Path $venv 'Scripts\python.exe'

if (-not (Test-Path $python)) {
    Write-Host "Создаю окружение $venv…"
    $launcher = if (Get-Command python -ErrorAction SilentlyContinue) { 'python' } elseif (Get-Command py -ErrorAction SilentlyContinue) { 'py' } else { throw 'не найден Python (python или py)' }
    & $launcher -m venv $venv
}

& $python -m pip install --disable-pip-version-check --quiet pillow python-pptx reportlab imageio-ffmpeg
if ($LASTEXITCODE -ne 0) { throw 'не удалось установить зависимости для тестовых файлов' }

& $python (Join-Path $root 'scripts\make_test_files.py')
if ($LASTEXITCODE -ne 0) { throw 'генератор тестовых файлов завершился с ошибкой' }
