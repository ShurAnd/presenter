#Requires -Version 5.1
<#
 Stage 14: merged coverage from safe default tests and optionally selected native
 PowerPoint/video tests and real localhost WebSocket stress. No global --ignored.
 Requires cargo-llvm-cov + rustup llvm-tools-preview, MSVC Windows toolchain.
 Example:
   powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\coverage_report_stage14.ps1
   powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\coverage_report_stage14.ps1 -Native -IUnderstandWindowAutomation
   powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\coverage_report_stage14.ps1 -Native -IUnderstandWindowAutomation -StressLoopback
#>
param(
    [switch]$Native,
    [switch]$IUnderstandWindowAutomation,
    [switch]$StressLoopback,
    [switch]$Branch,
    [switch]$OpenReport,
    [ValidateRange(1,240)][int]$TimeoutMinutes = 20
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'stage13_process.ps1')
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $root
try {
    if (-not (Test-Path -LiteralPath 'Cargo.toml')) { throw 'Cargo.toml not found.' }
    if (-not (Test-Path -LiteralPath 'vendor\pdfium\pdfium.dll')) {
        throw 'Missing vendor\pdfium\pdfium.dll. Run scripts\prepare_pdfium.ps1 first.'
    }
    $cargoPath = (Get-Command cargo.exe -ErrorAction Stop).Source
    if (-not (Get-Command rustup.exe -ErrorAction SilentlyContinue)) { throw 'rustup not found.' }
    if (-not (Get-Command cargo-llvm-cov -ErrorAction SilentlyContinue)) {
        throw 'cargo-llvm-cov missing. Install: cargo install cargo-llvm-cov --locked'
    }
    $rustcVersion = ((& rustc --version) | Out-String).Trim()
    $rustcHost = ((& rustc -vV) | Select-String '^host:').ToString()
    if ($rustcHost -match 'x86_64-pc-windows-gnu') {
        throw 'Coverage is not supported reliably for Windows GNU: use an MSVC Rust toolchain.'
    }
    if ($Branch -and $rustcVersion -notmatch 'nightly') {
        throw 'Branch coverage is unstable and requires nightly Rust; omit -Branch for normal stable coverage.'
    }
    $installed = @(& rustup component list --installed)
    if ($LASTEXITCODE -ne 0 -or -not ($installed | Select-String 'llvm-tools')) {
        throw 'Install LLVM tools: rustup component add llvm-tools-preview'
    }
    if ($Native) { Assert-Stage13NativePreflight -IUnderstandWindowAutomation:$IUnderstandWindowAutomation }

    $out = Join-Path $root 'target\presentator-coverage'
    # Never mix current logs/HTML with an older run (especially when -Native differs).
    if (Test-Path -LiteralPath $out) { Remove-Item -LiteralPath $out -Recurse -Force }
    New-Item -ItemType Directory -Path $out -Force | Out-Null
    $logs = Join-Path $out 'stage14-logs'
    New-Item -ItemType Directory -Path $logs -Force | Out-Null
    $flags = @()
    if ($Branch) { $flags += '--branch' }

    # Important: clear old profiles exactly once. --no-report retains the
    # coverage artifacts for the next selected test run, allowing them to merge.
    Invoke-Stage13Process -Label 'coverage-clean' -FilePath $cargoPath `
        -Arguments @('llvm-cov','clean','--workspace') -ReportDir $logs -TimeoutMinutes $TimeoutMinutes
    Invoke-Stage13Process -Label 'safe-tests' -FilePath $cargoPath `
        -Arguments (@('llvm-cov','--all-targets','--no-report') + $flags) `
        -ReportDir $logs -TimeoutMinutes $TimeoutMinutes
    $runs = @('safe-default')
    if ($StressLoopback) {
        Invoke-Stage13Process -Label 'localhost-reconnect-stress' -FilePath $cargoPath `
            -Arguments (@('llvm-cov','--all-targets','--no-report') + $flags +
                @('--','stress_loopback_repeated_connect_and_ping','--ignored','--nocapture')) `
            -ReportDir $logs -TimeoutMinutes $TimeoutMinutes
        $runs += 'localhost-50-reconnects'
    }
    if ($Native) {
        $oldNative = [Environment]::GetEnvironmentVariable('PRESENTATOR_STAGE12_NATIVE','Process')
        $env:PRESENTATOR_STAGE12_NATIVE = 'I_UNDERSTAND'
        try {
            foreach ($filter in @(
                'stage12_native_ppt_three_slides_navigation_and_close',
                'stage12_native_ppt_missing_file_then_recover',
                'stage14_native_ppt_process_termination_then_reopen',
                'stage12_native_video_play_end_rearm_and_replay',
                'stage14_native_video_invalid_media_then_valid_recovery'
            )) {
                Invoke-Stage13Process -Label $filter -FilePath $cargoPath `
                    -Arguments (@('llvm-cov','--all-targets','--no-report') + $flags +
                        @('--',$filter,'--ignored','--nocapture')) `
                    -ReportDir $logs -TimeoutMinutes $TimeoutMinutes
                $runs += $filter
            }
        } finally {
            if ($null -eq $oldNative) { Remove-Item Env:\PRESENTATOR_STAGE12_NATIVE -ErrorAction SilentlyContinue }
            else { $env:PRESENTATOR_STAGE12_NATIVE = $oldNative }
        }
    }
    Invoke-Stage13Process -Label 'report-text' -FilePath $cargoPath `
        -Arguments (@('llvm-cov','report','--text') + $flags) -ReportDir $logs -TimeoutMinutes $TimeoutMinutes
    Invoke-Stage13Process -Label 'report-html' -FilePath $cargoPath `
        -Arguments (@('llvm-cov','report','--html','--output-dir',$out) + $flags) `
        -ReportDir $logs -TimeoutMinutes $TimeoutMinutes
    Invoke-Stage13Process -Label 'report-json' -FilePath $cargoPath `
        -Arguments (@('llvm-cov','report','--json','--output-path',(Join-Path $out 'summary.json')) + $flags) `
        -ReportDir $logs -TimeoutMinutes $TimeoutMinutes
    Copy-Item -LiteralPath (Join-Path $logs 'report-text.stdout.log') `
        -Destination (Join-Path $out 'summary.txt') -Force
    @(
        'Presentator merged stage 14 coverage',
        "Rust: $rustcVersion", "Host: $rustcHost", "Native: $Native", "StressLoopback: $StressLoopback",
        "Branch: $Branch", 'Executed test groups:', ($runs | ForEach-Object { ' - ' + $_ })
    ) | Set-Content -LiteralPath (Join-Path $out 'test-runs.txt') -Encoding UTF8
    $coverage = Get-Content -LiteralPath (Join-Path $out 'summary.json') -Raw | ConvertFrom-Json
    $totals = $coverage.data[0].totals
    if ($null -eq $totals) { throw 'Coverage JSON has no totals section; refusing to publish an incomplete report.' }
    $branchCount = [int]$totals.branches.'count'
    $branchStatus = if (-not $Branch) {
        'not collected (use nightly Rust and -Branch; 0/0 is unavailable, not 0 percent)'
    } elseif ($branchCount -eq 0) {
        'requested but no counters returned (branch coverage is unavailable, not 0 percent)'
    } else {
        'measured'
    }
    $branchMetric = if ($branchCount -eq 0) {
        "Branches: unavailable (0/0 counters) — $branchStatus"
    } else {
        ("Branches: {0}/{1} ({2:n2}%) — {3}" -f $totals.branches.covered, $totals.branches.count, $totals.branches.percent, $branchStatus)
    }
    @(
        'Presentator 2.4.1 — Stage 14 coverage metrics',
        ("Lines: {0}/{1} ({2:n2}%)" -f $totals.lines.covered, $totals.lines.count, $totals.lines.percent),
        ("Functions: {0}/{1} ({2:n2}%)" -f $totals.functions.covered, $totals.functions.count, $totals.functions.percent),
        ("Regions: {0}/{1} ({2:n2}%)" -f $totals.regions.covered, $totals.regions.count, $totals.regions.percent),
        $branchMetric
    ) | Set-Content -LiteralPath (Join-Path $out 'coverage-metrics.txt') -Encoding UTF8
    $html = Join-Path $out 'html\index.html'
    if (-not (Test-Path -LiteralPath $html)) { throw "HTML report missing: $html" }
    $zip = Join-Path $root 'target\Presentator_stage14_coverage.zip'
    if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
    Compress-Archive -Path @(
        (Join-Path $out 'html'), (Join-Path $out 'summary.txt'),
        (Join-Path $out 'summary.json'), (Join-Path $out 'test-runs.txt'),
        (Join-Path $out 'coverage-metrics.txt'), $logs
    ) -DestinationPath $zip -CompressionLevel Optimal
    Write-Host "`nPASS. Combined report: $html"
    Write-Host "Upload this ZIP: $zip"
    Write-Host 'Do not treat ignored tests as passed. A 0/0 branch metric means branch coverage was not measured.'
    if ($OpenReport) { Invoke-Item -LiteralPath $html }
} finally {
    Pop-Location
}
