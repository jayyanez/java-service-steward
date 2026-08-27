# Thread dumps and heap dumps

This document distinguishes the diagnostics inherited from the `wrapper.conf`
feature set, the JVM's own features, and Java Service Steward extensions. A
thread dump and a heap dump solve different problems and are not
interchangeable.

| Diagnostic | Origin | Content | Destination |
| --- | --- | --- | --- |
| On-demand thread dump (`-d`) | Compatible command | Threads, stacks and locks | `jvm <n>` records inside `wrapper.log` |
| Automatic heap dump on OOM | HotSpot option, not a supervisor feature | Heap objects and references | HPROF file chosen by the JVM |
| On-demand heap dump (`--heapdump`) | Java Service Steward extension | Heap objects and references | HPROF file next to `wrapper.log` unless configured otherwise |
| Thread-dump fallback through `jcmd` | Java Service Steward improvement | Threads, stacks and locks | `jvm <n>` records inside `wrapper.log` |

The compatible interface has `-d`, the property
`wrapper.thread_dump_control_code` and the filter action `DUMP`; all of them
request a **thread dump**. It has no equivalent of `--heapdump`. It has always
been possible to pass HotSpot options through `wrapper.java.additional.<n>`,
but those options are interpreted by Java, not by the supervisor.

## Compatible thread dump

For a running service:

```powershell
wrapper.exe -d wrapper.conf
```

The equivalents with the default control code are:

```powershell
wrapper.exe -l=255 wrapper.conf
sc.exe control ServiceName 255
```

The code is configured with:

```properties
wrapper.thread_dump_control_code=255
```

A thread dump does not create a separate file. Java prints it and Java Service
Steward captures it in the configured `wrapper.log`, with the same `LPTM`
columns, local timestamp, separators, CRLF and rotation rules as the rest of
the JVM output.

The historical Windows mechanism is `CTRL_BREAK`. HotSpot disables that path
when the JVM is started with `-Xrs`, an option that is common in existing
configurations. Java Service Steward keeps the historical mechanism when it is
available and, with the default `AUTO` mode, uses `jcmd Thread.print`
automatically when it detects `-Xrs`. Those `wrapper.conf` files need no
change.

The optional extensions are:

```properties
# AUTO: BREAK normally; JCMD when -Xrs is present. This is the default.
jss.threaddump.method=AUTO

# Alternative values for controlled diagnostics:
# jss.threaddump.method=BREAK
# jss.threaddump.method=JCMD

# Maximum time for jcmd Thread.print, in seconds. Default: 30.
jss.threaddump.timeout=30
```

`BREAK` forces the `CTRL_BREAK` mechanism and does not work when Java was
started with `-Xrs`. `JCMD` forces the modern mechanism. `AUTO` is the
recommended value.

## Automatic heap dump on out-of-memory

The option that already appears in many configurations is:

```properties
wrapper.java.additional.<n>=-XX:+HeapDumpOnOutOfMemoryError
```

This feature belongs to HotSpot. It does not require `wrapper.jar` or the new
command. Without `-XX:HeapDumpPath`, Java chooses the name
`java_pid<pid>.hprof` in the JVM working directory, which is not guaranteed to
be the `wrapper.log` directory.

The file is generated only when HotSpot detects a qualifying memory error. It
does not replace a heap dump requested during an investigation.

## On-demand heap dump: Java Service Steward extension

For a running service:

```powershell
wrapper.exe --heapdump wrapper.conf
```

The project's own control code can also be sent directly:

```powershell
wrapper.exe -l=254 wrapper.conf
sc.exe control ServiceName 254
```

The optional properties are:

```properties
# SCM control code, between 128 and 255. Default: 254.
jss.heapdump.control_code=254

# Path relative to the wrapper.exe directory, like wrapper.logfile.
jss.heapdump.directory=../standalone/log

# Maximum time for jcmd GC.heap_dump, in seconds. Default: 600.
jss.heapdump.timeout=600
```

The heap-dump code must differ from `wrapper.thread_dump_control_code`. If a
code is changed while the service is running, restart the service so that the
active process re-reads `wrapper.conf`.

None of these properties is mandatory. Without `jss.heapdump.directory`, the
file is created in the directory of the active `wrapper.log`. For example:

```text
heap-20260826-101530-jvm1-pid12345.hprof
```

The name includes the local date, the JVM invocation number and the PID. An
existing file is never overwritten. Only one request runs at a time. The
supervisor writes `Heap dump requested` and `Heap dump completed` to
`wrapper.log`; a request made before the application has reported `STARTED` is
rejected without restarting Java. A partial file is removed when `jcmd`
reports a failure, and `jcmd` is terminated when `jss.heapdump.timeout` is
exceeded.

## JRE, JDK and reduced runtime images

The normal lifecycle does not require a JDK. To install and run the service it
is enough that `wrapper.java.command` selects a `java.exe` capable of running
the application and the bundled `wrapper.jar`. `javac.exe` and `jar.exe` are
not used in production; they are only needed to build the bridge JAR.

The runtime is not classified by the names `JRE` or `JDK` or by its directory.
Concrete capabilities are checked. A runtime created with `jlink` may contain a
different set of modules, and a distribution called a JRE may or may not ship
additional tools.

Core supervision (SCM, start, stop, restart, pings, output capture,
`wrapper.log`, rotation and filters) does not depend on `jcmd`.

The on-demand heap dump and the `-Xrs` thread-dump fallback run `jcmd` against
the PID of the supervised JVM:

- if `wrapper.java.command` contains a path, `jcmd.exe` is looked up next to
  that `java.exe`;
- if the command is a bare `java`, `jcmd.exe` is looked up through `PATH`;
- the account running the supervisor must be able to attach to the JVM, which
  is normally the case because both processes run under the same service
  account.

The tool must match the JVM version. A `jcmd` belonging to a different JDK is
not searched for automatically: using the tools of one JDK to diagnose a JVM of
another version is not supported by the JDK vendors.

Two further deliberate limitations apply:

- `-Xrs` disables the Windows `CTRL_BREAK` thread dump; with `AUTO`, `jcmd` is
  attempted, while a forced `BREAK` is rejected explicitly;
- `-XX:+DisableAttachMechanism` prevents `jcmd` from attaching even when the
  executable is installed.

If `jcmd` does not exist, is disabled, or cannot attach, only the diagnostic
request fails. An actionable `ERROR` is written to `wrapper.log`; the
application keeps running, is not restarted, and no nonexistent heap dump is
created or announced. During a forced shutdown the dump delay is not consumed
either when the request was rejected immediately.

`wrapper.exe -d` and `wrapper.exe --heapdump` send an asynchronous SCM
control. Their exit code confirms that Windows delivered the request to the
service, not that the diagnostic has finished. The definitive result is in
`wrapper.log`.

With a JRE that has no `jcmd`, a thread dump still works through `CTRL_BREAK`
if the JVM supports it and `-Xrs` is not configured. HotSpot's automatic heap
dump through `-XX:+HeapDumpOnOutOfMemoryError` is also independent of `jcmd`;
its availability depends on the JVM recognizing that option.

## Impact and operational safety

A thread dump is usually brief, although `jcmd` may pause the JVM briefly. A
heap dump is a heavy operation: it can cause a pause and produces a file close
to the size of the used heap. Before requesting one, check the free space on
the volume and wait for the completion record.

An HPROF file can contain passwords, tokens, user data and any other object that
was in memory. Protect it with the same controls as a production secret,
transfer it only through authorized channels, and delete it securely when the
analysis is finished. Java Service Steward does not rotate or delete completed
heap dumps automatically.

These operations already publish bounded internal events for future telemetry,
but remote transport and remote commands remain disabled. Telemetry does not
gain authority over the Java lifecycle by enabling these diagnostics.

## External references

- In the original configuration format, `-d`, `wrapper.thread_dump_control_code`
  and the `DUMP` filter action all refer to a thread dump, never to a heap dump;
  the public documentation pages consulted are listed in
  [research.md](research.md).
- Oracle documents `Thread.print` and `GC.heap_dump` in the
  [`jcmd` reference for Java 21](https://docs.oracle.com/en/java/javase/21/docs/specs/man/jcmd.html).
- Oracle documents `-Xrs`, `-XX:+HeapDumpOnOutOfMemoryError` and
  `-XX:HeapDumpPath` in the
  [Java 21 launcher reference](https://docs.oracle.com/en/java/javase/21/docs/specs/man/java.html).
- For Java 8, Oracle also documents
  [`HeapDumpOnOutOfMemoryError`](https://docs.oracle.com/javase/8/docs/technotes/guides/troubleshoot/clopts001.html).
