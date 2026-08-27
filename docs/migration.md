# Migrating an existing installation

This guide is for Windows services that already run a Java application with a
`wrapper.conf` file, a `wrapper.exe` and a `wrapper.jar` from the Java Service
Wrapper. Java Service Steward reads the same configuration and registers the
same command line, so the switch is a file replacement. See
[compatibility.md](compatibility.md) for the full property table.

## In-place replacement

1. Stop the service (`wrapper.exe -p wrapper.conf` or the Services console).
2. Back up `wrapper.exe`, `wrapper.jar` and, if present, `wrapper.dll`.
3. Copy the new `wrapper.exe` over the old executable.
4. Copy the new `wrapper.jar` over the JAR named by
   `wrapper.java.classpath.<n>` (for example `lib/wrapper.jar` or
   `bin/wrapper.jar`). The path must stay the same; only the content changes.
5. Optionally delete `wrapper.dll`. It is never loaded.
6. Start the service (`wrapper.exe -t wrapper.conf`).

`wrapper.conf` and the service `ImagePath` need no change: the installed
service already invokes `wrapper.exe -s <conf> [overrides]`, and the new
executable accepts that command line. Check `wrapper.log` for the
`--> Wrapper Started as Service` marker, the product banner on the next line,
and the application's own readiness output.

If the classpath does not contain the new `wrapper.jar`, the supervisor stops
before launching Java with a message naming `wrapper.java.classpath`. That is
the expected signal that step 4 was skipped or targeted the wrong path.

### Rollback

Stop the service, restore the backed-up `wrapper.exe`, `wrapper.jar` and
`wrapper.dll`, and start the service again. Nothing else was modified.

### Testing on the side first

To test without touching the installed service, copy the application directory,
edit `wrapper.ntservice.name` and `wrapper.logfile` in the copy, and run
`wrapper.exe -c wrapper.conf` from the copy. The console run uses the same
configuration parser, launcher and protocol as the service.

## Properties that change behavior

Most properties keep their meaning. The following, which are common in
existing files, are recognized but have no effect:

| Property | Status | Why |
| --- | --- | --- |
| `wrapper.native_library` | Accepted, ignored | No native library is loaded |
| `wrapper.cpu.timeout` | Accepted, ignored | Not used by the supervisor |
| `wrapper.java.additional.auto_bits` | Accepted, ignored | No `-d32`/`-d64` flag is injected on x64 |
| `wrapper.internal.namedpipe` | Accepted, ignored | Only the loopback socket backend exists |
| `wrapper.syslog.loglevel` | Accepted, ignored | Sets the minimum level negotiated with the bridge only; no syslog or Event Log sink |
| `wrapper.license.*` | Accepted, ignored | Licensing properties from other editions; one INFO line at startup |

Each family produces one INFO line at startup. Any other `wrapper.*` property
that is not in the supported list (for example properties that belong to
Standard or Professional edition features such as event commands, timers or
JMX) produces one WARN line naming the property, in the console and in
`wrapper.log`, and is ignored. Review those warnings once after the first
start; they are not errors.

The property `jss.java.bridge`, which existed in internal development builds,
was removed. Delete it if present.

## Integration methods

`wrapper.java.mainclass` keeps its existing value. A configured class that is
absent from the classpath and whose name ends in `SimpleApp`, `StartStopApp` or
`JarApp` is launched through the bundled launcher of that name (the
substitution is written to `wrapper.log`), so the values found in existing
files map as follows:

| Existing `wrapper.java.mainclass` value | Launched class |
| --- | --- |
| `org.tanukisoftware.wrapper.WrapperSimpleApp` | `io.github.jayyanez.jss.bridge.SimpleApp` |
| `org.tanukisoftware.wrapper.WrapperStartStopApp` | `io.github.jayyanez.jss.bridge.StartStopApp` |
| `org.tanukisoftware.wrapper.WrapperJarApp` | `io.github.jayyanez.jss.bridge.JarApp` |

New configurations may name the bridge classes directly.

- **Simple application.** `wrapper.app.parameter.1` is the application main
  class; the remaining parameters are its arguments. When `main` returns and
  no non-daemon threads remain, the JVM exits and `wrapper.on_exit.*` applies.
- **Start/stop application.** The parameters keep the shape
  `<startClass> <nStartArgs> <startArgs...> <stopClass> <waitForStopThreads> <nStopArgs> <stopArgs...>`.
  For example:

  ```properties
  wrapper.java.mainclass=io.github.jayyanez.jss.bridge.StartStopApp
  wrapper.app.parameter.1=com.example.Server
  wrapper.app.parameter.2=1
  wrapper.app.parameter.3=--config=server.xml
  wrapper.app.parameter.4=com.example.Shutdown
  wrapper.app.parameter.5=true
  wrapper.app.parameter.6=0
  ```

  On stop, `com.example.Shutdown.main(new String[0])` is invoked and, because
  `waitForStopThreads` is `true`, the JVM waits until no non-daemon
  application threads remain.
- **Executable JAR.** `wrapper.app.parameter.1` is the JAR path and the rest
  are arguments. The `Main-Class` manifest attribute is loaded through a
  `URLClassLoader` whose parent is the system class loader.

## Applications that call the original Java API

Java Service Steward does not provide the `org.tanukisoftware.wrapper` package.
An application that only runs through one of the launchers above needs no code
change. An application whose own code imports that API (for example to
implement the listener interface, to request a stop with an exit code, or to
read the wrapper's properties) must be changed at the source level to the
bridge's own API, and the bundled `wrapper.jar` must be on its compile
classpath.

The conceptual mapping is:

| Need | Java Service Steward API |
| --- | --- |
| Lifecycle callbacks from a custom main class | Implement `ServiceListener`; call `Steward.start(listener, args)` |
| Stop with an exit code / request a restart | `Steward.stop(int)` / `Steward.restart()` |
| Extend the startup deadline | `Steward.signalStarting(int waitHintMillis)` |
| Log through the supervisor | `Steward.log(int level, String message)` with `Steward.LOG_INFO` and friends |
| Read configuration and identity | `Steward.getProperties()`, `getLogFile()`, `getJvmId()`, `getWrapperPid()`, `getJavaPid()`, `isLaunchedAsService()`, `isManaged()`, `getVersion()` |
| React to pause, resume and user control codes | `ServiceListener.controlEvent(int)` with `Steward.CONTROL_PAUSE`, `Steward.CONTROL_RESUME`, or a code in 128..255 |

A minimal listener:

```java
package com.example;

import io.github.jayyanez.jss.bridge.ServiceListener;
import io.github.jayyanez.jss.bridge.Steward;

public final class ServiceMain implements ServiceListener {

    private Server server;

    public static void main(String[] args) {
        Steward.start(new ServiceMain(), args);
    }

    @Override
    public Integer start(String[] args) {
        server = new Server(args);
        server.start();
        return null; // keep running; return an exit code to stop immediately
    }

    @Override
    public int stop(int exitCode) {
        server.shutdown();
        return exitCode; // the exit code the JVM will use
    }

    @Override
    public void controlEvent(int event) {
        if (event == Steward.CONTROL_PAUSE) {
            server.pause();
        } else if (event == Steward.CONTROL_RESUME) {
            server.resume();
        }
    }
}
```

Set `wrapper.java.mainclass=com.example.ServiceMain` for such an application.
The class is launched exactly as configured; `Steward.start` connects to the
supervisor, reports `STARTED` after `start` returns, answers pings, and calls
`stop` when the service is stopped. A custom main class that never calls
`Steward.start` never connects, so the supervisor applies
`wrapper.startup.timeout` and restarts the JVM.

`Steward.isManaged()` returns `false` when the class is run outside the
supervisor (for example from an IDE), which allows the same main class to be
used in both situations.

## Diagnostics differences

- **Thread dumps with `-Xrs`.** Many existing configurations pass `-Xrs`,
  which disables the `CTRL_BREAK` thread-dump path in HotSpot. With the default
  `jss.threaddump.method=AUTO`, `wrapper.exe -d` and the `DUMP` filter action
  then use `jcmd Thread.print` and capture the output into `wrapper.log`. This
  needs `jcmd.exe` next to the configured `java.exe` or on `PATH`; a JRE
  without it reports an actionable error and the application keeps running.
- **Heap dumps on demand.** `wrapper.exe --heapdump wrapper.conf` (control
  code 254 by default) writes an HPROF file next to `wrapper.log` through
  `jcmd GC.heap_dump`. This is an extension; `-XX:+HeapDumpOnOutOfMemoryError`
  entries in `wrapper.java.additional.<n>` keep working as plain JVM options.
- **Log messages.** Supervisor messages are worded differently. Monitoring
  rules should rely on the kept markers `--> Wrapper Started as Service`,
  `--> Wrapper Started as Console` and `<-- Wrapper Stopped`, on the `LPTM`
  columns, and on the application's own output, not on the old message
  catalog.
- **Event Log.** A service that fails before it can open `wrapper.log` (for
  example because of a configuration error) leaves a record in the Windows
  Event Log.

See [diagnostics.md](diagnostics.md) for the details.

---

Java Service Steward is an independent project; product names are used only
to describe compatibility. See the "Trademarks and affiliation" section of the
[README](../README.md#trademarks-and-affiliation).
