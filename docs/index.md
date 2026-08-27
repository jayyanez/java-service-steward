---
title: Java Service Steward
---

# Java Service Steward

Runs, controls, and monitors Java applications as Windows services.

Java Service Steward is a Windows service host for Java applications, written
in Rust and released under `Apache-2.0 OR MIT`. It installs a Java application
as a Windows service, launches and supervises the JVM over an authenticated
loopback channel, restarts it when it fails or stops responding, rotates its
log, and provides thread and heap dumps on demand. It reads the widely
deployed `wrapper.conf` configuration format and follows its command-line
conventions, so existing installations can switch to it in place. It ships as
one executable (`wrapper.exe`) plus a small Java 8-compatible bridge JAR, with
no native library, no JNI and no installer.

- **Source and README:** <https://github.com/jayyanez/java-service-steward>
- **Downloads:** <https://github.com/jayyanez/java-service-steward/releases>
  (zip with `wrapper.exe` and `wrapper.jar`, `SHA256SUMS`, CycloneDX SBOM and
  a Sigstore build-provenance attestation)

## Documentation

- [Compatibility with the wrapper.conf format](compatibility.md): commands,
  every configuration property with its support status, integration methods,
  log format, service semantics, known limits.
- [Migrating an existing installation](migration.md): in-place replacement,
  rollback, properties that are ignored, launcher mapping, API migration.
- [Thread and heap dumps](diagnostics.md).
- [Architecture](architecture.md).
- [Testing](testing.md) and [performance baseline](performance.md).
- [Release process and supply-chain artifacts](release.md): how releases are
  built, how to verify a download.
- [Versioning policy](versioning.md).
- [Provenance](provenance.md): how compatibility was achieved and the rules
  contributors follow.
- [Interoperability research notes](research.md).

## Trademarks and affiliation

Java Service Steward is an independent project. It is not affiliated with,
endorsed by, or sponsored by Tanuki Software, Ltd. "Java Service Wrapper" and
"Tanuki Software" are names of Tanuki Software, Ltd. and are used here only to
describe compatibility with the `wrapper.conf` configuration format and
command-line conventions of that product. Java Service Steward does not
contain, bundle, or require any Tanuki Software code or binaries.
