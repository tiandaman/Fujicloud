# Fetches Microsoft's redistributable DirectML.dll (latest stable, from NuGet)
# into src-tauri/, where release builds bundle it next to Fujicloud.exe.
# ONNX Runtime needs a newer DirectML than the copy built into Windows, and
# Windows loads the one in the app's folder first.
#
#   pwsh scripts/fetch-directml.ps1 [-Into <dir> ...]

param([string[]]$Into = @())

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$feed = 'https://api.nuget.org/v3-flatcontainer/microsoft.ai.directml'
$version = ((Invoke-RestMethod "$feed/index.json").versions | Where-Object { $_ -notmatch '-' })[-1]

$work = Join-Path ([IO.Path]::GetTempPath()) "directml-$version"
$pkg = "$work.zip"
Invoke-WebRequest "$feed/$version/microsoft.ai.directml.$version.nupkg" -OutFile $pkg
Expand-Archive $pkg -DestinationPath $work -Force

$dll = Join-Path $work 'bin/x64-win/DirectML.dll'
foreach ($dir in @((Join-Path $root 'src-tauri')) + $Into) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Copy-Item $dll (Join-Path $dir 'DirectML.dll') -Force
}
Write-Host "DirectML $version -> src-tauri/DirectML.dll $($Into -join ' ')"
