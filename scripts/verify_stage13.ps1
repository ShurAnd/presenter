#Requires -Version 5.1
<#
 Stage 13 regression gate. Windows PowerShell 5.1.
 Default tests do not manipulate registry, run PowerPoint or require screen access.
 Real native PowerPoint/video tests require explicit opt-in on a test machine.
 Unlike stage 12's first variant, this runner shows a heartbeat and limits waits.
#>
param(
    [switch]$Native,
    [switch]$IUnderstandWindowAutomation,
    [switch]$StressLoopback,
    [ValidateRange(1,240)][int]$TimeoutMinutes = 20
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'stage13_process.ps1')
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$reportDir = Join-Path ([System.IO.Path]::GetTempPath()) ('Presentator-stage13-test-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Path $reportDir -Force | Out-Null
Push-Location $root
try {
    if (-not (Test-Path -LiteralPath 'Cargo.toml')) { throw 'Cargo.toml not found.' }
    $cargoPath = (Get-Command cargo.exe -ErrorAction Stop).Source
    if ($Native) { Assert-Stage13NativePreflight -IUnderstandWindowAutomation:$IUnderstandWindowAutomation }
    Invoke-Stage13Process -Label 'all-default-tests' -FilePath $cargoPath -Arguments @('test','--all-targets') `
        -ReportDir $reportDir -TimeoutMinutes $TimeoutMinutes
    if ($StressLoopback) {
        Invoke-Stage13Process -Label 'localhost-50-reconnects' -FilePath $cargoPath `
            -Arguments @('test','--all-targets','stress_loopback_repeated_connect_and_ping','--','--ignored','--nocapture') `
            -ReportDir $reportDir -TimeoutMinutes $TimeoutMinutes
    }
    if ($Native) {
        $oldNative = [Environment]::GetEnvironmentVariable('PRESENTATOR_STAGE12_NATIVE', 'Process')
        $env:PRESENTATOR_STAGE12_NATIVE = 'I_UNDERSTAND'
        try {
            foreach ($filter in @(
                'stage12_native_ppt_three_slides_navigation_and_close',
                'stage12_native_ppt_missing_file_then_recover',
                'stage12_native_video_play_end_rearm_and_replay'
            )) {
                Invoke-Stage13Process -Label $filter -FilePath $cargoPath `
                    -Arguments @('test','--all-targets',$filter,'--','--ignored','--nocapture') `
                    -ReportDir $reportDir -TimeoutMinutes $TimeoutMinutes
            }
        } finally {
            if ($null -eq $oldNative) { Remove-Item Env:\PRESENTATOR_STAGE12_NATIVE -ErrorAction SilentlyContinue }
            else { $env:PRESENTATOR_STAGE12_NATIVE = $oldNative }
        }
    }
    Write-Host "`nPASS. Test logs: $reportDir"
} finally {
    Pop-Location
}
