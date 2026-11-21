param([Parameter(Mandatory)][string]$ExePath,[Parameter(Mandatory)][ValidatePattern('^v\d+\.\d+\.\d+$')][string]$Tag,[Parameter(Mandatory)][string]$OutputDirectory)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
$name = "wisp-$Tag-x86_64-windows"
$folder = Join-Path $OutputDirectory $name
New-Item -ItemType Directory -Force $folder | Out-Null
Copy-Item -LiteralPath $ExePath -Destination (Join-Path $OutputDirectory "wisp-$Tag-x86_64.exe") -Force
Copy-Item -LiteralPath $ExePath -Destination (Join-Path $folder 'wisp.exe') -Force
Copy-Item -LiteralPath $ExePath -Destination (Join-Path $folder 'wisp-settings.exe') -Force
# Drop this compatibility zip after a couple of releases; 0.8 updaters require both names.
Compress-Archive -LiteralPath $folder -DestinationPath (Join-Path $OutputDirectory "$name.zip") -Force
@("wisp-$Tag-x86_64.exe", "$name.zip") | ForEach-Object {
    (Get-FileHash -LiteralPath (Join-Path $OutputDirectory $_) -Algorithm SHA256).Hash + "  $_"
} | Set-Content -Encoding ascii -LiteralPath (Join-Path $OutputDirectory 'SHA256SUMS.txt')
