#Requires -Version 5.1
# Shared runner for stage 13. Dot-source from a script; do not invoke directly.
# PowerShell 5.1 can turn native stderr into an ErrorRecord. Redirect to files instead.
function Invoke-Stage13Process {
    param(
        [Parameter(Mandatory=$true)][string]$Label,
        [Parameter(Mandatory=$true)][string]$FilePath,
        [Parameter(Mandatory=$true)][string[]]$Arguments,
        [Parameter(Mandatory=$true)][string]$ReportDir,
        [ValidateRange(1,240)][int]$TimeoutMinutes = 20
    )
    $stdout = Join-Path $ReportDir "$Label.stdout.log"
    $stderr = Join-Path $ReportDir "$Label.stderr.log"
    # Start-Process joins -ArgumentList values without quoting each one.
    # Quote only arguments requiring it, including paths under a spaced project root.
    $commandLine = ($Arguments | ForEach-Object {
        if ($_ -match '[\s"]') { '"' + ($_.Replace('"', '\"')) + '"' } else { $_ }
    }) -join ' '
    Write-Host "`n=== $Label ==="
    Write-Host "Command: $FilePath $commandLine"
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    $process = Start-Process -FilePath $FilePath -ArgumentList $commandLine `
        -WorkingDirectory (Get-Location).Path -NoNewWindow -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    # PowerShell 5.1 Start-Process can release the process handle before ExitCode is read.
    # Force the .NET handle to be acquired while the process is still tracked.
    $null = $process.Handle
    $lastProgress = 0.0
    $timedOut = $false
    while (-not $process.WaitForExit(1000)) {
        if ($timer.Elapsed.TotalMinutes -ge $TimeoutMinutes) {
            $timedOut = $true
            Write-Warning "$Label exceeded $TimeoutMinutes minute(s); stopping its process tree."
            try {
                $kill = Start-Process -FilePath 'taskkill.exe' -ArgumentList "/PID $($process.Id) /T /F" `
                    -NoNewWindow -PassThru -Wait -RedirectStandardOutput (Join-Path $ReportDir "$Label.taskkill.out.log") `
                    -RedirectStandardError (Join-Path $ReportDir "$Label.taskkill.err.log")
            } catch {
                try { $process.Kill() } catch { }
            }
            break
        }
        if ($timer.Elapsed.TotalSeconds - $lastProgress -ge 15) {
            $lastProgress = $timer.Elapsed.TotalSeconds
            Write-Host ("  Still running: {0:n0} seconds. Logs: {1}" -f $lastProgress, $ReportDir)
            if ((Test-Path -LiteralPath $stdout) -and ((Get-Item $stdout).Length -gt 0)) {
                Get-Content -LiteralPath $stdout -Tail 1 | ForEach-Object { Write-Host "  Latest: $_" }
            }
        }
    }
    if ($timedOut) { throw "TIMEOUT: $Label; inspect $stdout and $stderr" }
    # Wait specifically for the cargo process, not the whole descendant tree.
    $process.WaitForExit()
    $process.Refresh()
    $code = $process.ExitCode
    if ($null -eq $code) {
        throw "UNKNOWN EXIT CODE: $Label. Cargo may have completed, but PowerShell did not provide a process exit status. Inspect $stdout and $stderr"
    }
    "Exit code: $code" | Set-Content -LiteralPath (Join-Path $ReportDir "$Label.exit.txt") -Encoding UTF8
    if ($code -eq 0) {
        if (Test-Path -LiteralPath $stdout) {
            Get-Content -LiteralPath $stdout -Tail 5 | ForEach-Object { Write-Host $_ }
        }
    } else {
        if (Test-Path -LiteralPath $stdout) {
            Get-Content -LiteralPath $stdout -Tail 60 | ForEach-Object { Write-Host $_ }
        }
        if (Test-Path -LiteralPath $stderr) {
            Get-Content -LiteralPath $stderr -Tail 60 | ForEach-Object { Write-Host $_ }
        }
        throw "FAILED: $Label (exit $code). Logs: $stdout ; $stderr"
    }
    Write-Host ("OK: {0}, exit 0, {1:n1}s" -f $Label, $timer.Elapsed.TotalSeconds)
}

function Assert-Stage13NativePreflight {
    param([switch]$IUnderstandWindowAutomation)
    if (-not $IUnderstandWindowAutomation) {
        throw 'Native checks need -Native -IUnderstandWindowAutomation, on a test desktop only.'
    }
    if (-not [Environment]::UserInteractive) { throw 'Interactive Windows desktop required for native checks.' }
    if (@(Get-Process POWERPNT -ErrorAction SilentlyContinue).Count -ne 0) {
        throw 'PowerPoint is already running. Close it before running native checks on a test desktop.'
    }
    if (-not (Test-Path -LiteralPath 'tests\fixtures\stage12_video.mp4') -or
        -not (Test-Path -LiteralPath 'tests\fixtures\stage12_three_slides.pptx')) {
        throw 'Stage 12 native fixtures are missing. Restore the stage 12 fixture files.'
    }
}
