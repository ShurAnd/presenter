#Requires -Version 5.1
<#
  Безопасная проверка Presentator 2.4.1.
  Запускать в Windows PowerShell из Git Bash или PowerShell.
  По умолчанию: только автоматические тесты (никаких команд реальному серверу).
  -LoopbackOnly: проверка реального WebSocket по адресу 127.0.0.1 со случайным портом.
  -StressLoopback: отдельный ограниченный тест 50 последовательных переподключений.
  -Coverage: строит HTML-отчёт безопасных автоматических тестов (нужен cargo-llvm-cov).

  Примеры:
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File ./scripts/verify_tests.ps1
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File ./scripts/verify_tests.ps1 -BuildRelease
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File ./scripts/verify_tests.ps1 -LoopbackOnly
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File ./scripts/verify_tests.ps1 -StressLoopback
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File ./scripts/verify_tests.ps1 -Coverage
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File ./scripts/verify_tests.ps1 -PdfPath 'C:\test\complex.pdf'
#>
param(
    [switch]$BuildRelease,
    [switch]$LoopbackOnly,
    [switch]$StressLoopback,
    [switch]$Coverage,
    [string]$PdfPath = ''
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $root
try {
    if (-not (Test-Path -LiteralPath 'Cargo.toml')) {
        throw 'Cargo.toml не найден в корне проекта.'
    }
    if (-not (Test-Path -LiteralPath 'vendor\pdfium\pdfium.dll')) {
        throw 'PDFium для сборки отсутствует. Сначала выполните scripts\build_release.ps1.'
    }
    if ($Coverage) {
        if ($LoopbackOnly -or $StressLoopback -or $PdfPath -ne '') {
            throw 'Use -Coverage separately from -LoopbackOnly, -StressLoopback and -PdfPath.'
        }
        & (Join-Path $PSScriptRoot 'coverage_report.ps1')
        if ($LASTEXITCODE -ne 0) { throw 'Coverage generation failed.' }
    } elseif ($StressLoopback) {
        Write-Host '=== Presentator: bounded localhost reconnect stress test ==='
        & cargo test --all-targets stress_loopback_ -- --ignored --nocapture
    } elseif ($LoopbackOnly) {
        Write-Host '=== Presentator: безопасные WebSocket-тесты (localhost) ==='
        & cargo test --all-targets loopback_tests:: -- --nocapture
    } else {
        Write-Host '=== Presentator: cargo test --all-targets ==='
        & cargo test --all-targets
    }
    if ($LASTEXITCODE -ne 0) { throw 'Автоматические тесты не прошли.' }

    if ($PdfPath -ne '') {
        $file = (Resolve-Path -LiteralPath $PdfPath -ErrorAction Stop).Path
        $env:TEST_PDF = $file
        try {
            Write-Host '=== PDFium: отдельный дымовой тест реального PDF ==='
            & cargo test --release pdfium_smoke -- --ignored --nocapture
            if ($LASTEXITCODE -ne 0) { throw 'Проверка PDFium не прошла.' }

            Write-Host '=== Hayro: проверка увеличенного фрагмента PDF ==='
            & cargo test --release zoom_region_matches -- --ignored --nocapture
            if ($LASTEXITCODE -ne 0) { throw 'Проверка увеличения Hayro не прошла.' }
        } finally {
            Remove-Item Env:\TEST_PDF -ErrorAction SilentlyContinue
        }
    }

    if ($BuildRelease) {
        Write-Host '=== Release: cargo build --release ==='
        & cargo build --release
        if ($LASTEXITCODE -ne 0) { throw 'Release-сборка не удалась.' }
        if (-not (Test-Path -LiteralPath 'target\release\presentator.exe')) {
            throw 'Готовый EXE не найден в target\release.'
        }
    }

    Write-Host 'УСПЕШНО: выбранные проверки завершены.'
} finally {
    Pop-Location
}
