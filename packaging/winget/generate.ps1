#Requires -Version 5.1
<#
.SYNOPSIS
Generate winget manifests and a Scoop manifest from a release executable, without publishing.
.EXAMPLE
pwsh -File packaging/winget/generate.ps1 -Tag v0.6.0 -ExePath ./wisp-v0.9.0-x86_64.exe -OutputDirectory ./manifests
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^v\d+\.\d+\.\d+$')][string] $Tag,
    [Parameter(Mandatory)][string] $ExePath,
    [Parameter(Mandatory)][string] $OutputDirectory
)
$ErrorActionPreference = 'Stop'
$version = $Tag.Substring(1)
$repo = 'https://github.com/SumirSeth/wisp'
$url = "$repo/releases/download/$Tag/wisp-$Tag-x86_64.exe"
$sha256 = (Get-FileHash -LiteralPath $ExePath -Algorithm SHA256).Hash

$winget = Join-Path $OutputDirectory "winget/$version"
$scoop = Join-Path $OutputDirectory 'scoop'
New-Item -ItemType Directory -Force $winget, $scoop | Out-Null

@"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.version.1.10.0.schema.json
PackageIdentifier: SumirSeth.Wisp
PackageVersion: $version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.10.0
"@ | Set-Content -Encoding utf8 (Join-Path $winget 'SumirSeth.Wisp.yaml')

@"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.defaultLocale.1.10.0.schema.json
PackageIdentifier: SumirSeth.Wisp
PackageVersion: $version
PackageLocale: en-US
Publisher: Sumir Seth
PublisherUrl: https://github.com/SumirSeth
PackageName: Wisp
PackageUrl: $repo
License: MIT
LicenseUrl: $repo/blob/$Tag/LICENSE
ShortDescription: A dock and a launcher for Windows.
Moniker: wisp
Tags:
- dock
- launcher
- taskbar
ReleaseNotesUrl: $repo/releases/tag/$Tag
ManifestType: defaultLocale
ManifestVersion: 1.10.0
"@ | Set-Content -Encoding utf8 (Join-Path $winget 'SumirSeth.Wisp.locale.en-US.yaml')

@"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.installer.1.10.0.schema.json
PackageIdentifier: SumirSeth.Wisp
PackageVersion: $version
MinimumOSVersion: 10.0.19041.0
InstallerType: portable
Commands:
- wisp
Installers:
- Architecture: x64
  InstallerUrl: $url
  InstallerSha256: $sha256
ManifestType: installer
ManifestVersion: 1.10.0
"@ | Set-Content -Encoding utf8 (Join-Path $winget 'SumirSeth.Wisp.installer.yaml')

# The checked-in manifest provides Scoop metadata and future update rules.
$manifest = Get-Content (Join-Path $PSScriptRoot '../scoop/wisp.json') -Raw | ConvertFrom-Json
$manifest.version = $version
$manifest.architecture.'64bit'.url = "$url#/wisp.exe"
$manifest.architecture.'64bit'.hash = $sha256.ToLowerInvariant()
$manifest.PSObject.Properties.Remove('extract_dir')
$manifest.autoupdate.PSObject.Properties.Remove('extract_dir')
$manifest.shortcuts = @(@('wisp.exe', 'Wisp'), @('wisp.exe', 'Wisp Settings', '--settings'))
$manifest | ConvertTo-Json -Depth 10 | Set-Content -Encoding utf8 (Join-Path $scoop 'wisp.json')
Write-Output "Generated winget $version ($sha256) in $winget and Scoop in $scoop."
