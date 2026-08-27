# Naming and versioning policy

## Identity shown in the artifacts

The product name is **Java Service Steward**. Its official one-line description
is **Runs, controls, and monitors Java applications as Windows services.** The
deployable executable keeps the name `wrapper.exe` and the bridge keeps the name
`wrapper.jar` so that existing installations can be replaced in place. The
version banner always has three lines:

```text
Java Service Steward 64-bit 0.3.0
Runs, controls, and monitors Java applications as Windows services.
Reads wrapper.conf-style configuration files.
```

`0.3.0` is the version of this project and the first public release. No other
product's version number appears in the banner, in `wrapper.log` or in the
artifacts.

Project identifiers derive from the product name: the Rust package is
`java-service-steward`, project extensions use the `jss.*` property prefix, and
the Java bridge lives in `io.github.jayyanez.jss.bridge`. The compatible
identifiers `wrapper.exe`, `wrapper.conf`, `wrapper.*` properties and the
launcher-name mapping described in `docs/compatibility.md` do not change.

The single source of the version is `version` in `Cargo.toml`. During a build:

- Rust exposes it through `CARGO_PKG_VERSION`;
- `wrapper.exe --help`, `wrapper.exe --version` and `wrapper.log` read that
  same constant;
- `scripts/build-java-bridge.ps1` copies it into the `Implementation-Version`
  manifest attribute of `wrapper.jar` and the bridge exposes it through
  `Steward.getVersion()`;
- `scripts/verify-version.ps1` checks `Cargo.toml`, `Cargo.lock`,
  `CHANGELOG.md`, the EXE and the JAR.

The version is never written by hand in Rust code, Java code, help text or
manifests.

## SemVer scheme

The project follows [Semantic Versioning 2.0.0](https://semver.org/) with a
more conservative convention during the `0.x` series.

### PATCH: `0.3.0` -> `0.3.1`

PATCH is incremented when a release contains only compatible changes:

- a bug fix;
- a security fix that does not change the public contract;
- a performance or resource improvement without documented behavior change;
- a help or documentation correction or extension;
- new tests or internal refactoring without a new public feature.

If any distributed byte changes after `0.3.0` has been published, the result
cannot be published as `0.3.0` again; at minimum it becomes `0.3.1`.

### MINOR: `0.3.x` -> `0.4.0`

MINOR is incremented when a new capability or contract appears:

- a new `wrapper.exe` command;
- a new `wrapper.*` or `jss.*` property;
- a new integration method or launcher;
- an extension of the bridge API (`Steward`, `ServiceListener`);
- a new Java version declared as supported;
- telemetry, remote control or an agent-to-agent protocol;
- a deliberately incompatible change during the `0.x` stage, including a
  change to the internal EXE-JAR contract.

Although SemVer allows instability in `0.x`, every incompatibility must be
marked as **Breaking** in `CHANGELOG.md`, include a migration note, and
preserve the `wrapper.conf` contract whenever possible.

### MAJOR: `1.x` and later

`1.0.0` will be published when the supported scope is stable and backward
compatibility can be maintained. At minimum it requires:

- a stable `SimpleApp`, `StartStopApp`, `JarApp` and `ServiceListener`
  contract;
- SCM lifecycle, `LPTM` logging and rotation, filters, diagnostics and
  recovery covered by automated tests;
- a repeatable modern Java test matrix;
- a documented release, upgrade and rollback process;
- a defined public configuration and telemetry surface.

After `1.0.0`, a public incompatibility increments MAJOR, a compatible feature
increments MINOR and a compatible fix increments PATCH.

## Pre-release versions

To deliver candidates that must not yet be promoted to production:

```text
0.4.0-alpha.1
0.4.0-beta.1
0.4.0-rc.1
0.4.0
```

Each distinct candidate increments its suffix. The final version drops the
suffix. `+...` build metadata may identify a build, but it does not replace a
version increment when the content of a release changes.

## When not to change the version

The version is not incremented for rebuilding exactly the same code and
dependencies to reproduce the same release, nor for every intermediate local
build. The version is decided when preparing an artifact that will be declared
a release, delivered for normal use, or kept for rollback.

An experimental copy installed for a test does not automatically become a
release. Once a version has been declared or kept as a release, no different
artifact may reuse its number.

## Version bump rules

The release procedure itself (tagging, CI, packaging, checksums) is described
in [release.md](release.md). The rules for the version number are:

1. Classify the changes since the last release as PATCH, MINOR or MAJOR using
   the rules above.
2. Change **only** `version` in `Cargo.toml`; `Cargo.lock` is updated by Cargo.
3. Move the applicable notes from the unreleased section of `CHANGELOG.md` to
   a dated section with the same version.
4. Run `scripts/verify-version.ps1`; it fails when `Cargo.lock`, `CHANGELOG.md`,
   the built EXE or the built JAR disagree with `Cargo.toml`.
5. Never replace already delivered artifacts while keeping the same number.

The JAR packager sorts its entries and uses a fixed ZIP timestamp. Two runs on
the same sources and toolchain must produce the same hash; a difference is a
release-process problem and is investigated before publishing.

A product rename is a decision separate from the technical contract. After a
release, a branding-only change is at least a PATCH because it modifies the
artifacts; a namespace, property or API change is a MINOR in `0.x` and must
include aliases or migration instructions.
