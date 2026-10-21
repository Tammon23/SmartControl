param(
    [Parameter(Mandatory=$true)][string]$BinaryPath,
    [Parameter(Mandatory=$true)][string]$Version,
    [string]$OutputDirectory = 'artifacts'
)
$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[a-zA-Z0-9.-]+)?$') { throw 'Invalid release version' }
$repository = Split-Path $PSScriptRoot -Parent
$output = [IO.Path]::GetFullPath($OutputDirectory)
$package = Join-Path $output "SmartControl-$Version-windows-x64"
New-Item -ItemType Directory -Force -Path $package | Out-Null
Copy-Item -LiteralPath $BinaryPath -Destination (Join-Path $package 'SmartControl.exe') -Force
foreach ($name in @('LICENSE', 'THIRD_PARTY_NOTICES.md')) {
    Copy-Item -LiteralPath (Join-Path $repository $name) -Destination $package -Force
}
Copy-Item -LiteralPath (Join-Path $repository 'licenses') -Destination $package -Recurse -Force
Copy-Item -LiteralPath (Join-Path $repository 'windows/START-HERE.md') -Destination $package -Force
Copy-Item -LiteralPath (Join-Path $repository 'windows/client.server.example.json') -Destination $package -Force
# Explicit allowlist above: no local client.json or authentication files are packaged.
$archive = Join-Path $output "SmartControl-$Version-windows-x64.zip"
Compress-Archive -Path (Join-Path $package '*') -DestinationPath $archive -Force
$digest = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
"$digest  $([IO.Path]::GetFileName($archive))" | Set-Content -Encoding utf8 -LiteralPath "$archive.sha256"
Write-Output $archive
