#Requires -Version 5.1
<#
    Единая команда для сборки ОДНОГО presentator.exe.
    PDFium скачивается только на компьютере разработчика, если отсутствует.
#>
param([switch]$SkipTests, [switch]$RedownloadPdfium)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $root
try {
    & (Join-Path $PSScriptRoot 'prepare_pdfium.ps1') -Force:$RedownloadPdfium

    if (-not (Test-Path 'vendor\libmpv\libmpv-2.zip')) {
        throw 'Нет vendor\libmpv\libmpv-2.zip. Собирайте в ПОЛНОМ репозитории Presentator, а не в урезанном AI-экспорте.'
    }
    if (-not (Test-Path 'assets\icon_256.png')) {
        throw 'Нет assets\icon_256.png. Используйте полный рабочий проект Presentator.'
    }

    & cargo check --all-targets
    if ($LASTEXITCODE -ne 0) { throw 'cargo check завершился с ошибкой' }
    if (-not $SkipTests) {
        & cargo test
        if ($LASTEXITCODE -ne 0) { throw 'cargo test завершился с ошибкой' }
    }
    & cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'cargo build --release завершился с ошибкой' }

    Write-Host ''
    Write-Host 'Сборка готова: target\release\presentator.exe'
    Write-Host 'Для запуска на компьютере показа нужен ТОЛЬКО этот EXE.'
    Write-Host 'При первом запуске PDFium автоматически извлекается в %LOCALAPPDATA%\Presentator.'
} finally {
    Pop-Location
}
