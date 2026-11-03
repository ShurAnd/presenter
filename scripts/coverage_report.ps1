#Requires -Version 5.1
<#
  Generates an HTML coverage report for safe (non-ignored) Presentator tests.
  Does not connect to the live Presentator server, PowerPoint or a remote PC.
  Requires cargo-llvm-cov and the rustup llvm-tools-preview component.
#>
param([switch]$OpenReport)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $projectRoot
try {
    if (-not (Test-Path -LiteralPath 'Cargo.toml')) { throw 'Cargo.toml not found.' }
    if (-not (Test-Path -LiteralPath 'vendor\pdfium\pdfium.dll')) {
        throw 'Missing vendor\pdfium\pdfium.dll. Run scripts\prepare_pdfium.ps1 first.'
    }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'Cargo not found on PATH.' }
    if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) { throw 'rustup not found on PATH.' }
    $hostLine = (& rustc -vV | Select-String '^host: ').ToString()
    if ($hostLine -match 'x86_64-pc-windows-gnu') {
        throw ('LLVM coverage is not reliably supported for this GNU Windows target: ' + $hostLine + '. Use a supported MSVC toolchain for coverage.')
    }
    $components = @(& rustup component list --installed)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot query rustup components.' }
    if (-not ($components | Select-String -Pattern 'llvm-tools')) {
        throw 'Missing LLVM tools. Install once: rustup component add llvm-tools-preview'
    }
    if (-not (Get-Command cargo-llvm-cov -ErrorAction SilentlyContinue)) {
        throw 'Missing cargo-llvm-cov. Install once (with internet): cargo install cargo-llvm-cov --locked'
    }

    $out = Join-Path $projectRoot 'target\presentator-coverage'
    Write-Host '=== Coverage: safe non-ignored cargo tests, all targets ==='
    & cargo llvm-cov --all-targets --html --output-dir $out
    if ($LASTEXITCODE -ne 0) { throw 'Coverage tests or report generation failed.' }

    Write-Host '=== Coverage: summary by source file ==='
    & cargo llvm-cov report
    if ($LASTEXITCODE -ne 0) { throw 'Could not read coverage summary.' }

    $html = Join-Path $out 'html\index.html'
    if (-not (Test-Path -LiteralPath $html)) {
        throw ('Coverage tool did not create expected HTML file: ' + $html)
    }
    Write-Host ('Coverage HTML: ' + $html)
    Write-Host 'NOTE: coverage % is not the percentage of tests passed.'
    if ($OpenReport) { Invoke-Item -LiteralPath $html }
} finally {
    Pop-Location
}
