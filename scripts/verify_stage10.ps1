#Requires -Version 5.1
<#
Presentator: regression gate, phase 11.
Run from any directory with a Windows PowerShell 5.1+ installation.
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify_stage10.ps1
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify_stage10.ps1 -Coverage
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify_stage10.ps1 -StressLoopback

Do not run all ignored tests on the operator workstation.
#>
param(
    [switch]$Coverage,
    [switch]$StressLoopback
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$reportDir = Join-Path ([System.IO.Path]::GetTempPath()) "Presentator-stage11-$stamp"
New-Item -ItemType Directory -Path $reportDir -Force | Out-Null

function Invoke-LoggedProcess {
    param(
        [string]$Label,
        [string]$FilePath,
        [string[]]$Arguments
    )
    $outPath = Join-Path $reportDir "$Label.stdout.log"
    $errPath = Join-Path $reportDir "$Label.stderr.log"
    Write-Host "`n=== $Label ==="
    # Start-Process does not turn native stderr into PowerShell ErrorRecord.
    # Our fixed CLI arguments contain no whitespace or quotes; keep the
    # command line explicit to avoid PowerShell 5.1 argument-array surprises.
    $commandLine = $Arguments -join ' '
    $proc = Start-Process -FilePath $FilePath -ArgumentList $commandLine `
        -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $outPath -RedirectStandardError $errPath
    if (Test-Path -LiteralPath $outPath) {
        Get-Content -LiteralPath $outPath | ForEach-Object { Write-Host $_ }
    }
    if (Test-Path -LiteralPath $errPath) {
        Get-Content -LiteralPath $errPath | ForEach-Object { Write-Host $_ }
    }
    if ($proc.ExitCode -ne 0) {
        throw "FAILED: $Label (exit $($proc.ExitCode)). Logs: $outPath ; $errPath"
    }
}

Push-Location $root
try {
    if (-not (Test-Path -LiteralPath 'Cargo.toml')) {
        throw 'Cargo.toml missing: this is not the Presentator repository.'
    }
    $cargo = Get-Command cargo.exe -ErrorAction Stop
    (& $cargo.Source --version) | Set-Content -Path (Join-Path $reportDir 'cargo-version.txt') -Encoding UTF8
    if (Get-Command git -ErrorAction SilentlyContinue) {
        (& git status --short) | Set-Content -Path (Join-Path $reportDir 'git-status.txt') -Encoding UTF8
        (& git rev-parse HEAD) | Set-Content -Path (Join-Path $reportDir 'git-head.txt') -Encoding UTF8
    }
    Invoke-LoggedProcess 'test-inventory' $cargo.Source @('test','--all-targets','--','--list')
    Invoke-LoggedProcess 'all-default-tests' $cargo.Source @('test','--all-targets')
    Invoke-LoggedProcess 'stage11-real-websocket' $cargo.Source @('test','--all-targets','stage11_','--','--nocapture')
    Invoke-LoggedProcess 'upload-size-regression' $cargo.Source @('test','--all-targets','known_issue_upload_must_not_exceed_declared_size_on_disk','--','--nocapture')
    if ($StressLoopback) {
        Invoke-LoggedProcess 'localhost-reconnect-stress' $cargo.Source @('test','--all-targets','stress_loopback_','--','--ignored','--nocapture')
    }
    if ($Coverage) {
        $verify = Join-Path $PSScriptRoot 'verify_tests.ps1'
        if (-not (Test-Path -LiteralPath $verify)) { throw "Missing coverage script: $verify" }
        # Child Powershell launch with the script path quoted even if the
        # project root is under a directory with spaces.
        $arguments = '-NoProfile -ExecutionPolicy Bypass -File "' + $verify + '" -Coverage'
        Invoke-LoggedProcess 'instrumented-coverage' 'powershell.exe' @($arguments)
    }
    Write-Host "`nPASS: checks finished. Logs: $reportDir"
    Write-Host 'Ignored tests were not treated as passed.'
} finally {
    Pop-Location
}
