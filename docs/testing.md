# Testing

## Embedded operational help

The release executable embeds a complete reference available through `-?`,
`-h`, and `--help`; all three aliases return exit code 0 and byte-identical
content without loading `wrapper.conf`. A unit test cross-checks every
recognized fixed and numbered `wrapper.*` property, plus every implemented
`jss.*` extension, against that help text so a supported property cannot be
added silently without documenting it in the executable. Another unit test
asserts that the help text and the product banner mention no other product or
version.

## JVM test matrix

Integration tests compile the bridge and the synthetic applications with the
JDK found on `PATH` (`where.exe javac.exe`) and run real JVMs. CI runs them on
`windows-latest` with Eclipse Temurin 8, 21 and 25 and sets
`JSS_REQUIRE_JAVA_TESTS=1`, which turns every "skipping because a tool is
missing" path into a failure so the matrix cannot pass vacuously. Java 8
compilation uses `-source 8 -target 8`; the resulting bridge classes retain
class-file version 52.

Some cases need a specific runtime shape: the JRE-only case needs a JDK 8 with
a `jre/` directory, and the `jlink` case needs JDK 9 or later.

## Integration tests (`tests/pure_java_bridge.rs`)

The end-to-end tests build a fresh `wrapper.jar` and synthetic applications
under `target/`, leave `wrapper.java.mainclass` on the configuration alias,
and run the supervisor in-process. They cover:

- authenticated start-up through the alias mapping, the synthetic readiness
  line, ping traffic, an orderly control stop, the `<-- Wrapper Stopped`
  marker and the absence of native-library errors;
- a real `jcmd Thread.print` capture with `-Xrs`, a nonempty HPROF through
  `jcmd GC.heap_dump`, and PID/Java-PID/Java-ID file content and cleanup;
- a blocked shutdown hook that exceeds a one-second shutdown timeout, captures
  a thread dump during the grace period, and ends in forced termination;
- Java-to-supervisor protocol logging, configuration and identifier getters,
  Java-requested stop with exit-code policy, Java-requested restart, and
  failed-invocation throttling;
- a wildcard filter that shuts the supervisor down;
- a JRE-only runtime without `jcmd` (`runtime_only_java_keeps_core_supervision_when_jcmd_is_absent`),
  which verifies precise capability errors, no false HPROF start, and a normal
  stop;
- a `java.base`-only `jlink` image (`bridge_runs_on_a_java_base_only_jlink_image`),
  which proves that the bridge needs neither `java.management` nor `jcmd`.

The 0.3.0 release adds:

- a quick-exit application whose `main` returns immediately
  (`jvm_exits_when_the_application_main_returns`): the JVM exits, the
  supervisor applies `wrapper.on_exit.*`, and the stop appears within seconds;
- `StartStopApp` with separate start and stop classes and `waitForStopThreads`
  (`start_stop_launcher_invokes_the_stop_class`);
- `JarApp` launching the `Main-Class` of an executable JAR
  (`jar_launcher_runs_the_manifest_main_class`);
- a custom main class integrating through `ServiceListener`, including a user
  control code (`service_listener_receives_start_control_and_stop`);
- high-volume output of 50,000 fast lines with no loss and no ping timeout
  (`high_volume_output_is_logged_completely_without_ping_timeouts`).

Protocol unit tests in `src/protocol.rs` cover a slow handshake in which the
client waits several hundred milliseconds and splits its key across two
writes, a rejected key followed by a successful one, a silent connection
dropped at the handshake deadline, and packets that arrive together with the
key. A supervisor unit test proves that a single output line longer than
64 KiB is split instead of growing without bound.

Rust unit tests separately cover alias mapping, the missing-bridge failure
before launch, explicit JAR detection, wildcard classpath detection, and the
exclusion of sensitive properties from the properties packet.

## Non-ASCII output

The synthetic application has an `encoding` mode that reports the JVM's
`file.encoding`, `native.encoding` and `stdout.encoding` values and prints the
fixture line `áéíóú €`. With an explicitly UTF-8 stdout the captured log
record must contain the payload bytes
`C3 A1 C3 A9 C3 AD C3 B3 C3 BA 20 E2 82 AC` unchanged, and the surrounding
records must keep the configured `LPTM` columns and CRLF terminators. A logging
unit test separately verifies that Windows-1252 bytes written by the JVM pass
through to `wrapper.log` unmodified. The logger never transcodes application
output.

## Log rotation boundary

An automated sparse-file test starts with a 50 MiB active `wrapper.log`, all 50
configured archives, and `rollmode=SIZE`. The next record moves the active file
to `.1`, shifts the prior `.1` through `.49`, removes the prior `.50`, creates a
new active log, and never creates `.51`. The logger never substitutes another
path when the configured parent directory is missing. Additional recovery tests
cover interruption during archive shifting and immediately after the active log
has moved; the next logger start recreates the active file without creating
`.51` or losing the newest archive. On Windows the active handle shares read,
write, and delete access so log readers and rotation can coexist while the
service is running.

## Backend-loss tests

The synthetic application can deliberately close the bridge's authenticated
socket. The bridge treats that loss as unrecoverable within the same JVM: it
exits and the supervisor launches another JVM subject to the restart policy.
The test observes `jvm 1`, `jvm 2`, the configured failed-launch limit, a clean
supervisor stop, and zero Java processes.

## Windows SCM cycle

An elevated manual test installs a service with a clearly prefixed test name
and removes it afterwards:

| Operation | Expected result |
| --- | --- |
| Install | manual start; `ImagePath` stores `-s wrapper.conf` and all overrides |
| Start/query | SCM `Running`; query mask 19; `jvm 1` ready |
| `-d` | thread dump captured with `LPTM` |
| Pause/query | SCM `Paused`; query mask 83; no Java child |
| Resume/query | SCM `Running`; query mask 19; `jvm 2` ready |
| Stop/query | SCM `Stopped`; query mask 17 |
| Remove | service absent; zero Java processes |

A second manual test starts the service with an invalid configuration and
checks that the failure is recorded in the Windows Event Log.

## Optional reference directory

`JSS_REFERENCE_DIR` may point to a directory outside the repository that
contains reference `*.conf` and `*.log` files. When it is set, generic tests
check that every `*.conf` parses without warnings and that every `*.log`
satisfies the `LPTM` column widths and CRLF contract. When it is unset the
tests skip with a message. No reference file is committed.

## Gotchas

- Run `cargo clean` (or at least touch `tests/*.rs`) after moving or renaming
  the repository directory: the test binaries bake `CARGO_MANIFEST_DIR` into
  the build and would otherwise look for Java sources at the old path.
- JVM tests skip when no JDK is on `PATH`, unless `JSS_REQUIRE_JAVA_TESTS=1`
  is set, in which case they fail. Set it locally to make sure the tests
  really ran.
- Tests that install or modify Windows services must use a clearly prefixed
  test service name and clean up only that exact service.
