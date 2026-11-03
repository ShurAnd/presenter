#Requires -Version 5.1
<#
Stage 12. Default: all safe tests. -Native: additional opt-in real PowerPoint/video tests.
Never run on an operator PC during a real show.
#>
param(
    [switch]$Native,
    [switch]$IUnderstandWindowAutomation
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$reportDir = Join-Path ([System.IO.Path]::GetTempPath()) ('Presentator-stage12-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Path $reportDir -Force | Out-Null

function RunCargo {
    param([string]$Label, [string]$CargoArguments)
    $stdout = Join-Path $reportDir ($Label + '.stdout.log')
    $stderr = Join-Path $reportDir ($Label + '.stderr.log')
    Write-Host "`n=== $Label ==="
    $process = Start-Process -FilePath $script:cargoPath -ArgumentList $CargoArguments -PassThru -NoNewWindow -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    # Wait for cargo.exe itself, not every descendant process (Start-Process -Wait waits for the process tree).
    $process.WaitForExit()
    $process.Refresh()
    $code = $process.ExitCode
    if (Test-Path -LiteralPath $stdout) { Get-Content -LiteralPath $stdout | ForEach-Object { Write-Host $_ } }
    if (Test-Path -LiteralPath $stderr) { Get-Content -LiteralPath $stderr | ForEach-Object { Write-Host $_ } }
    "$Label exit code: $code" | Set-Content -Path (Join-Path $reportDir ($Label + '.exit.txt')) -Encoding UTF8
    if ($code -ne 0) { throw "FAILED: $Label (exit $code); logs: $reportDir" }
}

Push-Location $root
try {
    if (-not (Test-Path -LiteralPath 'Cargo.toml')) { throw 'Cargo.toml missing.' }
    if (-not (Test-Path -LiteralPath 'tests\fixtures\stage12_video.mp4')) { throw 'Stage 12 media fixtures missing. Unpack the ZIP at project root.' }
    $script:cargoPath = (Get-Command cargo.exe -ErrorAction Stop).Source
    RunCargo 'all-default-tests' 'test --all-targets'
    RunCargo 'stage12-com-regressions' 'test --all-targets stage12_com_ -- --nocapture'
    if ($Native) {
        if (-not $IUnderstandWindowAutomation) {
            throw 'Native tests open PowerPoint and Windows video windows. Re-run with both -Native -IUnderstandWindowAutomation on a test account.'
        }
        $pptProcesses = @(Get-Process -Name POWERPNT -ErrorAction SilentlyContinue)
        if ($pptProcesses.Count -gt 0) {
            throw 'PowerPoint is already running. Close all PowerPoint windows/processes and retry on a test workstation.'
        }
        if (-not [Environment]::UserInteractive) { throw 'Native tests need an interactive desktop session.' }
        $env:PRESENTATOR_STAGE12_NATIVE = 'I_UNDERSTAND'
        try {
            RunCargo 'native-ppt-navigation' 'test --all-targets stage12_native_ppt_three_slides_navigation_and_close -- --ignored --nocapture'
            RunCargo 'native-ppt-recovery' 'test --all-targets stage12_native_ppt_missing_file_then_recover -- --ignored --nocapture'
            RunCargo 'native-video-replay' 'test --all-targets stage12_native_video_play_end_rearm_and_replay -- --ignored --nocapture'
        } finally {
            Remove-Item Env:\PRESENTATOR_STAGE12_NATIVE -ErrorAction SilentlyContinue
        }
    }
    Write-Host "`nPASS. Logs: $reportDir"
} finally {
    Pop-Location
}
