# scripts/bundle-windows.ps1: the Windows counterpart of bundle.sh.
# Builds antler.exe in release mode (build.rs embeds the icon and the version info) and makes
# target/bundle/Antler-<version>-x86_64-windows.zip: antler.exe and bin\antler.cmd (the `antler` command), like VS Code's zip.
# Needs the MSVC Build Tools and a Windows SDK (fxc.exe compiles GPUI's shaders): docs/agents/gpui-notes.md.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($args.Count) { Write-Error "usage: scripts/bundle-windows.ps1" }

Set-Location (Split-Path -Parent $PSScriptRoot)
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
# cargo builds for the host: the zip name says x86_64, so refuse anything else (aarch64 is a follow-up, #39)
$hostTriple = (rustc -vV | Select-String '^host: (.*)').Matches[0].Groups[1].Value
if ($hostTriple -ne 'x86_64-pc-windows-msvc') { Write-Error "builds on x86_64-pc-windows-msvc only, not $hostTriple" }

cargo build --release
if ($LASTEXITCODE) { exit $LASTEXITCODE }

$stage = 'target/bundle/windows'
$zip = "target/bundle/Antler-$version-x86_64-windows.zip"
Remove-Item -Recurse -Force $stage, $zip -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$stage/bin" | Out-Null
Copy-Item target/release/antler.exe $stage
# CRLF whatever the checkout gave (.gitattributes asks for it): cmd misreads labels in an LF batch file
[IO.File]::WriteAllText((Join-Path (Resolve-Path "$stage/bin") antler.cmd), ((Get-Content -Raw scripts/antler.cmd) -replace "`r?`n", "`r`n"))
Copy-Item LICENSE-MIT, LICENSE-APACHE $stage

# .NET's zip writes / separators (Compress-Archive in Windows PowerShell 5 wrote \)
Add-Type -AssemblyName System.IO.Compression.FileSystem
[IO.Compression.ZipFile]::CreateFromDirectory((Resolve-Path $stage), (Join-Path (Resolve-Path .) $zip))
Write-Host "Zipped $zip ($version)"
