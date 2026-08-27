# Changelog

All notable changes to this project are recorded here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning 2.0.0](https://semver.org/). See
[docs/versioning.md](docs/versioning.md) for the project's version policy.

## [0.3.2] - 2026-08-27

### Added

- Every release publishes a CycloneDX SBOM of the Rust dependencies, a
  Sigstore build-provenance attestation and an SBOM attestation
  (`gh attestation verify`); `SHA256SUMS` also covers the SBOM.
- `wrapper.exe` carries a Windows version resource (product name,
  description, file and product version).

## [0.3.1] - 2026-08-26

### Fixed

- On Java 8 the bridge could turn a normal application exit (the application
  `main` returned) into JVM exit code 1: the control thread treated the socket
  closed by its own shutdown hook as a lost channel and called `System.exit(1)`
  while the JVM was already shutting down. The control thread now stops
  silently once shutdown has begun.

## [0.3.0] - 2026-08-26

First public release.

### Breaking

- The original Java Service Wrapper JAR and DLL are no longer supported.
  Deploy the bundled `wrapper.jar` at the path named by
  `wrapper.java.classpath`; `wrapper.dll` is never loaded and can be deleted.
  See [docs/migration.md](docs/migration.md).
- The property `jss.java.bridge` was removed. A configured main class that is
  absent from the classpath and whose name ends in `SimpleApp`, `StartStopApp`
  or `JarApp` is launched through the matching bundled launcher, so
  configuration files written for other launchers keep working unchanged; the
  wrapper fails before launching Java when the bridge is missing from the
  classpath.
- The bridge package was renamed to `io.github.jayyanez.jss.bridge` with the
  classes `SimpleApp`, `StartStopApp`, `JarApp`, `Steward` and
  `ServiceListener`. Code that imported the previous bridge namespace must be
  updated.
- Supervisor log messages were reworded in the project's own voice. Only the
  markers `--> Wrapper Started as Service`, `--> Wrapper Started as Console`
  and `<-- Wrapper Stopped` are unchanged; the second line of every run is now
  the product banner `Java Service Steward 64-bit <version>`.
- The system properties `wrapper.version`, `wrapper.native_library` and
  `wrapper.cpu.timeout` are no longer passed to the JVM; `jss.version` is
  passed instead.

### Added

- `StartStopApp` launcher with separate start and stop classes and the
  `waitForStopThreads` option.
- `JarApp` launcher that runs the `Main-Class` of an executable JAR.
- `ServiceListener` interface and `Steward.start(listener, args)` for
  applications that need lifecycle callbacks from a custom main class.
- `jss.heapdump.timeout` (default 600 seconds) bounds `jcmd GC.heap_dump`.
- `jss.java.job_object` (default `true`) places the JVM and its children in a
  Windows Job Object that is closed together with the supervisor.
- `wrapper.license.*` is accepted and ignored with a single INFO line, so
  configuration files from other editions do not produce one warning per line.
- Configuration warnings are written to `wrapper.log`, not only to stderr.
- Service start-up failures that happen before the log file can be opened are
  recorded in the Windows Event Log.
- Dual license `Apache-2.0 OR MIT`, `NOTICE`, third-party notices, provenance
  statement, contribution guidelines, security policy.
- GitHub Actions CI (Windows, Temurin 8/21/25, `JSS_REQUIRE_JAVA_TESTS=1`) and
  a tag-driven release workflow that publishes the zip and `SHA256SUMS`.

### Fixed

- The JVM now exits when the application `main` returns and no non-daemon
  threads remain; the supervisor then applies `wrapper.on_exit.*`. Previously
  the control loop kept the JVM alive indefinitely.
- The control-channel handshake no longer rejects a key that arrives after a
  short pause; pending connections are given a deadline instead of 50 ms.
- Packet framing keeps partial packets across read timeouts on both ends, so a
  timeout in the middle of a packet no longer desynchronizes the stream.
- The supervisor no longer busy-loops on one core while the control channel is
  disconnected.
- JVM output draining is decoupled from socket polling; sustained high-volume
  output no longer blocks the application in `println` or triggers ping
  timeouts.
- Output lines are capped at 64 KiB and split, so a line without a newline can
  no longer grow without bound.
- The heap-dump worker has a timeout and no longer disables the ping watchdog
  while it runs.
- Sensitive properties (names containing `password`, `secret`, `token`, `key`,
  `credential` or `vault`) are no longer sent to the JVM in the properties
  packet.
- The exit code requested by the application is forwarded in the `STOP`
  packet, so the Java process exits with that code as well.

### Changed

- Everything in the repository (code, messages, documentation, changelog) is
  written in English.
- Documentation was restructured: `docs/compatibility.md` (feature set and
  property table), `docs/migration.md`, `docs/provenance.md`,
  `docs/release.md`; the research notes keep interface facts only.
- `--version` prints a three-line banner naming only this product.

## Internal, unpublished history

The following versions were internal development milestones and were never
published.

### [0.2.1] - 2026-08-26

#### Fixed

- A production runtime may be a JRE or a reduced image without `javac`, `jar`
  or `jcmd`; build tools are not service requirements.
- A missing `jcmd` and `-XX:+DisableAttachMechanism` produce actionable
  capability errors without stopping or restarting the Java application.
- A heap dump is not announced as started and does not create its directory
  when the required tool is unavailable.
- The delay before forced termination is not consumed when the thread-dump
  request was already rejected.
- The bridge no longer links `java.management` statically; it reads the modern
  PID through `ProcessHandle` and degrades the getter on minimal runtimes.
- JAR packaging uses a fixed entry order and timestamp so two builds of the
  same code produce the same SHA-256.

#### Tests

- Start, pings, degraded diagnostics and stop verified with a real Temurin 8
  JRE whose `bin` directory contains no `jcmd.exe`.

### [0.2.0] - 2026-08-26

#### Added

- Project-owned Java 8-compatible bridge JAR without a native library, with
  transparent substitution of the simple-application launcher alias and a
  bridge selection property.
- On-demand HPROF heap dumps and a `jcmd` fallback for thread dumps with
  `-Xrs`.
- Complete operational help embedded in `wrapper.exe`.
- Extended properties, filters, rotation, PID/ID files and SCM lifecycle.
- Single-version policy and validation for the EXE and the JAR.

#### Compatibility

- Simple-application launcher verified with Java 8, 21 and 25.
- Field start/stop/start verified with a Java 21 application server.
- Other integration methods were not yet part of the guarantee.

### [0.1.0] - 2026-08-25

#### Added

- First Rust prototype able to replace `wrapper.exe` while keeping an existing
  `wrapper.conf` and the original wrapper JAR/DLL.
- Console mode, Windows service, local protocol, `LPTM` logging and basic
  supervision.
