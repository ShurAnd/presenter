#Requires -Version 5.1
<#
    Загружает PDFium 7881 для СБОРКИ Presentator, не для распространения.
    После компиляции pdfium.dll будет встроена в presentator.exe.
    На компьютере показа файл DLL рядом с EXE НЕ нужен.
#>
param([switch]$Force)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$vendor = Join-Path $root 'vendor\pdfium'
$destination = Join-Path $vendor 'pdfium.dll'
$version = '7881'
$url = 'https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881/pdfium-win-x64.tgz'

if ((Test-Path -LiteralPath $destination -PathType Leaf) -and -not $Force) {
    $existing = Get-Item -LiteralPath $destination
    if ($existing.Length -gt 1048576) {
        Write-Host "PDFium $version уже подготовлен для сборки: $destination"
        exit 0
    }
    throw "Похоже, $destination повреждён. Запустите prepare_pdfium.ps1 -Force"
}

New-Item -ItemType Directory -Force -Path $vendor | Out-Null
$tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("presentator-pdfium-build-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $tempDir | Out-Null
try {
    $archive = Join-Path $tempDir 'pdfium-win-x64.tgz'
    Write-Host "Загружаю PDFium $version (Windows x64) из закреплённого релиза..."
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing

    & "$env:SystemRoot\System32\tar.exe" -xzf $archive -C $tempDir
    if ($LASTEXITCODE -ne 0) { throw "Не удалось распаковать архив PDFium" }

    $dll = Get-ChildItem -LiteralPath $tempDir -Filter 'pdfium.dll' -File -Recurse |
        Select-Object -First 1
    if ($null -eq $dll -or $dll.Length -lt 1048576) {
        throw "В PDFium $version не найдена корректная x64 DLL"
    }
    $header = [System.IO.File]::ReadAllBytes($dll.FullName)
    if ($header[0] -ne 77 -or $header[1] -ne 90) {
        throw "Неверная сигнатура pdfium.dll"
    }
    Copy-Item -LiteralPath $dll.FullName -Destination $destination -Force
    $sha = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash
    Write-Host "PDFium готов для встраивания в EXE. SHA256: $sha"
} finally {
    Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}
