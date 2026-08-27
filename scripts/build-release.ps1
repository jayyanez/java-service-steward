# SPDX-License-Identifier: Apache-2.0 OR MIT
[CmdletBinding()]
param(
    [string] $JavaHome
)

$ErrorActionPreference = 'Stop'
$verifyVersion = Join-Path $PSScriptRoot 'verify-version.ps1'
$buildJavaBridge = Join-Path $PSScriptRoot 'build-java-bridge.ps1'

& $verifyVersion

& cargo fmt --check
if ($LASTEXITCODE -ne 0) { throw 'cargo fmt --check failed' }

& cargo clippy --all-targets --all-features -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'cargo clippy failed' }

& cargo test --all-targets --all-features
if ($LASTEXITCODE -ne 0) { throw 'cargo test failed' }

& cargo build --release
if ($LASTEXITCODE -ne 0) { throw 'cargo build --release failed' }

if ($JavaHome) {
    & $buildJavaBridge -JavaHome $JavaHome
}
else {
    & $buildJavaBridge
}
if ($LASTEXITCODE -ne 0) { throw 'Java bridge build failed' }

& $verifyVersion -RequireArtifacts
