# AGENTS.md

## Mission

Build **Java Service Steward**, a resource-efficient Rust service host and
lifecycle monitor for Java applications on Windows. It reads
`wrapper.conf`-style configuration files, honors the same command-line
conventions and log format, and ships its own Java bridge (`wrapper.jar`).

The executable remains `wrapper.exe`, the bridge remains `wrapper.jar`, project
extensions use the `jss.*` property prefix, the Java bridge uses
`io.github.jayyanez.jss.bridge`, and the Rust package is `java-service-steward`.

An existing installation must be able to migrate by replacing `wrapper.exe` and
`wrapper.jar` while keeping its `wrapper.conf` and service registration.
Longer-term telemetry and fleet-control features must remain isolated from the
compatibility core.

## Non-negotiable compatibility rules

- Target Windows x64 first.
- Resolve a relative configuration path from the directory containing the
  executable, not from the caller's current directory.
- Support the documented command-line surface (see `docs/compatibility.md`)
  and the internal `-s` service command stored in an installed service's
  `ImagePath`.
- Preserve command-line property overrides and `--` application argument
  pass-through behavior.
- Treat a production JRE or reduced runtime as valid. Core service supervision
  must require only the configured Java launcher and application runtime APIs;
  never assume `javac`, `jar`, `jcmd`, or another JDK tool is installed.
- Detect diagnostic capabilities individually. An unavailable optional dump or
  attach mechanism must produce an actionable error without stopping,
  restarting, or otherwise destabilizing the Java application.
- Never silently reinterpret an unsupported `wrapper.*` property. Warn with the
  property name and continue only when ignoring it is safe. Properties that
  are deliberately accepted and ignored are listed in `docs/compatibility.md`.
- The control backend must be independently testable without installing a
  Windows service.
- Do not add telemetry behavior to the JVM lifecycle state machine directly.
  Publish internal events and let telemetry consume them.

## Security and privacy

- Never commit real `wrapper.conf` files supplied by users. Fixtures must contain
  synthetic values and obvious placeholders.
- Treat any property whose name contains `password`, `secret`, `token`, `key`,
  `credential`, or `vault` as sensitive. Redact its value from logs and
  telemetry and do not send it to the JVM.
- Bind the JVM control socket to loopback only and authenticate the child JVM
  before accepting lifecycle messages.
- Remote control must default to disabled. Later implementations must use
  authenticated, encrypted transport and an explicit command allowlist.
- Do not log the generated backend key or a full Java command line containing
  sensitive system properties.

## Legal boundary

- The license is `Apache-2.0 OR MIT`. Every source file carries
  `SPDX-License-Identifier: Apache-2.0 OR MIT`. Contributions are accepted
  under the same terms.
- Never consult the Java Service Wrapper's source code, javadoc, decompiled binaries, or
  documentation text while working on this project, and never copy any of it.
- Interface facts come only from public property and command documentation,
  this repository's tests, and black-box observation of externally visible
  behavior. If a needed fact is not available from those sources, stop and ask.
- Never add artifacts of the Java Service Wrapper (executables, JARs, DLLs) or real
  deployment configurations to the repository.
- "Java Service Wrapper" and "Tanuki Software" may be used only nominatively,
  in documentation, to describe compatibility. Never in banners, `--version`,
  log output, code identifiers, file names, or the product name. The
  configuration alias values in `src/jvm.rs` are configuration data and are
  the only exception in code.
- Never cite a Java Service Wrapper version number. Describe compatibility as
  "the Community Edition feature set" or property by property.
- Everything in the repository is written in English.
- Keep third-party license notices for every dependency and bundled component.

## Architecture boundaries

- `config`: parsing, includes, environment expansion, typed compatibility views.
- `cli`: command parsing and exit-code behavior.
- `jvm`: Java command construction, main-class aliasing, and child process
  ownership.
- `protocol`: authenticated local communication with the bridge.
- `supervisor`: lifecycle state machine, timeouts, restarts, and shutdown.
- `logging`: stdout/stderr capture, rotation, filtering, and redaction.
- `service`: Windows Service Control Manager integration.
- `telemetry`: event consumers and remote transport; no lifecycle authority.

Keep platform-neutral logic testable on any Rust host. Windows API calls belong
behind the `service` and process-platform boundaries.

## Engineering practices

- Prefer safe Rust. Any `unsafe` block must be small, documented with its safety
  invariants, and covered by a higher-level test where practical.
- Avoid global mutable state.
- Avoid unbounded channels, buffers, log queues, and retry loops.
- Add dependencies only when they reduce meaningful risk or complexity.
- Preserve low idle CPU and memory use; record benchmark methodology when a
  telemetry loop is introduced.
- Use structured error types internally and concise actionable messages at the
  CLI boundary.

## Verification

Before considering a change complete, run:

```powershell
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Compatibility work must also include integration tests that run real JVMs;
CI sets `JSS_REQUIRE_JAVA_TESTS=1` so those tests fail instead of skipping
when a JDK tool is missing. Tests that install or modify Windows services must
use a clearly prefixed test service name and must clean up only that exact
service.

Java 8, 21 and 25 are the CI matrix. Java 8 support must never force a weaker
design or reduce support for the modern runtimes.

## Documentation

Update `docs/compatibility.md` whenever support for a property, command, or
integration method changes. Record deliberate deviations and their migration
impact in `docs/migration.md`.
