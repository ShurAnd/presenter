#Requires -Version 5.1
<#
Stage 16 regression gate. Default run is safe and does not start PowerPoint/video.
Native recovery tests require explicit opt-in on an isolated Windows test desktop.
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
$reportDir = Join-Path ([System.IO.Path]::GetTempPath()) ('Presentator-stage16-test-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Path $reportDir -Force | Out-Null
Push-Location $root
try {
    if (-not (Test-Path -LiteralPath 'Cargo.toml')) { throw 'Cargo.toml not found.' }
    $cargoPath = (Get-Command cargo.exe -ErrorAction Stop).Source
    if ($Native) { Assert-Stage13NativePreflight -IUnderstandWindowAutomation:$IUnderstandWindowAutomation }

    Invoke-Stage13Process -Label 'all-default-tests' -FilePath $cargoPath `
        -Arguments @('test','--all-targets') -ReportDir $reportDir -TimeoutMinutes $TimeoutMinutes

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
                'stage14_native_ppt_process_termination_then_reopen',
                'stage15_native_ppt_three_restarts_keep_navigation_working',
                'stage12_native_video_play_end_rearm_and_replay',
                'stage14_native_video_invalid_media_then_valid_recovery',
                'stage15_native_video_pause_seek_resume_then_finish',
                'stage16_native_video_repeat_loops_then_stops_cleanly'
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

    @(
        'Presentator 2.4.1 Stage 16 test run',
        "Native: $Native",
        "StressLoopback: $StressLoopback",
        'Executed default tests and explicitly selected optional groups only.'
    ) | Set-Content -LiteralPath (Join-Path $reportDir 'stage16-test-runs.txt') -Encoding UTF8
    Write-Host "`nPASS. Stage 16 logs: $reportDir"
} finally {
    Pop-Location
}
