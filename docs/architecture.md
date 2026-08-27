# Architecture

Java Service Steward is split into a small compatibility core and optional event
consumers. The Windows service and console front ends feed the same supervisor.

```text
CLI / Windows SCM
        |
        v
 config -> main-class aliasing -> supervisor -> JVM child process
                                   |               |
                                   |               +-> stdout/stderr -> logging -> filters
                                   |
                                   +<-> authenticated loopback protocol
                                   |              |
                                   |              v
                                   |     Java bridge (wrapper.jar)
                                   |
                                   +-> bounded internal events -> telemetry (optional)
```

## Lifecycle ownership

Only the supervisor may launch, stop, kill, or restart the JVM. Logging and
telemetry can request an action through typed, bounded channels, but they cannot
mutate lifecycle state directly. This keeps remote telemetry failures from
affecting local service reliability.

## Design order: compatibility core first, telemetry as an optional consumer

The compatibility core (configuration, protocol, supervisor, logging, service
integration) is complete and tested before any telemetry transport exists.
Telemetry is developed as an optional, default-disabled consumer of bounded
lifecycle events and never becomes an owner of the lifecycle.

## Main-class aliasing

While constructing the Java command, `resolve_main_class` in `src/jvm.rs`
resolves `wrapper.java.mainclass`: a bundled launcher named explicitly is used
as is; a class that is not on the effective classpath and whose simple name
ends with `SimpleApp`, `StartStopApp` or `JarApp` is replaced by the matching
bundled launcher, and the substitution is written to the log; any other class
is launched exactly as configured. Only the launched main-class token changes;
the configuration text, the application main class and all numbered parameters
remain untouched. The rule is generic and the code contains no third-party
class names.

When a launcher is selected, the effective classpath must contain the bridge
(a JAR, a class directory, or a `dir/*` wildcard entry containing
`io/github/jayyanez/jss/bridge/SimpleApp.class`). If it does not, the
supervisor fails before launching Java with a message naming
`wrapper.java.classpath`. Any other main class is launched as configured and is
expected to call `Steward.start(listener, args)` itself.

The bridge defines classes only in its own package. It implements the local
packet protocol in Java, runs the control loop on a daemon thread named
`jss-control`, and never calls `System.loadLibrary`. Windows service ownership,
process control, thread dumps, logging and restart policy remain in Rust. The
JVM exits when the application has no non-daemon threads left; the shutdown
hook then reports the stop to the supervisor.

The supervisor passes `jss.version` and the `wrapper.*` launch properties
(key, ports, PID, JVM id, service flag, debug and listener flags) to the JVM as
system properties. Sensitive configuration properties are excluded from the
properties packet.

## Diagnostic operations

Thread dumps and heap dumps are owned by the Rust supervisor, independently of
the bridge. The default thread-dump path targets the Java process group with
`CTRL_BREAK`; `-Xrs` configurations automatically use a bounded, timed
`jcmd Thread.print` capture instead. On-demand HPROF creation runs
`jcmd GC.heap_dump` on a worker thread, bounded by `jss.heapdump.timeout`, so
long heap writes do not block ping handling. Both paths publish bounded
diagnostic events for future telemetry, which remains a consumer rather than a
lifecycle owner. Operational usage and the distinction from HotSpot's OOM
option are documented in [the diagnostics guide](diagnostics.md).

## Process containment

By default (`jss.java.job_object=true`) the JVM is assigned to a Windows Job
Object with kill-on-close, so the JVM and any processes it spawns end when the
supervisor ends. Environment variable names read from `set.*` are normalized to
upper case before being exported to the JVM; that is correct on Windows, where
names are case-insensitive, and would need to change if Unix were ever
supported.

## Resource model

- One supervisor process per Java application.
- One child JVM.
- One loopback listener with one authenticated active JVM connection.
- Bounded channels for log lines and internal events.
- Streaming log handling; no complete log file is retained in memory. Output
  lines are capped at 64 KiB and split when longer.
- Timers driven by monotonic time.
