# Mitts Build — a real project on Windows, Mitts against Cargo as the reference.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Cargo runs here only as the reference Mitts is compared with (owner decision 2026-11-25,
# AGENTS.md). For one checked-out project:
#   1. `build --all-targets` through both, into separate target directories: the same
#      executables and DLLs must come out;
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

function Products([string]$target) {
    $dir = Join-Path $target 'debug'
    $names = @()
    foreach ($sub in @('', 'examples')) {
        $d = Join-Path $dir $sub
        if (Test-Path $d) {
            $names += Get-ChildItem -LiteralPath $d -File |
                Where-Object { $_.Extension -in '.exe', '.dll' -and $_.Name -notmatch '-[0-9a-f]{16}\.' } |
                ForEach-Object { if ($sub) { "$sub/$($_.Name)" } else { $_.Name } }
        }
    }
    return @($names | Sort-Object -Unique)
}

Write-Host "== build --all-targets $BuildArgs"
$c = Run 'cargo' (@('build', '--all-targets', '--target-dir', $cargoTarget) + $locked + $extra) 'cargo-build.log'
$m = Run $Mitts (@('build', '--all-targets', '--target-dir', $mittsTarget) + $locked + $extra) 'mitts-build.log'
Check ($c.Code -eq 0) 'Cargo builds it (reference sanity)'
if ($m.Code -ne 0) { Write-Host ($m.Text -split "`n" | Select-Object -Last 60 | Out-String) }
Check ($m.Code -eq $c.Code) "Mitts exits as Cargo ($($m.Code) vs $($c.Code))"
$cp = Products $cargoTarget
$mp = Products $mittsTarget
$missing = @($cp | Where-Object { $_ -notin $mp })
$surplus = @($mp | Where-Object { $_ -notin $cp })
Write-Host "  products: $($cp.Count) from Cargo, $($mp.Count) from Mitts"
Check ($missing.Count -eq 0) "no executable or DLL missing (missing: $($missing -join ', '))"
Check ($surplus.Count -eq 0) "no extra executable or DLL (extra: $($surplus -join ', '))"

if ($Bin) {
    Write-Host "== $Bin --version"
    $cv = (& (Join-Path $cargoTarget "debug\$Bin.exe") --version 2>&1 | Out-String).Trim()
    $mv = (& (Join-Path $mittsTarget "debug\$Bin.exe") --version 2>&1 | Out-String).Trim()
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
$again = Run $Mitts (@('build', '--all-targets', '--target-dir', $mittsTarget) + $locked + $extra) 'mitts-noop.log'
$stats = ([regex]::Matches($again.Text, '\[mitts::stats\][^\r\n]*') | Select-Object -Last 1).Value
Write-Host "  no-op: $stats"
Check (($again.Code -eq 0) -and $stats -match 'compiled=0 ') 'an unchanged rebuild compiles nothing'
Remove-Item -Recurse -Force $mittsTarget
$warm = Run $Mitts (@('build', '--all-targets', '--target-dir', $mittsTarget) + $locked + $extra) 'mitts-warm.log'
$stats = ([regex]::Matches($warm.Text, '\[mitts::stats\][^\r\n]*') | Select-Object -Last 1).Value
Write-Host "  fresh target, warm cache: $stats ($($warm.Seconds)s; cold Mitts $($m.Seconds)s, Cargo $($c.Seconds)s)"
Check ($warm.Code -eq 0) 'a deleted target directory rebuilds from the cache'
Check ((Products $mittsTarget).Count -eq $mp.Count) 'with every executable and DLL back'

if ($failures.Count -gt 0) {
    Write-Host "FAILED ($($failures.Count)):"; $failures | ForEach-Object { Write-Host "  - $_" }
    exit 1
}
Write-Host 'PROJECT MATCHES CARGO'
exit 0
