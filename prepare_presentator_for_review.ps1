# Presentator source and test evidence collector (Windows PowerShell 5.1+)
param(
    [string]$ProjectDir = 'C:\rust\presenter',
    [string]$OutputDir = '',
    [switch]$IncludeCoverage,
    [switch]$RunTests
)
$ErrorActionPreference = 'Stop'
$ProjectDir = (Resolve-Path -LiteralPath $ProjectDir).Path
if (-not $OutputDir) { $OutputDir = Join-Path $ProjectDir 'review_packages' }
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$OutputDir = (Resolve-Path -LiteralPath $OutputDir).Path
$stamp = Get-Date -Format 'yyyyMMdd_HHmmss'
$stage = Join-Path $env:TEMP ('presentator_review_' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $stage | Out-Null
$logDir = Join-Path $stage 'diagnostics'
New-Item -ItemType Directory -Force -Path $logDir | Out-Null

function Copy-RelativeFile([string]$relative) {
    $src = Join-Path $ProjectDir $relative
    if (Test-Path -LiteralPath $src -PathType Leaf) {
        $dest = Join-Path $stage $relative
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest) | Out-Null
        Copy-Item -LiteralPath $src -Destination $dest -Force
    }
}
function Copy-SourceTree([string]$relative, [string[]]$extensions) {
    $root = Join-Path $ProjectDir $relative
    if (-not (Test-Path -LiteralPath $root -PathType Container)) { return }
    Get-ChildItem -LiteralPath $root -File -Recurse -ErrorAction SilentlyContinue | ForEach-Object {
        if ($extensions -contains $_.Extension.ToLowerInvariant()) {
            $rel = $_.FullName.Substring($ProjectDir.Length).TrimStart('\','/')
            Copy-RelativeFile $rel
        }
    }
}

try {
    # Sources, test infrastructure and manifests only; no executable binaries or user media.
    Copy-SourceTree 'src' @('.rs')
    Copy-SourceTree 'tests' @('.rs','.json','.toml','.md')
    Copy-SourceTree 'benches' @('.rs')
    Copy-SourceTree 'scripts' @('.ps1','.cmd','.bat','.py','.md')
    Copy-SourceTree '.cargo' @('.toml')
    foreach ($file in @('Cargo.toml','Cargo.lock','rust-toolchain.toml','rust-toolchain','build.rs','README.md','README_STAGE11.md','.gitignore')) { Copy-RelativeFile $file }

    $info = @(
        'Collected: ' + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss zzz'),
        'Project: ' + $ProjectDir,
        'Windows: ' + [Environment]::OSVersion.VersionString,
        'PowerShell: ' + $PSVersionTable.PSVersion.ToString(),
        'Computer name and username intentionally excluded.',
        'Secrets, personal config, media, target/, .git/, .env and binary assets intentionally excluded.'
    )
    $info | Set-Content -LiteralPath (Join-Path $logDir 'environment.txt') -Encoding UTF8
    if (Get-Command cargo -ErrorAction SilentlyContinue) {
        (& cargo --version 2>&1 | Out-String) | Add-Content -LiteralPath (Join-Path $logDir 'environment.txt')
    }
    if (Get-Command rustc -ErrorAction SilentlyContinue) {
        (& rustc --version 2>&1 | Out-String) | Add-Content -LiteralPath (Join-Path $logDir 'environment.txt')
    }
    if (Get-Command git -ErrorAction SilentlyContinue) {
        Push-Location $ProjectDir
        try {
            (& git status --short 2>$null | Out-String) | Set-Content -LiteralPath (Join-Path $logDir 'git_status.txt') -Encoding UTF8
            (& git log -1 --format='%h %s' 2>$null | Out-String) | Set-Content -LiteralPath (Join-Path $logDir 'git_revision.txt') -Encoding UTF8
        } finally { Pop-Location }
    }

    if ($RunTests) {
        if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'cargo not found in PATH' }
        Push-Location $ProjectDir
        try {
            $stdout = Join-Path $logDir 'cargo_test_stdout.log'
            $stderr = Join-Path $logDir 'cargo_test_stderr.log'
            $p = Start-Process -FilePath 'cargo.exe' -ArgumentList @('test','--all-targets') -NoNewWindow -Wait -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
            ('cargo test --all-targets exit code: ' + $p.ExitCode) | Set-Content -LiteralPath (Join-Path $logDir 'test_exit_code.txt')
            Write-Host ('Test exit code: ' + $p.ExitCode)
        } finally { Pop-Location }
    }

    if ($IncludeCoverage) {
        # Existing report only. No tool invocation, no overwrite of existing report.
        $candidates = @('coverage','coverage-report','coverage_report','target\llvm-cov','presentator-coverage')
        foreach ($c in $candidates) {
            $dir = Join-Path $ProjectDir $c
            if (Test-Path -LiteralPath $dir -PathType Container) {
                Get-ChildItem -LiteralPath $dir -Recurse -File -ErrorAction SilentlyContinue |
                    Where-Object { $_.Extension -in @('.json','.txt','.info','.lcov','.md') -and $_.Length -lt 20MB } |
                    ForEach-Object {
                        $rel = $_.FullName.Substring($ProjectDir.Length).TrimStart('\','/')
                        Copy-RelativeFile $rel
                    }
            }
        }
        Get-ChildItem -LiteralPath $ProjectDir -File -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match 'coverage|llvm-cov' -and $_.Extension -in @('.json','.txt','.info','.lcov') -and $_.Length -lt 20MB } |
            ForEach-Object { Copy-RelativeFile $_.Name }
    }

    @'
This archive contains Rust sources, test code, build manifests, and optional diagnostics.
It intentionally excludes: target/, .git/, user configuration, credentials, .env files,
PowerPoint/PDF/video/media assets and other binary files.
Please inspect the ZIP before uploading if your source contains private addresses,
passwords, tokens, proprietary code, or other confidential information.
For review, also attach the latest presentator-coverage ZIP separately.
'@ | Set-Content -LiteralPath (Join-Path $stage 'PACKAGE_README.txt') -Encoding UTF8

    $zip = Join-Path $OutputDir ('Presentator_review_' + $stamp + '.zip')
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal -Force
    Write-Host "Package created: $zip" -ForegroundColor Green
    Write-Host 'Upload this ZIP and your latest coverage report ZIP to the chat.'
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
