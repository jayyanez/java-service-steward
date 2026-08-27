# SPDX-License-Identifier: Apache-2.0 OR MIT
[CmdletBinding()]
param(
    [switch] $RequireArtifacts
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$cargoTomlPath = Join-Path $repositoryRoot 'Cargo.toml'
$cargoLockPath = Join-Path $repositoryRoot 'Cargo.lock'
$changelogPath = Join-Path $repositoryRoot 'CHANGELOG.md'
$manifestTemplatePath = Join-Path $repositoryRoot 'java\bridge\MANIFEST.MF'
$wrapperExePath = Join-Path $repositoryRoot 'target\release\wrapper.exe'
$wrapperJarPath = Join-Path $repositoryRoot 'target\release\wrapper.jar'

$cargoToml = Get-Content -LiteralPath $cargoTomlPath -Raw
$versionMatch = [regex]::Match(
    $cargoToml,
    '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"'
)
if (-not $versionMatch.Success) {
    throw "Could not read the package version from $cargoTomlPath"
}
$projectVersion = $versionMatch.Groups[1].Value
$semverPattern = '^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$'
if ($projectVersion -notmatch $semverPattern) {
    throw "Cargo package version is not valid SemVer: $projectVersion"
}

$cargoLock = Get-Content -LiteralPath $cargoLockPath -Raw
$lockPattern = '(?ms)^\[\[package\]\]\s*name\s*=\s*"java-service-steward"\s*version\s*=\s*"' +
    [regex]::Escape($projectVersion) + '"'
if ($cargoLock -notmatch $lockPattern) {
    throw "Cargo.lock does not contain java-service-steward version $projectVersion"
}

$changelog = Get-Content -LiteralPath $changelogPath -Raw
if (-not $changelog.Contains("## [$projectVersion]")) {
    throw "CHANGELOG.md does not contain a section for $projectVersion"
}

$manifestTemplate = Get-Content -LiteralPath $manifestTemplatePath -Raw
if (-not $manifestTemplate.Contains('Implementation-Version: @PROJECT_VERSION@')) {
    throw 'The Java manifest must derive Implementation-Version from @PROJECT_VERSION@'
}

if ($RequireArtifacts) {
    foreach ($artifact in $wrapperExePath, $wrapperJarPath) {
        if (-not (Test-Path -LiteralPath $artifact -PathType Leaf)) {
            throw "Required release artifact is missing: $artifact"
        }
    }

    $versionOutput = @(& $wrapperExePath --version)
    if ($LASTEXITCODE -ne 0) {
        throw "wrapper.exe --version failed with exit code $LASTEXITCODE"
    }
    $expectedProduct = "Java Service Steward 64-bit $projectVersion"
    if ($versionOutput.Count -lt 3 -or $versionOutput[0] -cne $expectedProduct) {
        throw "wrapper.exe version mismatch; expected first line '$expectedProduct'"
    }
    if (($versionOutput -join "`n") -match '(?i)tanuki') {
        throw 'wrapper.exe --version must not name third-party products'
    }

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::OpenRead($wrapperJarPath)
    try {
        $entry = $archive.GetEntry('META-INF/MANIFEST.MF')
        if ($null -eq $entry) {
            throw 'wrapper.jar does not contain META-INF/MANIFEST.MF'
        }
        $reader = [System.IO.StreamReader]::new($entry.Open())
        try {
            $jarManifest = $reader.ReadToEnd()
        }
        finally {
            $reader.Dispose()
        }
    }
    finally {
        $archive.Dispose()
    }
    if (-not $jarManifest.Contains("Implementation-Version: $projectVersion")) {
        throw "wrapper.jar Implementation-Version is not $projectVersion"
    }

    foreach ($artifact in $wrapperExePath, $wrapperJarPath) {
        $item = Get-Item -LiteralPath $artifact
        $hash = Get-FileHash -LiteralPath $artifact -Algorithm SHA256
        Write-Host "$($item.Name) $projectVersion $($item.Length) bytes SHA256 $($hash.Hash)"
    }
}

Write-Host "Version $projectVersion is consistent."
