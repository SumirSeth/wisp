#Requires -Version 7.0
<#
.SYNOPSIS
Generate winget manifests and a Scoop manifest from a release zip, without publishing.
.EXAMPLE
pwsh -File packaging/winget/generate.ps1 -Tag v0.6.0 -ZipPath ./wisp-v0.6.0-x86_64-windows.zip -OutputDirectory ./manifests
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^v\d+\.\d+\.\d+$')][string] $Tag,
    [Parameter(Mandatory)][string] $ZipPath,
    [Parameter(Mandatory)][string] $OutputDirectory
)
$ErrorActionPreference = 'Stop'
$version = $Tag.Substring(1)
$repo = 'https://github.com/SumirSeth/wisp'
$url = "$repo/releases/download/$Tag/wisp-$Tag-x86_64-windows.zip"
$sha256 = (Get-FileHash -LiteralPath $ZipPath -Algorithm SHA256).Hash

# Old releases have a versioned top folder. Inspect exact entries rather than
# guessing from the version, so this also works with local packaging dry runs.
$archive = [System.IO.Compression.ZipFile]::OpenRead((Resolve-Path -LiteralPath $ZipPath))
try {
    $entries = @($archive.Entries | ForEach-Object { $_.FullName.Replace('\', '/') })
    $folder = "wisp-$Tag-x86_64-windows"
    if ('wisp.exe' -cin $entries -and 'wisp-settings.exe' -cin $entries) {
        $prefix = ''
    } elseif ("$folder/wisp.exe" -cin $entries -and "$folder/wisp-settings.exe" -cin $entries) {
        $prefix = "$folder\"
    } else {
        throw 'The zip must contain both executables at its root or in its release folder.'
    }
} finally {
    $archive.Dispose()
}

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
InstallerType: zip
NestedInstallerType: portable
NestedInstallerFiles:
- RelativeFilePath: ${prefix}wisp.exe
  PortableCommandAlias: wisp
- RelativeFilePath: ${prefix}wisp-settings.exe
  PortableCommandAlias: wisp-settings
Installers:
- Architecture: x64
  InstallerUrl: $url
  InstallerSha256: $sha256
ManifestType: installer
ManifestVersion: 1.10.0
"@ | Set-Content -Encoding utf8 (Join-Path $winget 'SumirSeth.Wisp.installer.yaml')

# The checked-in manifest provides Scoop metadata and future update rules.
$manifest = Get-Content (Join-Path $PSScriptRoot '../scoop/wisp.json') -Raw | ConvertFrom-Json -AsHashtable
$manifest.version = $version
$manifest.architecture.'64bit'.url = $url
$manifest.architecture.'64bit'.hash = $sha256.ToLowerInvariant()
if ($prefix) {
    $manifest.extract_dir = $prefix.TrimEnd('\')
} else {
    $manifest.Remove('extract_dir')
}
$manifest | ConvertTo-Json -Depth 10 | Set-Content -Encoding utf8 (Join-Path $scoop 'wisp.json')
Write-Output "Generated winget $version ($sha256) in $winget and Scoop in $scoop."
