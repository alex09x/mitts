# Mitts Build — install a verified GitHub Release binary on Windows.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
#
#   irm https://mittsbuild.com/install.ps1 | iex
#
# Works in PowerShell 7 and in Windows PowerShell 5.1. MITTS_VERSION picks a release
# (default: the latest), MITTS_INSTALL_ROOT the prefix (default: CARGO_INSTALL_ROOT,
# CARGO_HOME, else %USERPROFILE%\.cargo); mitts.exe goes to <prefix>\bin. MITTS_ARCHIVE
# installs a local release archive, verified against the `.sha256` file beside it (used to
# test a release before it is published).
#
# The whole script runs in its own scope: piped into `iex` it leaves no variables, strict
# mode or error preference behind in the caller's session.
& {
    $ErrorActionPreference = 'Stop'
    # Windows PowerShell 5.1 slows downloads tenfold drawing the progress bar.
    $ProgressPreference = 'SilentlyContinue'
    Set-StrictMode -Version Latest
    # Windows PowerShell 5.1 on older systems offers TLS 1.0 only; GitHub needs 1.2.
    try {
        [Net.ServicePointManager]::SecurityProtocol =
            [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch {}

    function Fail([string]$message) {
        throw "mitts installer: $message"
    }

    $target = 'x86_64-pc-windows-msvc'
    $version = $env:MITTS_VERSION
    if ($version) {
        if (-not $version.StartsWith('v')) { $version = "v$version" }
        if ($version -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+$') { Fail 'Release must have a stable vX.Y.Z tag.' }
    }

    $root = $env:MITTS_INSTALL_ROOT
    if (-not $root) { $root = $env:CARGO_INSTALL_ROOT }
    if (-not $root) { $root = $env:CARGO_HOME }
    if (-not $root) { $root = Join-Path $env:USERPROFILE '.cargo' }
    $destination = Join-Path $root 'bin'

    # The real architecture, also from a 32-bit or emulated PowerShell.
    $arch = $env:PROCESSOR_ARCHITEW6432
    if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
    if (-not $arch) {
        # No variable to go by (a stripped environment): ask .NET.
        try {
            $arch = switch ("$([Runtime.InteropServices.RuntimeInformation]::OSArchitecture)") {
                'X64' { 'AMD64' } 'Arm64' { 'ARM64' } 'X86' { 'x86' } default { "$_" }
            }
        } catch {
            if ([Environment]::Is64BitOperatingSystem) { $arch = 'AMD64' }
        }
    }
    if ($arch -ne 'AMD64') {
        # As install.sh: build from crates.io where there is no prebuilt binary.
        $reason = "No prebuilt binary for Windows on $arch (prebuilt: x86-64)."
        if ($env:MITTS_NO_SOURCE_FALLBACK) { Fail "$reason Source fallback is disabled (MITTS_NO_SOURCE_FALLBACK)." }
        if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
            Fail "$reason Install Rust (https://rustup.rs) and run: cargo install mitts --locked"
        }
        Write-Host "mitts installer: $reason Building from crates.io with Cargo (this takes a few minutes)..."
        $cargoArgs = @('install', 'mitts', '--locked', '--force', '--root', $root)
        if ($version) { $cargoArgs += @('--version', "=$($version.Substring(1))") }
        & cargo @cargoArgs
        if ($LASTEXITCODE -ne 0) { Fail 'cargo install failed.' }
        $built = (& (Join-Path $destination 'mitts.exe') --version) | Select-Object -First 1
        Write-Host "$built built from source and installed at $(Join-Path $destination 'mitts.exe')"
        return
    }

    $repository = 'https://github.com/alex09x/mitts'
    $temporary = Join-Path ([IO.Path]::GetTempPath()) ('mitts-install-' + [Guid]::NewGuid())
    New-Item -ItemType Directory -Path $temporary | Out-Null
    $pending = $null
    try {
        if ($env:MITTS_ARCHIVE) {
            $archive = (Resolve-Path $env:MITTS_ARCHIVE).Path
            $asset = Split-Path $archive -Leaf
            $line = (Get-Content "$archive.sha256" -Raw).Trim()
            $fields = $line -split '\s+'
            if ($fields.Count -ne 2 -or $fields[1] -ne $asset) { Fail 'Malformed archive checksum file.' }
            $expected = $fields[0]
            if (-not $version) {
                if ($asset -notmatch '^mitts-(v[0-9]+\.[0-9]+\.[0-9]+)-') { Fail 'Cannot tell the archive version.' }
                $version = $Matches[1]
            }
        } else {
            if (-not $version) {
                $latest = Invoke-WebRequest -Uri "$repository/releases/latest" -Method Head -MaximumRedirection 10 -UseBasicParsing
                # The URL the redirect ended at: `ResponseUri` (Windows PowerShell 5.1) or the
                # request message's URI (PowerShell 7).
                $response = $latest.BaseResponse
                if ($response.PSObject.Properties['ResponseUri']) {
                    $resolved = "$($response.ResponseUri)"
                } else {
                    $resolved = "$($response.RequestMessage.RequestUri)"
                }
                if ($resolved -notmatch '/releases/tag/(v[0-9]+\.[0-9]+\.[0-9]+)$') {
                    Fail 'Latest release did not resolve to a stable GitHub release tag.'
                }
                $version = $Matches[1]
            }
            $asset = "mitts-$version-$target.tar.gz"
            $base = "$repository/releases/download/$version"
            Write-Host "Installing Mitts $version for $target..."
            $archive = Join-Path $temporary $asset
            Invoke-WebRequest -Uri "$base/$asset" -OutFile $archive -UseBasicParsing
            $sums = Join-Path $temporary 'SHA256SUMS'
            Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile $sums -UseBasicParsing
            $matching = @(Get-Content $sums | Where-Object {
                    $f = $_ -split '\s+'; $f.Count -eq 2 -and $f[1] -eq $asset
                })
            if ($matching.Count -ne 1) { Fail 'Missing or ambiguous archive checksum.' }
            $expected = ($matching[0] -split '\s+')[0]
        }
        if ($expected -notmatch '^[a-f0-9]{64}$') { Fail 'Malformed archive checksum.' }
        $actual = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLowerInvariant()
        if ($actual -ne $expected) { Fail 'Archive checksum mismatch; existing installation was preserved.' }

        $unpacked = Join-Path $temporary 'unpacked'
        New-Item -ItemType Directory -Path $unpacked | Out-Null
        # Windows 10 and later ship bsdtar as tar.exe.
        & "$env:SystemRoot\System32\tar.exe" -xzf $archive -C $unpacked mitts.exe
        if ($LASTEXITCODE -ne 0) { Fail 'Archive does not contain mitts.exe.' }
        $binary = Join-Path $unpacked 'mitts.exe'
        $item = Get-Item $binary
        if ($item.LinkType -or $item.PSIsContainer) { Fail 'Release executable is not a regular file.' }

        New-Item -ItemType Directory -Force -Path $destination | Out-Null
        $final = Join-Path $destination 'mitts.exe'
        if (Test-Path $final -PathType Container) { Fail 'Install destination is a directory.' }
        # Old copies a previous install could not delete while they were running.
        Get-ChildItem -Path $destination -Filter '.mitts-old-*.exe' -ErrorAction SilentlyContinue |
            ForEach-Object { Remove-Item -Force $_.FullName -ErrorAction SilentlyContinue }
        $pending = Join-Path $destination ('.mitts-install-' + [Guid]::NewGuid() + '.exe')
        Copy-Item $binary $pending
        $installed = (& $pending --version) | Select-Object -First 1
        if ($LASTEXITCODE -ne 0 -or -not $installed) { Fail 'Cannot execute the release binary.' }
        $plain = $version.Substring(1)
        if ($installed -ne "mitts $plain" -and -not $installed.StartsWith("mitts $plain (")) {
            Fail "Unexpected binary version: $installed"
        }
        # A running mitts.exe cannot be replaced, but it can be renamed: move it aside first.
        if (Test-Path $final) {
            $old = Join-Path $destination ('.mitts-old-' + [Guid]::NewGuid() + '.exe')
            Move-Item -Force $final $old
            Move-Item -Force $pending $final
            Remove-Item -Force $old -ErrorAction SilentlyContinue
        } else {
            Move-Item -Force $pending $final
        }
        $pending = $null
        Write-Host "$installed installed at $final"
        $onPath = @(($env:PATH -split ';') | Where-Object { $_ -and ($_.TrimEnd('\') -ieq $destination.TrimEnd('\')) })
        if ($onPath.Count -eq 0) {
            Write-Host "Add $destination to PATH. Rust and the MSVC build tools are needed to build Rust projects."
        }
    } finally {
        if ($pending -and (Test-Path $pending)) { Remove-Item -Force $pending }
        Remove-Item -Recurse -Force $temporary -ErrorAction SilentlyContinue
    }
}
