# SPDX-License-Identifier: Apache-2.0 OR MIT
[CmdletBinding()]
param(
    [string] $JavaHome
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$sourceRoot = Join-Path $repositoryRoot 'java\bridge\src\main\java'
$manifestTemplate = Join-Path $repositoryRoot 'java\bridge\MANIFEST.MF'
$classes = Join-Path $repositoryRoot 'target\java-bridge\classes'
$manifest = Join-Path $repositoryRoot 'target\java-bridge\MANIFEST.MF'
$output = Join-Path $repositoryRoot 'target\release\wrapper.jar'
$cargoToml = Join-Path $repositoryRoot 'Cargo.toml'

if ($JavaHome) {
    $javac = Join-Path $JavaHome 'bin\javac.exe'
} else {
    $javac = (Get-Command javac.exe -ErrorAction Stop).Source
}

if (Test-Path -LiteralPath $classes) {
    Remove-Item -LiteralPath $classes -Recurse -Force
}
New-Item -ItemType Directory -Path $classes -Force | Out-Null
New-Item -ItemType Directory -Path (Split-Path -Parent $output) -Force | Out-Null

$cargoText = Get-Content -LiteralPath $cargoToml -Raw
$versionMatch = [regex]::Match(
    $cargoText,
    '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"'
)
if (-not $versionMatch.Success) {
    throw "Could not read the package version from $cargoToml"
}
$projectVersion = $versionMatch.Groups[1].Value
$manifestText = Get-Content -LiteralPath $manifestTemplate -Raw
if (-not $manifestText.Contains('@PROJECT_VERSION@')) {
    throw "The Java bridge manifest does not contain @PROJECT_VERSION@"
}
$manifestText = $manifestText.Replace('@PROJECT_VERSION@', $projectVersion)
[System.IO.File]::WriteAllText(
    $manifest,
    $manifestText,
    [System.Text.UTF8Encoding]::new($false)
)

$sources = Get-ChildItem -LiteralPath $sourceRoot -Filter '*.java' -Recurse |
    Select-Object -ExpandProperty FullName
$javacVersion = (& $javac -version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "javac -version failed with exit code $LASTEXITCODE"
}
$compatibilityArguments = if ($javacVersion -match '^javac 1\.8') {
    @('-source', '8', '-target', '8', '-Xlint:-options')
} else {
    @('--release', '8', '-Xlint:-options')
}
& $javac @compatibilityArguments -encoding UTF-8 -d $classes @sources
if ($LASTEXITCODE -ne 0) {
    throw "javac failed with exit code $LASTEXITCODE"
}

if (Test-Path -LiteralPath $output) {
    Remove-Item -LiteralPath $output -Force
}
Add-Type -AssemblyName System.IO.Compression
$fixedTimestamp = [DateTimeOffset]::Parse(
    '1980-01-01T00:00:00Z',
    [Globalization.CultureInfo]::InvariantCulture
)
$fileStream = [System.IO.File]::Open(
    $output,
    [System.IO.FileMode]::CreateNew,
    [System.IO.FileAccess]::ReadWrite,
    [System.IO.FileShare]::None
)
$archive = $null
try {
    $archive = [System.IO.Compression.ZipArchive]::new(
        $fileStream,
        [System.IO.Compression.ZipArchiveMode]::Create,
        $true
    )

    $manifestEntry = $archive.CreateEntry(
        'META-INF/MANIFEST.MF',
        [System.IO.Compression.CompressionLevel]::Optimal
    )
    $manifestEntry.LastWriteTime = $fixedTimestamp
    $entryStream = $manifestEntry.Open()
    try {
        $manifestBytes = [System.IO.File]::ReadAllBytes($manifest)
        $entryStream.Write($manifestBytes, 0, $manifestBytes.Length)
    }
    finally {
        $entryStream.Dispose()
    }

    $classPrefix = $classes.TrimEnd([char[]]@('\', '/')) + [IO.Path]::DirectorySeparatorChar
    $classFiles = Get-ChildItem -LiteralPath $classes -File -Recurse |
        Sort-Object FullName
    foreach ($classFile in $classFiles) {
        $entryName = $classFile.FullName.Substring($classPrefix.Length).Replace('\', '/')
        $entry = $archive.CreateEntry(
            $entryName,
            [System.IO.Compression.CompressionLevel]::Optimal
        )
        $entry.LastWriteTime = $fixedTimestamp
        $entryStream = $entry.Open()
        $inputStream = $classFile.OpenRead()
        try {
            $inputStream.CopyTo($entryStream)
        }
        finally {
            $inputStream.Dispose()
            $entryStream.Dispose()
        }
    }
}
finally {
    if ($null -ne $archive) {
        $archive.Dispose()
    }
    $fileStream.Dispose()
}

$artifact = Get-Item -LiteralPath $output
$hash = Get-FileHash -LiteralPath $output -Algorithm SHA256
Write-Host "Created $($artifact.FullName) version $projectVersion ($($artifact.Length) bytes)"
Write-Host "SHA256 $($hash.Hash)"
