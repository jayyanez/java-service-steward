# Java Service Steward

Runs, controls, and monitors Java applications as Windows services.

[![CI](https://github.com/jayyanez/java-service-steward/actions/workflows/ci.yml/badge.svg)](https://github.com/jayyanez/java-service-steward/actions/workflows/ci.yml)
[![License: Apache-2.0 OR MIT](https://img.shields.io/badge/license-Apache--2.0%20OR%20MIT-blue.svg)](#license)

## Why

Java Service Steward is a Windows service host for Java applications, written
in Rust. It registers a Java application as a Windows service, launches the
JVM, keeps a loopback control channel to it, restarts it when it exits
unexpectedly or stops answering, rotates its log, and answers Service Control
Manager requests such as stop, pause, resume, and custom control codes. It
ships as one executable plus a small Java 8 compatible JAR: no native DLL, no
JNI, no installer.

It reads the widely deployed `wrapper.conf` configuration format and follows
the same command-line conventions (`-c`, `-i`, `-t`, `-p`, `-r`, `-d`, and so
on), so an existing configuration keeps working with a different host
underneath it. Java Service Steward targets 64-bit Windows, ships as a single
executable plus one JAR, and is permissively licensed (`Apache-2.0 OR MIT`), so
anybody can deploy, embed, or redistribute it without publishing their own
code.

## Status

Pre-1.0. The current version is **0.3.0**, the first public release. The
configuration format, command line, log format, and supervision semantics are
meant for production use; the Rust and Java APIs may still change between
minor versions, and every incompatible change is recorded in
[CHANGELOG.md](CHANGELOG.md).

| Area | Supported | Not supported |
| --- | --- | --- |
| Launchers | `SimpleApp`, `StartStopApp`, `JarApp`, selected through `wrapper.java.mainclass` (the launcher aliases used by existing configuration files are recognized) | The bridge JAR and native library of the original product |
| Application API | `io.github.jayyanez.jss.bridge.Steward` and `ServiceListener`: start, stop, restart, log, control events, start wait hints | The original product's Java API; applications that import it need a source-level migration |
| Diagnostics | Thread dumps (`-d`, control code 255, `CTRL_BREAK` or `jcmd`), on-demand HPROF heap dumps (`--heapdump`, control code 254) | JMX, remote control |
| Logging | `LPTM` log records, console format, `SIZE`/`WRAPPER`/`JVM` roll modes, size limit, archive count, per-level filtering | Syslog |
| Supervision | Startup, ping, and shutdown timeouts; restart throttling; exit-code actions; output filters (`RESTART`, `SHUTDOWN`, `DUMP`, `GC`, `PAUSE`, `RESUME`); pause/resume; Job Object clean-up of child processes | Unix daemons, named-pipe backends |
| Editions | The Community Edition feature set, property by property | Features that exist only in the Standard or Professional editions of the original product |

The complete property-by-property table is in
[docs/compatibility.md](docs/compatibility.md).

## Quick start

1. Download `java-service-steward-<version>-windows-x64.zip` from the
   [releases page](https://github.com/jayyanez/java-service-steward/releases)
   and check it against the `SHA256SUMS` file published next to it.
2. Unpack it. The zip contains `wrapper.exe`, `wrapper.jar`, the license
   files, and `examples/wrapper.conf.example`.
3. Copy `examples/wrapper.conf.example` to `wrapper.conf` next to
   `wrapper.exe` and edit the Java command, the classpath, and the main class.
4. Run the application in the current console first:

   ```powershell
   wrapper.exe -c wrapper.conf
   ```

   Press `Ctrl+C` to stop it. The log is written to the path named by
   `wrapper.logfile`.

5. Install and control it as a Windows service from an elevated shell:

   ```powershell
   wrapper.exe -i wrapper.conf    # install
   wrapper.exe -t wrapper.conf    # start
   wrapper.exe -p wrapper.conf    # stop
   wrapper.exe -r wrapper.conf    # remove
   ```

   `wrapper.exe -it wrapper.conf` installs and starts in one step, `-q`
   prints the service state, and `-a`/`-e` pause and resume a pausable
   service.

`wrapper.exe --help` prints the complete command and property reference; no
configuration file is read for that command.

Every release also ships a CycloneDX SBOM and a Sigstore build-provenance
attestation; `gh attestation verify <zip> --owner jayyanez` proves that a
download was built by this repository's release workflow. See
[docs/release.md](docs/release.md#supply-chain-artifacts). `wrapper.exe` is not
yet Authenticode-signed, so SmartScreen may warn on first launch.

## Configuration

A minimal `wrapper.conf`:

```properties
#encoding=UTF-8
set.APP_HOME=C:/example/app

wrapper.java.command=%JAVA_HOME%/bin/java.exe
wrapper.java.mainclass=io.github.jayyanez.jss.bridge.SimpleApp
wrapper.java.classpath.1=wrapper.jar
wrapper.java.classpath.2=%APP_HOME%/lib/*
wrapper.java.additional.1=-Xmx512m
wrapper.app.parameter.1=com.example.Main

wrapper.logfile=../logs/wrapper.log
wrapper.ntservice.name=example-service
wrapper.ntservice.displayname=Example Service
```

Rules worth knowing:

- A relative configuration path, and every relative path inside the file
  (classpath entries, log file, PID files, working directory), is resolved
  from the directory that contains `wrapper.exe`, not from the caller's
  current directory. This keeps service start-up independent of how the
  Service Control Manager launches the process.
- `set.NAME=value` defines an environment variable for the rest of the file
  and for the JVM; `set.default.NAME=value` defines it only when it is not
  already set. `%NAME%` expands an environment variable, including one
  defined with `set.`, anywhere in a value.
- Numbered properties (`wrapper.java.classpath.<n>`,
  `wrapper.java.additional.<n>`, `wrapper.app.parameter.<n>`, and others) are
  read in order from 1 and stop at the first missing index. An explicitly
  empty entry keeps its index and produces no argument.
- Properties given on the command line as `name=value` override the file,
  which is convenient for one-off runs such as
  `wrapper.exe -c wrapper.conf wrapper.debug=true`.

The fully commented [examples/wrapper.conf.example](examples/wrapper.conf.example)
covers the timeouts, filters, logging, and service properties, and the
project's own `jss.*` extensions. The prefix tells you what is portable:
`wrapper.*` properties belong to the shared configuration format, while
`jss.*` properties (all optional, all with defaults) exist only in Java Service
Steward; see [docs/compatibility.md](docs/compatibility.md#project-extensions-jss).

## Migrating an existing installation

An installation that already runs from a `wrapper.conf` can switch host in
place. Neither `wrapper.conf` nor the service's registered `ImagePath` needs
to change: the executable keeps the name `wrapper.exe`, the bridge keeps the
name `wrapper.jar`, and the launcher class names accepted in
`wrapper.java.mainclass` are recognized and mapped to the bundled launchers.

1. Stop the service.
2. Back up the current `wrapper.exe`, `wrapper.jar`, and the native DLL next
   to them.
3. Copy the new `wrapper.exe` over the old one, and the new `wrapper.jar`
   over the JAR named by `wrapper.java.classpath`.
4. Optionally delete the old native DLL. It is never loaded.
5. Start the service.

Rollback is the reverse: stop the service and restore the backed-up files.

The JAR and DLL of the original product are not supported: when the configured
classpath does not contain the bundled `wrapper.jar`, the wrapper refuses to
launch Java and says so in the log. Applications that only run a `main`
method need no code change. Applications that implemented the original
product's listener interface must be ported to
`io.github.jayyanez.jss.bridge.Steward` and `ServiceListener`; this is a
source change, described in [docs/migration.md](docs/migration.md) together
with the table of properties that are supported, accepted and ignored, or
rejected.

## Diagnostics

- `wrapper.exe -d wrapper.conf` asks the running service for a thread dump.
  It is captured as ordinary `jvm <n>` records in the configured log file,
  with the normal format and rotation. `sc.exe control <service> 255` does
  the same without the wrapper.
- `wrapper.exe --heapdump wrapper.conf` asks for an HPROF heap dump written
  next to the log file, or to `jss.heapdump.directory`. This is a Java
  Service Steward extension on control code 254. It needs a `jcmd` matching
  the configured Java runtime; without one the request is rejected with an
  actionable log message and the service keeps running.
- A JVM started with `-Xrs` cannot receive the `CTRL_BREAK` thread-dump
  signal. The default `jss.threaddump.method=AUTO` notices `-Xrs` and uses
  `jcmd Thread.print` instead, so existing configurations need no change.

Details, including what a heap dump contains and how to protect it, are in
[docs/diagnostics.md](docs/diagnostics.md).

## Building from source

Requirements: Rust 1.88 or newer (`rust-toolchain.toml` selects the stable
channel with `rustfmt` and `clippy`), and a JDK 8 or newer on `PATH` to build
`wrapper.jar`, which is compiled with `--release 8`. Production machines only
need a Java runtime.

```powershell
cargo build --release                 # target\release\wrapper.exe
./scripts/build-java-bridge.ps1       # target\release\wrapper.jar
./scripts/build-release.ps1           # fmt, clippy, tests, both artifacts, version checks
```

The integration tests compile small synthetic Java applications and run real
JVMs, so they need a JDK on `PATH` (`javac`, `jar`, `jcmd`, and `jlink` are
looked up there). Without a JDK those tests skip; set
`JSS_REQUIRE_JAVA_TESTS=1` to make them fail instead, which is what CI does:

```powershell
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
$env:JSS_REQUIRE_JAVA_TESTS = "1"
cargo test --all-targets --all-features
```

CI runs this on `windows-latest` against Temurin 8, 21, and 25, plus a
Rust 1.88 build check and a license and advisory audit; see
[.github/workflows/ci.yml](.github/workflows/ci.yml). The release procedure
is in [docs/release.md](docs/release.md).

## Security

Please read [SECURITY.md](SECURITY.md) before deploying. Two points deserve
attention up front: the service runs as `LocalSystem` unless
`wrapper.ntservice.account` names a dedicated account, and the control channel
between `wrapper.exe` and the JVM is a loopback-only TCP socket protected by a
random key generated for each launch. Vulnerabilities should be reported
privately as described in that file.

## Contributing

Contributions are welcome. [CONTRIBUTING.md](CONTRIBUTING.md) explains the
development workflow, the provenance rules that keep the project independent,
and the Developer Certificate of Origin: every commit must carry a
`Signed-off-by` line (`git commit -s`).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <https://opensource.org/licenses/MIT>)

at your option. Third-party components and their licenses are listed in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this project by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

## Trademarks and affiliation

Java Service Steward is an independent project. It is not affiliated with,
endorsed by, or sponsored by Tanuki Software, Ltd. "Java Service Wrapper" and
"Tanuki Software" are names of Tanuki Software, Ltd. and are used here only to
describe compatibility with the `wrapper.conf` configuration format and
command-line conventions of that product. Java Service Steward does not
contain, bundle, or require any Tanuki Software code or binaries.

How the project was built without reference to third-party source code is
described in [docs/provenance.md](docs/provenance.md).
