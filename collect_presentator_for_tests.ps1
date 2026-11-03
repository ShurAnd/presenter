# ASCII-only script for Windows PowerShell 5.1 and PowerShell 7.
# Run from project root. Produces a sanitized ZIP for test development.
param(
    [string]$ProjectRoot = (Get-Location).Path,
    [string]$OutputPath = "",
    [switch]$IncludeLockfile
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $ProjectRoot).Path
if (-not (Test-Path -LiteralPath (Join-Path $root 'Cargo.toml'))) {
    throw 'Cargo.toml not found. Run from the Presentator project root.'
}
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $root 'presentator_test_sources.zip'
}
$OutputPath = [IO.Path]::GetFullPath($OutputPath)
$stage = Join-Path ([IO.Path]::GetTempPath()) ('presentator_tests_' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage -Force | Out-Null
$excludedDirs = @('.git','target','node_modules','.idea','.vscode','vendor','dist','build','release','out','cache','logs','tmp','temp')
$excludedExt = @('.exe','.dll','.pdb','.lib','.a','.so','.dylib','.zip','.7z','.rar','.tar','.tgz','.gz','.pdf','.ppt','.pptx','.mp4','.mkv','.mov','.avi','.jpg','.jpeg','.png','.webp','.gif','.bmp','.ico','.wav','.mp3','.ogg','.sqlite','.db','.pem','.pfx','.p12','.key','.cer','.crt')
$allowedExt = @('.rs','.toml','.md','.txt','.ps1','.sh','.yml','.yaml','.json','.lock','.gitignore','.gitattributes','.bat','.cmd','.config')
$maxBytes = 2MB
$count = 0
$skipped = New-Object System.Collections.Generic.List[string]
try {
    $files = Get-ChildItem -LiteralPath $root -File -Recurse -Force -ErrorAction SilentlyContinue
    foreach ($file in $files) {
        $rel = $file.FullName.Substring($root.Length).TrimStart('\','/')
        $parts = $rel -split '[\\/]'
        if (@($parts | Where-Object { $excludedDirs -contains $_ }).Count -gt 0) { continue }
        if ($file.FullName -eq $OutputPath) { continue }
        if ($file.Length -gt $maxBytes) { $skipped.Add("oversize: $rel"); continue }
        $ext = $file.Extension.ToLowerInvariant()
        if ($excludedExt -contains $ext) { continue }
        if ($allowedExt -notcontains $ext -and $file.Name -notin @('Dockerfile','Makefile','rust-toolchain','rust-toolchain.toml')) { continue }
        if ($file.Name -match '(?i)(secret|credential|token|password|private|\.env|id_rsa|id_ed25519)') { $skipped.Add("sensitive-name: $rel"); continue }
        if ($file.Name -eq 'Cargo.lock' -and -not $IncludeLockfile) { continue }
        # Avoid accidental inclusion of application/user settings or external dependencies.
        if ($parts.Count -gt 1 -and $parts[0] -notin @('src','tests','benches','examples','scripts','.cargo','assets','docs','testdata','fixtures')) { continue }
        if ($parts[0] -eq 'assets' -and $ext -notin @('.rs','.toml','.md','.txt','.json')) { continue }
        if ($parts[0] -eq 'testdata' -or $parts[0] -eq 'fixtures') {
            if ($ext -notin @('.rs','.toml','.md','.txt','.json')) { continue }
        }
        # Skip files that appear to contain credentials; do not attempt to redact source automatically.
        $text = [IO.File]::ReadAllText($file.FullName)
        if ($text -match '(?im)(-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----|ghp_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,}|AKIA[0-9A-Z]{16})') {
            $skipped.Add("possible-secret: $rel"); continue
        }
        $dest = Join-Path $stage $rel
        $parent = Split-Path -Parent $dest
        if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
        Copy-Item -LiteralPath $file.FullName -Destination $dest
        $count++
    }
    $report = New-Object System.Collections.Generic.List[string]
    $report.Add('Presentator test development snapshot')
    $report.Add('Collected files: ' + $count)
    $report.Add('Project path intentionally omitted from report.')
    $report.Add('No binary assets, user presentations, vendor dependencies, Git history, or build outputs.')
    $report.Add('')
    foreach ($command in @('rustc','cargo')) {
        $found = Get-Command $command -ErrorAction SilentlyContinue
        if ($found) {
            try { $v = (& $command --version 2>&1 | Out-String).Trim(); $report.Add($v) } catch { $report.Add("$command version unavailable") }
        } else { $report.Add("$command not installed") }
    }
    $report.Add('')
    $report.Add('Files included:')
    Get-ChildItem -LiteralPath $stage -File -Recurse | ForEach-Object {
        $report.Add($_.FullName.Substring($stage.Length).TrimStart('\','/'))
    }
    $report.Add('')
    $report.Add('Skipped files (limited to 100 entries):')
    $skipped | Select-Object -First 100 | ForEach-Object { $report.Add($_) }
    [IO.File]::WriteAllLines((Join-Path $stage 'SNAPSHOT_REPORT.txt'), $report, [Text.Encoding]::UTF8)
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    if (Test-Path -LiteralPath $OutputPath) { Remove-Item -LiteralPath $OutputPath -Force }
    [IO.Compression.ZipFile]::CreateFromDirectory($stage, $OutputPath, [IO.Compression.CompressionLevel]::Optimal, $false)
    Write-Host "Created: $OutputPath"
    Write-Host "Source files: $count"
    Write-Host 'Review ZIP contents before sharing, especially configuration and source files.'
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
