# Mitts Build — a real project on Linux, macOS or Windows, Mitts against Cargo as the reference.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Cargo runs here only as the reference Mitts is compared with (owner decision 2026-11-25,
# AGENTS.md). For one checked-out project:
#   1. `build --all-targets` through both, into separate target directories: the same
#      executables and shared libraries must come out;
#   2. the project's binary (if named) prints the same `--version` from both;
#   3. `test --no-fail-fast` through both: Mitts may fail no test Cargo passes, and runs as
#      many as Cargo does;
#   4. the same build again through Mitts compiles nothing, and a deleted target directory
#      is restored from the cache.
param(
    [Parameter(Mandatory = $true)][string]$Project,
    [Parameter(Mandatory = $true)][string]$Mitts,
    [string]$Bin = '',
    [string]$BuildArgs = '',
    [switch]$SkipTests
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$Project = (Resolve-Path $Project).Path
$Mitts = (Resolve-Path $Mitts).Path
$work = Join-Path $env:RUNNER_TEMP 'project-check'
New-Item -ItemType Directory -Force -Path $work | Out-Null
$cargoTarget = Join-Path $work 'target-cargo'
$mittsTarget = Join-Path $work 'target-mitts'
$env:MITTS_CACHE_DIR = Join-Path $work 'mitts-cache'
$env:CARGO_TERM_COLOR = 'never'
# Every input of every unit key, in the logs kept as artifacts: a unit an unchanged rebuild
# recompiles is explained by the inputs that differ between two runs.
$env:MITTS_LOG_KEYS = '1'
$failures = @()
function Check([bool]$ok, [string]$what) {
    if ($ok) { Write-Host "  ok: $what" } else { Write-Host "  FAIL: $what"; $script:failures += $what }
}

$locked = @()
if (Test-Path (Join-Path $Project 'Cargo.lock')) { $locked = @('--locked') }
$extra = @()
if ($BuildArgs) { $extra = @($BuildArgs -split ' ' | Where-Object { $_ }) }

# Runs `tool args...` in the project; returns @{ Code; Text; Seconds }.
function Run([string]$tool, [string[]]$arguments, [string]$log) {
    Push-Location $Project
    $started = Get-Date
    try {
        $ErrorActionPreference = 'Continue'
        $text = & $tool @arguments 2>&1 | ForEach-Object { "$_" }
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = 'Stop'
        Pop-Location
    }
    $seconds = [int]((Get-Date) - $started).TotalSeconds
    $text | Set-Content -LiteralPath (Join-Path $work $log)
    Write-Host "  $tool $($arguments -join ' ') -> exit $code in ${seconds}s"
    return @{ Code = $code; Text = ($text -join "`n"); Seconds = $seconds }
}

# What a user runs or loads: executables and shared libraries, by the platform's spelling.
# On Unix an executable has no extension and its execute bit set (rustc's dep-info `.d`
# and the `.rlib` files are not products).
function IsProduct([System.IO.FileInfo]$file) {
    if ($file.Name.StartsWith('.') -or $file.Name -match '-[0-9a-f]{16}(\.|$)') { return $false }
    if ($IsWindows) { return $file.Extension -in '.exe', '.dll' }
    if ($file.Extension -in '.so', '.dylib') { return $true }
    return ($file.Extension -eq '') -and (($file.UnixFileMode -band [System.IO.UnixFileMode]::UserExecute) -ne 0)
}

function Products([string]$target) {
    $dir = Join-Path $target 'debug'
    $names = @()
    foreach ($sub in @('', 'examples')) {
        $d = Join-Path $dir $sub
        if (Test-Path $d) {
            $names += Get-ChildItem -LiteralPath $d -File |
                Where-Object { IsProduct $_ } |
                ForEach-Object { if ($sub) { "$sub/$($_.Name)" } else { $_.Name } }
        }
    }
    return @($names | Sort-Object -Unique)
}

Write-Host "== build --all-targets $BuildArgs"
$c = Run 'cargo' (@('build', '-v', '--all-targets', '--target-dir', $cargoTarget) + $locked + $extra) 'cargo-build.log'
$m = Run $Mitts (@('build', '-v', '--all-targets', '--target-dir', $mittsTarget) + $locked + $extra) 'mitts-build.log'
foreach ($side in @(@('Cargo', $c), @('Mitts', $m))) {
    if ($side[1].Code -ne 0) {
        Write-Host "  --- $($side[0]) output, last lines ---"
        Write-Host ($side[1].Text -split "`n" | Where-Object { $_ -notmatch '^\s*(Running|Fresh|Compiling|Dirty) ' } |
                Select-Object -Last 40 | Out-String)
    }
}
if ($m.Code -ne 0) {
    # For every crate rustc could not find: the --extern path it was given, whether that file
    # exists now, and what the shared artifacts directory holds under that crate's name.
    $artifacts = Join-Path (Join-Path $env:MITTS_CACHE_DIR 'cas') 'artifacts'
    foreach ($crate in @([regex]::Matches($m.Text, "can't find crate for ``([A-Za-z0-9_]+)``") | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)) {
        $given = @([regex]::Matches($m.Text, "--extern '$crate=([^']+)'") | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
        foreach ($path in $given) { Write-Host "  extern $crate -> $path (exists: $(Test-Path -LiteralPath $path))" }
        $present = @(Get-ChildItem -LiteralPath $artifacts -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -like "*lib$crate-*" -or $_.Name -like ".tmp_$crate*" } | ForEach-Object { "$($_.Name) ($($_.Length) bytes)" })
        Write-Host "  in artifacts: $($present -join ', ')"
    }
    # The first failed build-script compile, run again now that every artifact exists: a
    # success means it ran too early; the same error means rustc rejects what it was given.
    $line = @($m.Text -split "`n" | Where-Object { $_ -match "Running ``'.*--crate-name build_script_build '$([regex]::Escape($Project))" }) | Select-Object -First 1
    if ($line) {
        $quoted = $line.Substring($line.IndexOf('`') + 1).TrimEnd().TrimEnd('`')
        $words = @([regex]::Matches($quoted, "'([^']*)'|(\S+)") | ForEach-Object { if ($_.Groups[1].Success) { $_.Groups[1].Value } else { $_.Groups[2].Value } })
        $exe = $words[0]
        $rest = @($words | Select-Object -Skip 1)
        $out = Join-Path $work 'rerun-build-script'
        New-Item -ItemType Directory -Force -Path $out | Out-Null
        $at = [array]::IndexOf($rest, '-o')
        if ($at -ge 0) { $rest[$at + 1] = Join-Path $out 'build_script_build' }
        Push-Location $Project
        $ErrorActionPreference = 'Continue'
        $again = & $exe @rest 2>&1 | ForEach-Object { "$_" }
        $code = $LASTEXITCODE
        $ErrorActionPreference = 'Stop'
        Pop-Location
        Write-Host "  build script compile run again: exit $code"
        $again | Select-Object -First 30 | ForEach-Object { Write-Host "    | $_" }
        # Variants that tell what rustc rejects.
        $deps = Join-Path (Join-Path $mittsTarget 'debug') 'deps'
        $variants = [ordered]@{
            'plus -L target deps' = @($rest + @('-L', "dependency=$deps"))
            'externs as .rmeta'   = @($rest | ForEach-Object { if ($_ -match '^[A-Za-z0-9_]+=.*\.rlib$') { $_ -replace '\.rlib$', '.rmeta' } else { $_ } })
            'externs from target deps' = @($rest | ForEach-Object {
                    if ($_ -match '^([A-Za-z0-9_]+)=.*[\\/]lib([A-Za-z0-9_]+)-[0-9a-f]+\.rlib$') {
                        $found = @(Get-ChildItem -LiteralPath $deps -Filter "lib$($Matches[2])-*.rlib" -ErrorAction SilentlyContinue | Select-Object -First 1)
                        if ($found.Count -eq 1) { "$($Matches[1])=$($found[0].FullName)" } else { $_ }
                    } else { $_ } })
        }
        foreach ($name in $variants.Keys) {
            Push-Location $Project
            $ErrorActionPreference = 'Continue'
            $vout = & $exe @($variants[$name]) 2>&1 | ForEach-Object { "$_" } | Where-Object { $_ -match '^error' }
            $vcode = $LASTEXITCODE
            $ErrorActionPreference = 'Stop'
            Pop-Location
            Write-Host "  variant '$name': exit $vcode $(@($vout | Select-Object -First 3) -join ' / ')"
        }
    }
}
Check ($c.Code -eq 0) 'Cargo builds it (reference sanity)'
Check ($m.Code -eq $c.Code) "Mitts exits as Cargo ($($m.Code) vs $($c.Code))"
$cp = @(Products $cargoTarget)
$mp = @(Products $mittsTarget)
$missing = @($cp | Where-Object { $_ -notin $mp })
$surplus = @($mp | Where-Object { $_ -notin $cp })
Write-Host "  products: $($cp.Count) from Cargo, $($mp.Count) from Mitts"
Check ($missing.Count -eq 0) "no executable or shared library missing (missing: $($missing -join ', '))"
Check ($surplus.Count -eq 0) "no extra executable or shared library (extra: $($surplus -join ', '))"

if ($Bin) {
    Write-Host "== $Bin --version"
    # A build timestamp differs between any two builds (shadow-rs's build_time).
    $exe = if ($IsWindows) { "$Bin.exe" } else { $Bin }
    # A binary a failed build did not leave is reported, not run.
    function Version([string]$target) {
        $path = Join-Path (Join-Path $target 'debug') $exe
        if (-not (Test-Path -LiteralPath $path)) { return "(no $exe)" }
        return (& $path --version 2>&1 | Where-Object { "$_" -notmatch '^build_time:' } | Out-String).Trim()
    }
    $cv = Version $cargoTarget
    $mv = Version $mittsTarget
    Write-Host "  cargo: $cv"
    Write-Host "  mitts: $mv"
    Check ($cv -eq $mv) 'the same output from both builds'
}

if (-not $SkipTests) {
    Write-Host '== test --no-fail-fast'
    $ct = Run 'cargo' (@('test', '--no-fail-fast', '--target-dir', $cargoTarget) + $locked + $extra) 'cargo-test.log'
    $mt = Run $Mitts (@('test', '--no-fail-fast', '--target-dir', $mittsTarget) + $locked + $extra) 'mitts-test.log'
    function Summary([string]$text) {
        $passed = 0; $failed = 0
        foreach ($match in [regex]::Matches($text, 'test result: \w+\. (\d+) passed; (\d+) failed')) {
            $passed += [int]$match.Groups[1].Value; $failed += [int]$match.Groups[2].Value
        }
        $names = @([regex]::Matches($text, '(?m)^test (\S+) \.\.\. FAILED') | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
        return @{ Passed = $passed; Failed = $failed; Names = $names }
    }
    $cs = Summary $ct.Text
    $ms = Summary $mt.Text
    Write-Host "  cargo: $($cs.Passed) passed, $($cs.Failed) failed; mitts: $($ms.Passed) passed, $($ms.Failed) failed"
    $onlyMitts = @($ms.Names | Where-Object { $_ -notin $cs.Names })
    Check ($onlyMitts.Count -eq 0) "no test fails under Mitts alone ($($onlyMitts -join ', '))"
    Check ($ms.Passed + $ms.Failed -eq $cs.Passed + $cs.Failed) 'Mitts runs as many tests as Cargo'
    if ($cs.Failed -gt 0) { Write-Host "  (fails under Cargo too: $($cs.Names -join ', '))" }
}

Write-Host '== cache'
$again = Run $Mitts (@('build', '-v', '--all-targets', '--target-dir', $mittsTarget) + $locked + $extra) 'mitts-noop.log'
$recompiled = @([regex]::Matches($again.Text, '\[mitts::compile\] package=\S+ target=\S+ kind=\S+') | ForEach-Object { $_.Value })
if ($recompiled.Count -gt 0) { Write-Host "  recompiled: $($recompiled -join '; ')" }
$stats = ([regex]::Matches($again.Text, '\[mitts::stats\][^\r\n]*') | Select-Object -Last 1).Value
Write-Host "  no-op: $stats"
Check (($again.Code -eq 0) -and $stats -match 'compiled=0 ') 'an unchanged rebuild compiles nothing'
Remove-Item -Recurse -Force $mittsTarget
$warm = Run $Mitts (@('build', '-v', '--all-targets', '--target-dir', $mittsTarget) + $locked + $extra) 'mitts-warm.log'
$stats = ([regex]::Matches($warm.Text, '\[mitts::stats\][^\r\n]*') | Select-Object -Last 1).Value
Write-Host "  fresh target, warm cache: $stats ($($warm.Seconds)s; cold Mitts $($m.Seconds)s, Cargo $($c.Seconds)s)"
Check ($warm.Code -eq 0) 'a deleted target directory rebuilds from the cache'
Check (@(Products $mittsTarget).Count -eq $mp.Count) 'with every executable and shared library back'

if ($failures.Count -gt 0) {
    Write-Host "FAILED ($($failures.Count)):"; $failures | ForEach-Object { Write-Host "  - $_" }
    exit 1
}
Write-Host 'PROJECT MATCHES CARGO'
exit 0
