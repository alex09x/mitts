# Mitts Build — acceptance tests for site/install.ps1 on a Windows runner.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Runs in the PowerShell that launches it (Windows PowerShell 5.1 or PowerShell 7) and
# starts every installer run in a fresh process of that same PowerShell, piping the script
# into Invoke-Expression under the Restricted execution policy, as `irm ... | iex` does.
# Installs the latest and the previous stable release from GitHub. With -Native, the
# runner's PowerShell is not x86-64 (Windows on ARM): only the refusal path is checked.
param(
    [Parameter(Mandatory = $true)][string]$Installer,
    [switch]$Native
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Set-StrictMode -Version Latest

$Installer = (Resolve-Path $Installer).Path
$shell = (Get-Process -Id $PID).Path
$work = Join-Path ([IO.Path]::GetTempPath()) ('mitts-install-test-' + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $work | Out-Null
$script:failures = @()
Write-Host "PowerShell $($PSVersionTable.PSVersion) ($shell), work dir $work"

function Check([bool]$condition, [string]$what) {
    if ($condition) { Write-Host "  ok: $what" }
    else { Write-Host "  FAIL: $what"; $script:failures += $what }
}

# No scenario may quietly fall back to building from source.
$env:MITTS_NO_SOURCE_FALLBACK = '1'
$managed = @('MITTS_VERSION', 'MITTS_INSTALL_ROOT', 'MITTS_ARCHIVE', 'MITTS_NO_SOURCE_FALLBACK',
    'CARGO_INSTALL_ROOT', 'CARGO_HOME', 'USERPROFILE', 'PROCESSOR_ARCHITECTURE', 'PROCESSOR_ARCHITEW6432')

# Runs the installer in a child PowerShell with exactly `$vars` changed (a $null value
# removes the variable). Returns @{ Code; Output; Leak }: Leak lists what the piped
# script left behind in the caller's session.
function Invoke-Installer([hashtable]$vars) {
    # Windows PowerShell 5.1 turns a child's stderr line into a terminating error under Stop.
    $ErrorActionPreference = 'Continue'
    # Through the env: drive, never [Environment]::SetEnvironmentVariable: PowerShell passes
    # $null to it as "" and PowerShell 7 then keeps an empty variable instead of none.
    $saved = @{}
    foreach ($name in $managed) { $saved[$name] = (Get-Item -LiteralPath "env:$name" -ErrorAction SilentlyContinue).Value }
    foreach ($name in @('MITTS_VERSION', 'MITTS_INSTALL_ROOT', 'MITTS_ARCHIVE', 'CARGO_INSTALL_ROOT', 'CARGO_HOME')) {
        Remove-Item -LiteralPath "env:$name" -ErrorAction SilentlyContinue
    }
    foreach ($name in $vars.Keys) {
        if ($null -eq $vars[$name]) { Remove-Item -LiteralPath "env:$name" -ErrorAction SilentlyContinue }
        else { Set-Item -LiteralPath "env:$name" -Value $vars[$name] }
    }
    $probe = Join-Path $work ('probe-' + [Guid]::NewGuid() + '.txt')
    $command = @"
`$ErrorActionPreference = 'Continue'
Get-Content -Raw -LiteralPath '$Installer' | Invoke-Expression
`$leak = @()
foreach (`$n in 'version','root','destination','temporary','target','final','pending') {
    if (Get-Variable -Name `$n -Scope Global -ErrorAction SilentlyContinue) { `$leak += `$n }
}
if (Get-Command Fail -ErrorAction SilentlyContinue) { `$leak += 'function Fail' }
if (`$ErrorActionPreference -ne 'Continue') { `$leak += 'ErrorActionPreference' }
try { `$null = `$notDefinedAnywhere; } catch { `$leak += 'StrictMode' }
Set-Content -LiteralPath '$probe' -Value (`$leak -join ',')
"@
    try {
        $output = & $shell -NoProfile -NonInteractive -ExecutionPolicy Restricted -Command $command 2>&1 |
            ForEach-Object { "$_" }
        $code = $LASTEXITCODE
    } finally {
        foreach ($name in $managed) {
            if ($null -eq $saved[$name]) { Remove-Item -LiteralPath "env:$name" -ErrorAction SilentlyContinue }
            else { Set-Item -LiteralPath "env:$name" -Value $saved[$name] }
        }
    }
    $leak = $null
    if (Test-Path $probe) { $leak = (Get-Content -Raw $probe).Trim() }
    $text = ($output -join "`n")
    Write-Host ($text -split "`n" | ForEach-Object { "    | $_" } | Out-String).TrimEnd()
    return @{ Code = $code; Output = $text; Leak = $leak; Completed = ($null -ne $leak) }
}

function Version-Of([string]$exe) { ((& $exe --version) | Select-Object -First 1) }

if ($Native -and $env:PROCESSOR_ARCHITECTURE -eq 'AMD64') {
    # An x86-64 PowerShell under emulation sees an x86-64 machine: the x86-64 binary must
    # then install and run, under the same emulation.
    Write-Host '== x86-64 PowerShell emulated on ARM64: the x86-64 binary installs and runs'
    $r = Invoke-Installer @{ MITTS_INSTALL_ROOT = (Join-Path $work 'emu') }
    Check $r.Completed 'completes'
    Check ((Version-Of (Join-Path $work 'emu\bin\mitts.exe')) -like 'mitts *') 'mitts.exe runs'
} elseif ($Native) {
    Write-Host "== native $($env:PROCESSOR_ARCHITECTURE): clear refusal without Cargo fallback"
    $r = Invoke-Installer @{ MITTS_INSTALL_ROOT = (Join-Path $work 'arm'); MITTS_NO_SOURCE_FALLBACK = '1' }
    Check (-not $r.Completed) 'the installer stops'
    Check ($r.Output -match "No prebuilt binary for Windows on $env:PROCESSOR_ARCHITECTURE") 'names the architecture'
    Check ($r.Output -match 'MITTS_NO_SOURCE_FALLBACK') 'names the disabled fallback'
    Check (-not (Test-Path (Join-Path $work 'arm\bin\mitts.exe'))) 'installs nothing'
} else {
    $headers = @{ 'User-Agent' = 'mitts-installer-test' }
    if ($env:GITHUB_TOKEN) { $headers['Authorization'] = "Bearer $env:GITHUB_TOKEN" }
    $releases = Invoke-RestMethod -Headers $headers -Uri 'https://api.github.com/repos/alex09x/mitts/releases?per_page=30'
    $stable = @($releases | Where-Object {
            -not $_.draft -and -not $_.prerelease -and $_.tag_name -match '^v[0-9]+\.[0-9]+\.[0-9]+$' -and
            @($_.assets | Where-Object { $_.name -like '*-x86_64-pc-windows-msvc.tar.gz' }).Count -eq 1
        })
    $latest = $stable[0].tag_name
    $previous = $stable[1].tag_name
    Write-Host "latest $latest, previous $previous"

    Write-Host '== default prefix: %USERPROFILE%\.cargo (path with spaces and non-ASCII letters)'
    $profileDir = Join-Path $work ([string]::Concat('User Name ', [char]0x042E, [char]0x0437, [char]0x0435, [char]0x0440))
    New-Item -ItemType Directory -Path $profileDir | Out-Null
    $r = Invoke-Installer @{ USERPROFILE = $profileDir }
    $exe = Join-Path $profileDir '.cargo\bin\mitts.exe'
    Check $r.Completed 'completes'
    Check ($r.Leak -eq '') "leaves nothing in the caller's session (leaked: '$($r.Leak)')"
    Check (Test-Path $exe) "installs $exe"
    Check ((Version-Of $exe) -like "mitts $($latest.Substring(1))*") "is $latest"
    Check ($r.Output -match 'Add .* to PATH') 'says to add the directory to PATH'

    Write-Host '== prefix precedence: MITTS_INSTALL_ROOT > CARGO_INSTALL_ROOT > CARGO_HOME > USERPROFILE'
    $p = @{}
    foreach ($k in 'mitts', 'install', 'home', 'profile') { $p[$k] = Join-Path $work "prec-$k" }
    $all = @{ MITTS_INSTALL_ROOT = $p.mitts; CARGO_INSTALL_ROOT = $p.install; CARGO_HOME = $p.home; USERPROFILE = $p.profile }
    $null = Invoke-Installer $all
    Check (Test-Path "$($p.mitts)\bin\mitts.exe") 'MITTS_INSTALL_ROOT wins'
    $all.Remove('MITTS_INSTALL_ROOT')
    $null = Invoke-Installer $all
    Check (Test-Path "$($p.install)\bin\mitts.exe") 'then CARGO_INSTALL_ROOT'
    $all.Remove('CARGO_INSTALL_ROOT')
    $null = Invoke-Installer $all
    Check (Test-Path "$($p.home)\bin\mitts.exe") 'then CARGO_HOME'
    Check (-not (Test-Path "$($p.profile)\.cargo")) 'USERPROFILE untouched while CARGO_HOME is set'

    Write-Host "== pinned version without the v prefix ($previous)"
    $root = Join-Path $work 'pinned'
    $r = Invoke-Installer @{ MITTS_INSTALL_ROOT = $root; MITTS_VERSION = $previous.Substring(1) }
    $exe = Join-Path $root 'bin\mitts.exe'
    Check ((Version-Of $exe) -like "mitts $($previous.Substring(1))*") "is $previous"

    Write-Host '== on PATH: no PATH hint'
    $oldPath = $env:PATH
    $env:PATH = "$root\bin;$oldPath"
    $r = Invoke-Installer @{ MITTS_INSTALL_ROOT = $root; MITTS_VERSION = $previous }
    $env:PATH = $oldPath
    Check ($r.Output -notmatch 'to PATH') 'no PATH hint when the directory is on PATH'

    Write-Host "== upgrade to $latest while the installed mitts.exe is building"
    $project = Join-Path $work 'hold project'
    New-Item -ItemType Directory -Path (Join-Path $project 'src') | Out-Null
    $marker = Join-Path $work 'build-script-started'
    Set-Content -LiteralPath (Join-Path $project 'Cargo.toml') -Value "[package]`nname = `"hold`"`nversion = `"0.1.0`"`nedition = `"2021`"`n"
    Set-Content -LiteralPath (Join-Path $project 'src\main.rs') -Value 'fn main() { println!("hold ok"); }'
    $buildRs = @'
fn main() {
    std::fs::write(std::env::var("HOLD_MARKER").unwrap(), "started").unwrap();
    std::thread::sleep(std::time::Duration::from_secs(45));
}
'@
    Set-Content -LiteralPath (Join-Path $project 'build.rs') -Value $buildRs
    $env:HOLD_MARKER = $marker
    $env:MITTS_CACHE_DIR = Join-Path $work 'cache'
    $build = Start-Process -FilePath $exe -ArgumentList @('build', '--manifest-path', "`"$project\Cargo.toml`"") `
        -PassThru -NoNewWindow -RedirectStandardError (Join-Path $work 'hold.err') -RedirectStandardOutput (Join-Path $work 'hold.out')
    # Without an open handle, Process.ExitCode is empty once the process has exited.
    $null = $build.Handle
    $deadline = (Get-Date).AddMinutes(5)
    while (-not (Test-Path $marker) -and -not $build.HasExited -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500 }
    Check (Test-Path $marker) 'the build is running (its build script started)'
    $overwrite = $true
    try { Copy-Item -Force (Join-Path $p.mitts 'bin\mitts.exe') $exe } catch { $overwrite = $false }
    Check (-not $overwrite) 'Windows refuses a plain overwrite of the running mitts.exe'
    $r = Invoke-Installer @{ MITTS_INSTALL_ROOT = $root; MITTS_VERSION = $latest }
    Check $r.Completed 'the installer completes'
    Check ((Version-Of $exe) -like "mitts $($latest.Substring(1))*") "mitts.exe is now $latest"
    $build.WaitForExit()
    Check ($build.ExitCode -eq 0) "the running build finished (exit $($build.ExitCode))"
    Write-Host (Get-Content -Raw (Join-Path $work 'hold.err'))
    Check (Test-Path (Join-Path $project 'target\debug\hold.exe')) 'and produced target\debug\hold.exe'
    Check (@(Get-ChildItem (Join-Path $root 'bin') -Filter '.mitts-install-*').Count -eq 0) 'no staging files left'
    $null = Invoke-Installer @{ MITTS_INSTALL_ROOT = $root; MITTS_VERSION = $latest }
    Check (@(Get-ChildItem -Force (Join-Path $root 'bin') -Filter '.mitts-old-*').Count -eq 0) 'the next install removes the old copy'
    Check (@(Get-ChildItem -Force (Join-Path $root 'bin')).Count -eq 1) 'bin holds mitts.exe alone'

    Write-Host '== local archive: verified, and refused on a checksum mismatch'
    $asset = "mitts-$latest-x86_64-pc-windows-msvc.tar.gz"
    $archives = Join-Path $work 'archives'
    New-Item -ItemType Directory -Path $archives | Out-Null
    $archive = Join-Path $archives $asset
    Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/alex09x/mitts/releases/download/$latest/$asset" -OutFile $archive
    $hash = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLowerInvariant()
    Set-Content -LiteralPath "$archive.sha256" -Value "$hash  $asset"
    $local = Join-Path $work 'local'
    $r = Invoke-Installer @{ MITTS_INSTALL_ROOT = $local; MITTS_ARCHIVE = $archive }
    Check ((Version-Of (Join-Path $local 'bin\mitts.exe')) -like "mitts $($latest.Substring(1))*") 'a verified archive installs'
    $before = (Get-FileHash (Join-Path $root 'bin\mitts.exe')).Hash
    Set-Content -LiteralPath "$archive.sha256" -Value ("0" * 64 + "  $asset")
    $r = Invoke-Installer @{ MITTS_INSTALL_ROOT = $root; MITTS_ARCHIVE = $archive }
    Check (-not $r.Completed) 'a mismatching archive stops the installer'
    Check ($r.Output -match 'checksum mismatch') 'and says why'
    Check ((Get-FileHash (Join-Path $root 'bin\mitts.exe')).Hash -eq $before) 'the existing mitts.exe is untouched'

    Write-Host '== refusals'
    $r = Invoke-Installer @{ MITTS_INSTALL_ROOT = (Join-Path $work 'bad'); MITTS_VERSION = 'latest' }
    Check ((-not $r.Completed) -and $r.Output -match 'stable vX.Y.Z') 'MITTS_VERSION=latest is refused'
    $r = Invoke-Installer @{ MITTS_INSTALL_ROOT = (Join-Path $work 'arm'); PROCESSOR_ARCHITECTURE = 'ARM64'; MITTS_NO_SOURCE_FALLBACK = '1' }
    Check ((-not $r.Completed) -and $r.Output -match 'Windows on ARM64') 'ARM64 without fallback is refused by name'
    Check (-not (Test-Path (Join-Path $work 'arm'))) 'and installs nothing'
    $r = Invoke-Installer @{ MITTS_INSTALL_ROOT = (Join-Path $work 'wow'); PROCESSOR_ARCHITECTURE = 'x86'; PROCESSOR_ARCHITEW6432 = 'AMD64' }
    Check (Test-Path (Join-Path $work 'wow\bin\mitts.exe')) '32-bit PowerShell on 64-bit Windows installs the x86-64 binary'
    $r = Invoke-Installer @{ MITTS_INSTALL_ROOT = (Join-Path $work 'noarch'); PROCESSOR_ARCHITECTURE = $null; PROCESSOR_ARCHITEW6432 = $null; MITTS_NO_SOURCE_FALLBACK = '1' }
    Check (Test-Path (Join-Path $work 'noarch\bin\mitts.exe')) 'without the architecture variables .NET tells x86-64'
}

Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
if ($script:failures.Count -gt 0) {
    Write-Host "FAILED ($($script:failures.Count)):"
    $script:failures | ForEach-Object { Write-Host "  - $_" }
    exit 1
}
Write-Host 'ALL INSTALLER CHECKS PASSED'
# Not the exit code of the last installer run that was expected to fail.
exit 0
