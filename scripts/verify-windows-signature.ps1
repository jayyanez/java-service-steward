# SPDX-License-Identifier: Apache-2.0 OR MIT
#Requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$FilePath,
    [switch]$AllowUnsigned,
    [string]$PublisherName = 'Jay Yanez',
    [string]$ProfileEku = '1.3.6.1.4.1.311.97.79280211.230247311.368118662.886626554',
    [string]$OutputPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows) { throw 'Windows Authenticode verification requires Windows.' }
if (-not (Test-Path -LiteralPath $FilePath -PathType Leaf)) { throw "Missing artifact: $FilePath" }
$artifact = (Get-Item -LiteralPath $FilePath).FullName
$signature = Get-AuthenticodeSignature -LiteralPath $artifact
$publisher = $null
$timestamp = $null

if ($signature.Status -eq 'NotSigned' -and $AllowUnsigned) {
    Write-Host 'Signing was deliberately disabled; accepting an unsigned artifact.'
} else {
    if ($signature.Status -ne 'Valid' -or $null -eq $signature.SignerCertificate) {
        throw "A valid Authenticode signature is required: $($signature.Status); $artifact"
    }
    if ($null -eq $signature.TimeStamperCertificate) { throw 'The signature must have a timestamp.' }
    $publisher = $signature.SignerCertificate.GetNameInfo(
        [System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
    if ($publisher -cne $PublisherName) { throw "Unexpected publisher: $publisher" }
    $eku = @($signature.SignerCertificate.Extensions | Where-Object { $_.Oid.Value -eq '2.5.29.37' })
    if ($eku.Count -ne 1 -or $ProfileEku -notin @($eku[0].EnhancedKeyUsages | ForEach-Object { $_.Value })) {
        throw 'The signature is not from the expected Azure certificate profile.'
    }
    $timestamp = $signature.TimeStamperCertificate.Subject
}

$receipt = [pscustomobject][ordered]@{
    FileName = [IO.Path]::GetFileName($artifact)
    Status = $signature.Status.ToString()
    Publisher = $publisher
    ProfileEku = if ($publisher) { $ProfileEku } else { $null }
    TimestampAuthority = $timestamp
    Sha256 = (Get-FileHash -LiteralPath $artifact -Algorithm SHA256).Hash
    VerifiedAtUtc = [DateTime]::UtcNow.ToString('o')
}
if ($OutputPath) {
    $receipt | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $OutputPath -Encoding utf8
}
$receipt
