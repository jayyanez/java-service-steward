# CLAUDE.md

Guidance for Claude Code when working in this repository. The project-wide
engineering rules live in `AGENTS.md` and apply to every agent; read them first.

@AGENTS.md

## What this project is

**Java Service Steward** is a Rust service host for Java applications on
Windows. It reads `wrapper.conf`-style configuration files (the format used by
the Java Service Wrapper), installs/controls a Windows service, launches and
supervises one JVM, captures its output into a rotated `wrapper.log`, and talks
to the JVM over an authenticated loopback socket through the project's own
Java bridge (`java/bridge`, package `io.github.jayyanez.jss.bridge`, Java 8
bytecode, no JNI, no DLL).

The deployable artifacts are `target\release\wrapper.exe` and
`target\release\wrapper.jar`. Both keep their names so they can replace an
existing installation in place. License: `Apache-2.0 OR MIT`.

## Layout

| Path | Role |
| --- | --- |
| `src/config.rs` | `wrapper.conf` parsing: includes, `#encoding`, `set.*`, `%VAR%` expansion, numbered sequences, overrides, warnings (`WarningLevel`) |
| `src/cli.rs`, `src/help.txt` | Command parsing (`-c`, `-s`, `-i`, `-t`, `-p`, `-r`, `-q`, `-d`, `--heapdump`, ...) and the embedded `--help` reference (a test checks it lists every supported property) |
| `src/jvm.rs` | Java command construction, `LAUNCHER_ALIASES` (configuration alias -> bundled launcher), bridge detection on the classpath |
| `src/protocol.rs` | Packet framing (`Framer`), non-blocking handshakes with a deadline, `Connection` with a reader thread and an event channel |
| `src/supervisor.rs` | `JvmRun` event loop (`select!` over output, controls, protocol events and a 50 ms tick): start-up deadline, ping watchdog, filters, restart throttling, shutdown, pause/resume, dumps |
| `src/logging.rs` | LPTM/PM formatting, CRLF, roll modes, filters with wildcards |
| `src/service.rs` | Windows SCM install/start/stop/query/control, the `-s` dispatcher, Event Log fallback for start-up failures |
| `src/windows_process.rs` | Console signals, process groups, `CTRL_BREAK`, console title, `JobObject`, Event Log (only `unsafe` in the crate) |
| `src/thread_dump.rs`, `src/heap_dump.rs`, `src/diagnostics.rs` | `CTRL_BREAK`/`jcmd` thread dumps, on-demand HPROF with timeout, capability detection |
| `src/telemetry.rs` | Bounded, lossy event publisher; consumers only, never lifecycle authority |
| `java/bridge/` | `SimpleApp`, `StartStopApp`, `JarApp` launchers; `Steward` API; `ServiceListener`; `BackendClient` (daemon control thread) |
| `tests/pure_java_bridge.rs`, `tests/java/` | Integration tests that compile the bridge and run real JVMs (skip without a JDK unless `JSS_REQUIRE_JAVA_TESTS=1`) |
| `tests/local_reference.rs` | Optional checks over `JSS_REFERENCE_DIR` (never inside the repo) |
| `scripts/` | `build-java-bridge.ps1`, `build-release.ps1`, `verify-version.ps1` |
| `docs/` | `compatibility.md`, `migration.md`, `provenance.md`, `architecture.md`, `testing.md`, `diagnostics.md`, `versioning.md`, `release.md`; `docs/plans/` holds review plans |
| `.github/workflows/` | CI (Windows, Temurin 8/21/25, MSRV, cargo-deny) and the tag-driven release |

## Commands

```powershell
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features      # needs javac/jar/jcmd/jlink on PATH for the JVM tests
cargo build --release
./scripts/build-java-bridge.ps1              # needs a JDK; writes target\release\wrapper.jar
./scripts/build-release.ps1                  # all of the above plus version/artifact checks
./scripts/verify-version.ps1 -RequireArtifacts
cargo about generate about.hbs -o THIRD_PARTY_NOTICES.md   # after dependency changes
```

Run a console smoke test with `target\release\wrapper.exe -c <conf>`; never
install a Windows service for an ad-hoc check unless the test name is clearly
prefixed and cleaned up (see `AGENTS.md`).

## Things that bite

- Test binaries bake `CARGO_MANIFEST_DIR` via `env!`. After moving or renaming
  the repository run `cargo clean` (or touch `tests/*.rs`), otherwise the JVM
  tests fail with `NotFound`.
- Integration tests locate a JDK with `where.exe javac.exe` and *skip* when it
  is missing. A green run without a JDK proves little; CI sets
  `JSS_REQUIRE_JAVA_TESTS=1`.
- Relative paths in `wrapper.conf` (including the conf path itself) resolve
  from the **executable's directory**, not the current directory.
- Log records are byte-exact (`LPTM`, fixed column widths, CRLF). Tests assert
  on this; do not "tidy" log output. The only stable message markers are
  `--> Wrapper Started as Service|Console` and `<-- Wrapper Stopped`.
- The wire protocol and the `-Dwrapper.*`/`-Djss.version` launch properties
  are an internal EXE<->JAR contract; change both sides together and note it
  in `CHANGELOG.md`.
- `help.txt` must mention every property in `SUPPORTED_WRAPPER_PROPERTIES`
  and every `jss.*` extension; `cli.rs` tests enforce it.

## Legal and naming rules (summary; AGENTS.md is authoritative)

- Never open, download or quote the Java Service Wrapper's source code, javadoc or
  documentation text. Interface facts come only from public docs (property and
  command semantics), from this repository's own tests, or from black-box
  observation.
- Never add the Java Service Wrapper's binaries or real deployment configurations to the
  repository. Reference material lives outside the tree (`JSS_REFERENCE_DIR`).
- "Java Service Wrapper" and "Tanuki Software" are used only nominatively in
  docs to describe compatibility, never in product names, banners, logs or
  identifiers; never cite one of its version numbers.
- `resolve_main_class` in `src/jvm.rs` maps a configured class that is absent
  from the classpath and named like a bundled launcher to that launcher; the
  code contains no third-party class names, and the project implements no
  third-party Java API.
- Everything in the repository is English.

## Plan and status

The public-release plan with rationale and task checklist is
`docs/plans/2026-08-26-public-release-plan.md`. Update its checkboxes when a
task completes; `CHANGELOG.md` records user-visible changes per version.
