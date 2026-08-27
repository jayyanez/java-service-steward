# Compatibility with the Java Service Wrapper configuration format

## Scope

Java Service Steward is designed so that an existing Windows installation that
uses a `wrapper.conf` file can switch to it by replacing two files:
`wrapper.exe` and `wrapper.jar`. Compatibility means four things:

1. **Configuration format.** The `wrapper.conf` syntax (directives, `set.*`
   definitions, `%NAME%` expansion, numbered sequences, includes) and the
   property families listed below are read with the same meaning.
2. **Command line.** The commands, aliases, exit codes and query masks listed
   below behave the same way, including the hidden `-s` command stored in the
   service `ImagePath` of an installed service.
3. **Log format.** `wrapper.log` keeps the `LPTM` layout, CRLF line endings,
   the roll modes and archive numbering, and two start/stop markers that
   monitoring tools commonly look for.
4. **Service semantics.** Startup, ping, shutdown, restart throttling, filter
   actions, exit-code actions, pause/resume and Windows service registration
   follow the Community Edition feature set.

Compatibility does **not** mean:

- that any code from the Java Service Wrapper is included; the executable and the bridge are
  the project's own implementation;
- that the original `wrapper.jar` or `wrapper.dll` can be used; only the bundled
  `wrapper.jar` is supported, and no native library is loaded;
- that the `org.tanukisoftware.*` Java API is provided; applications that
  import it need a source change (see [migration.md](migration.md)).

Where the semantics of a property differ between versions of the original
product, this document describes the behavior implemented here.

## Command-line surface

```text
wrapper.exe <command> <configuration file> [name=value ...] [-- <application args>]
wrapper.exe <configuration file> [name=value ...] [-- <application args>]
wrapper.exe <command>                       (configuration defaults to wrapper.conf)
wrapper.exe                                 (equivalent to -c wrapper.conf)
```

| Command | Meaning |
| --- | --- |
| `-c`, `--console` | Run the supervisor and the Java application in this console |
| `-s`, `--service` | Internal SCM entry point; stored in the service `ImagePath` |
| `-t`, `--start` | Start the installed service and wait for `Running` |
| `-a`, `--pause` | Pause a service configured as pausable |
| `-e`, `--resume` | Resume a paused service |
| `-p`, `--stop` | Stop the installed service and wait for `Stopped` |
| `-i`, `--install` | Install the service described by the configuration |
| `-it`, `--installstart` | Install and then start the service |
| `-r`, `--remove` | Stop and remove the configured service |
| `-l=<code>`, `--controlcode=<code>` | Send a user control code in the range 128..255 |
| `-d`, `--dump` | Request a thread dump from the running service (control code 255 by default) |
| `--heapdump` | Extension: request an HPROF heap dump from the running service (control code 254 by default) |
| `-q`, `--query` | Print the service state and return a status mask |
| `-qs`, `--querysilent` | Return the status mask without printing |
| `-v`, `--version` | Print the three-line product banner |
| `-?`, `-h`, `--help` | Print the embedded reference; no configuration is read |
| `--` | End of wrapper arguments; the remainder goes to the Java application |

Query mask bits: 1 = installed, 2 = running, 4 = interactive, 8 = automatic
start, 16 = manual start, 32 = disabled, 64 = paused. An absent service returns
0. Install, remove and some control commands require an elevated shell.

`--version` prints exactly:

```text
Java Service Steward 64-bit 0.3.0
Runs, controls, and monitors Java applications as Windows services.
Reads wrapper.conf-style configuration files.
```

## Configuration properties

Status values: **Supported** (implemented with the documented meaning),
**Accepted, ignored** (recognized; one INFO line per property family at
startup; no effect), **Warns, ignored** (a WARN line naming the property; no
effect). Unknown `wrapper.*` and `jss.*` names fall into the last category;
the wrapper never refuses to start because of an unknown property. Any other
property name is treated as an application property: it is kept and made
available to the JVM through `Steward.getProperties()` unless its name looks
sensitive.

### Configuration syntax

| Element | Status | Notes |
| --- | --- | --- |
| `name=value` | Supported | |
| `set.NAME=value`, `set.default.NAME=value` | Supported | Child environment and `%NAME%` expansion |
| `%NAME%` expansion | Supported | Names may contain dots |
| `#encoding=<charset>` (first line) | Supported | |
| `#include`, `#include.required`, `#include.debug` | Supported | Nesting limited to 10 levels |
| Trailing `\` continuation, `##` literal hash | Supported | |
| Command-line `name=value` overrides | Supported | Stored in `ImagePath` at install time |

### Java launch

| Property | Status | Notes |
| --- | --- | --- |
| `wrapper.java.command` | Supported | Bare `java` resolves through `PATH`; relative paths resolve from the EXE directory |
| `wrapper.java.mainclass` | Supported | Configuration aliases are mapped to the bundled bridge; see Integration methods |
| `wrapper.java.classpath.<n>` | Supported | Must contain the bundled `wrapper.jar` when a launcher alias is used |
| `wrapper.java.library.path.<n>` | Supported | |
| `wrapper.java.additional.<n>` | Supported | Windows quoting applies |
| `wrapper.java.additional.auto_bits` | Accepted, ignored | No `-d32`/`-d64` flag is injected on x64 |
| `wrapper.java.initmemory` | Supported | Adds `-Xms` when greater than zero |
| `wrapper.java.maxmemory` | Supported | Adds `-Xmx` when greater than zero |
| `wrapper.app.parameter.<n>` | Supported | |
| `wrapper.working.dir` | Supported | Default: EXE directory |
| `wrapper.java.command.loglevel` | Supported | Sensitive `-D` values are redacted |
| `wrapper.debug` | Supported | |
| `wrapper.ignore_sequence_gaps` | Supported | |
| `wrapper.use_system_time` | Supported | Forwarded to the bridge |
| `wrapper.disable_console_input` | Supported | Forwarded to the bridge |
| `wrapper.listener.force_stop` | Supported | Forwarded to the bridge |
| `wrapper.disable_shutdown_hook` | Supported | Forwarded to the bridge |
| `wrapper.native_library` | Accepted, ignored | No native library is loaded |
| `wrapper.cpu.timeout` | Accepted, ignored | |

### Local control protocol

| Property | Status | Notes |
| --- | --- | --- |
| `wrapper.port` | Supported | 0 or unset scans the range |
| `wrapper.port.min`, `wrapper.port.max` | Supported | Defaults 32000-32999 |
| `wrapper.jvm.port` | Supported | Optional fixed client port |
| `wrapper.jvm.port.min`, `wrapper.jvm.port.max` | Supported | Defaults 31000-31999 |
| `wrapper.internal.namedpipe` | Accepted, ignored | Only the loopback socket backend exists |

### Startup, ping, shutdown and restart

| Property | Status | Notes |
| --- | --- | --- |
| `wrapper.startup.timeout` | Supported | Default 30; 0 disables |
| `wrapper.startup.delay` | Supported | Default 0 |
| `wrapper.startup.delay.console`, `wrapper.startup.delay.service` | Supported | Mode-specific overrides |
| `wrapper.shutdown.timeout` | Supported | Default 30 |
| `wrapper.ping.interval` | Supported | Default 5, minimum 1 |
| `wrapper.ping.timeout` | Supported | Default 30; 0 disables |
| `wrapper.restart.delay` | Supported | Default 5 |
| `wrapper.max_failed_invocations` | Supported | Default 5 |
| `wrapper.successful_invocation_time` | Supported | Default 300 |
| `wrapper.disable_restarts` | Supported | |
| `wrapper.disable_restarts.automatic` | Supported | |
| `wrapper.on_exit.default` | Supported | `SHUTDOWN` (default), `RESTART`, `PAUSE` |
| `wrapper.on_exit.<code>` | Supported | Per-exit-code override |
| `wrapper.pause_on_startup` | Supported | |
| `wrapper.request_thread_dump_on_failed_jvm_exit` | Supported | |
| `wrapper.request_thread_dump_on_failed_jvm_exit.delay` | Supported | Default 5 |

### PID and invocation files

| Property | Status | Notes |
| --- | --- | --- |
| `wrapper.pidfile` | Supported | Decimal PID plus CRLF; removed at exit |
| `wrapper.pidfile.strict` | Supported | Fails when the file already exists |
| `wrapper.java.pidfile` | Supported | Lifetime of the current JVM |
| `wrapper.java.idfile` | Supported | JVM invocation number plus CRLF |

### Log file and console

| Property | Status | Notes |
| --- | --- | --- |
| `wrapper.logfile` | Supported | Default `wrapper.log`, resolved from the EXE directory |
| `wrapper.logfile.format` | Supported | Default `LPTM` |
| `wrapper.logfile.loglevel` | Supported | Default `INFO` |
| `wrapper.logfile.rollmode` | Supported | `NONE`, `SIZE`, `WRAPPER`, `JVM`, `SIZE_OR_WRAPPER`, `SIZE_OR_JVM` |
| `wrapper.logfile.maxsize` | Supported | For example `50m`, `20mb`, `2g` |
| `wrapper.logfile.maxfiles` | Supported | 0 keeps an unlimited history |
| `wrapper.console.format` | Supported | Default `PM` |
| `wrapper.console.loglevel` | Supported | Default `INFO` |
| `wrapper.console.flush` | Supported | Default `false` |
| `wrapper.console.title` | Supported | |
| `wrapper.console.title.windows` | Supported | Overrides `wrapper.console.title` on Windows |
| `wrapper.syslog.loglevel` | Accepted, ignored | The value still sets the minimum level negotiated with the bridge; no Event Log sink is written for it |

### Output filters

| Property | Status | Notes |
| --- | --- | --- |
| `wrapper.filter.trigger.<n>` | Supported | First matching filter wins |
| `wrapper.filter.allow_wildcards.<n>` | Supported | `*` and `?`; default `false` |
| `wrapper.filter.action.<n>` | Supported | `NONE`, `DEBUG`, `DUMP`, `GC`, `RESTART`, `SHUTDOWN`, `PAUSE`, `RESUME`; chained with comma or space |
| `wrapper.filter.message.<n>` | Supported | |

### Windows service

| Property | Status | Notes |
| --- | --- | --- |
| `wrapper.ntservice.name` | Supported | Required for service commands |
| `wrapper.ntservice.displayname` | Supported | Default: service name |
| `wrapper.ntservice.description` | Supported | |
| `wrapper.ntservice.starttype` | Supported | `AUTO_START` (default), `DEMAND_START`, `DISABLED`; `AUTO`/`MANUAL` accepted |
| `wrapper.ntservice.dependency.<n>` | Supported | `+name` denotes a group |
| `wrapper.ntservice.account` | Supported | Default LocalSystem |
| `wrapper.ntservice.password` | Supported | Used at install time only; never stored |
| `wrapper.ntservice.interactive` | Supported | |
| `wrapper.ntservice.generate_console` | Supported | Default `true` |
| `wrapper.pausable`, `wrapper.ntservice.pausable` | Supported | Default `false` |
| `wrapper.pausable.stop_jvm`, `wrapper.ntservice.pausable.stop_jvm` | Supported | Default `true` |
| `wrapper.thread_dump_control_code` | Supported | Default 255; 0 disables |

### Licensing properties

| Property | Status | Notes |
| --- | --- | --- |
| `wrapper.license.*` | Accepted, ignored | One INFO line at startup; files from Standard or Professional installations do not produce one warning per line |

### Everything else

| Property | Status | Notes |
| --- | --- | --- |
| Any other `wrapper.*` | Warns, ignored | One WARN line naming the property, in the console and in `wrapper.log` |

### Project extensions (`jss.*`)

Two prefixes exist on purpose. `wrapper.*` names are the shared configuration
format: a property keeps that name only when it exists in the original format
with the same meaning, so files stay interchangeable. `jss.*` names are
features that exist only in Java Service Steward; a file that uses one is no
longer portable to other hosts, and the prefix makes that visible at a glance.
No `jss.*` property is ever required: every extension has a default that
reproduces the original behavior, and an existing file needs none of them. The
same rule applies to commands (`--heapdump` is an extension) and to the Java
API (`io.github.jayyanez.jss.bridge.*`).

| Property | Status | Notes |
| --- | --- | --- |
| `jss.threaddump.method` | Supported | `AUTO` (default), `BREAK`, `JCMD` |
| `jss.threaddump.timeout` | Supported | Seconds; default 30 |
| `jss.heapdump.control_code` | Supported | 128..255; default 254; must differ from the thread-dump code |
| `jss.heapdump.directory` | Supported | Default: directory of the active `wrapper.log` |
| `jss.heapdump.timeout` | Supported | Seconds; default 600; `jcmd` is killed when exceeded |
| `jss.java.job_object` | Supported | Default `true`; the JVM and its children are placed in a Windows Job Object that is closed with the supervisor |

## Integration methods

`wrapper.java.mainclass` selects the launcher. A configured class that is not
on the classpath and whose simple name ends in `SimpleApp`, `StartStopApp` or
`JarApp` is launched through the bundled launcher of that name, so the values
found in existing configuration files map as follows:

| Configured value | Launched class |
| --- | --- |
| `org.tanukisoftware.wrapper.WrapperSimpleApp` | `io.github.jayyanez.jss.bridge.SimpleApp` |
| `org.tanukisoftware.wrapper.WrapperStartStopApp` | `io.github.jayyanez.jss.bridge.StartStopApp` |
| `org.tanukisoftware.wrapper.WrapperJarApp` | `io.github.jayyanez.jss.bridge.JarApp` |
| `io.github.jayyanez.jss.bridge.SimpleApp`, `StartStopApp`, `JarApp` | unchanged |

The configuration text is never modified; only the launched main-class token
changes. When one of these launchers is selected, the effective classpath must
contain the bundled bridge (`wrapper.jar`, a class directory, or a `dir/*`
wildcard entry that contains it). Otherwise the supervisor fails before
launching Java with an error naming `wrapper.java.classpath`.

- **`SimpleApp`** takes the application main class as
  `wrapper.app.parameter.1` and the remaining parameters as application
  arguments. `main` runs on a non-daemon thread; when it returns and no other
  non-daemon threads remain, the JVM exits normally and `wrapper.on_exit.*`
  applies.
- **`StartStopApp`** takes
  `<startClass> <nStartArgs> <startArgs...> <stopClass> <waitForStopThreads> <nStopArgs> <stopArgs...>`
  as its parameters. On stop it invokes `stopClass.main(stopArgs)`; when
  `waitForStopThreads` is `true` it waits until no non-daemon application
  threads remain before the JVM exits.
- **`JarApp`** takes `<jarPath> <args...>`. It reads `Main-Class` from the JAR
  manifest, loads it through a `URLClassLoader` whose parent is the system
  class loader, and invokes `main(args)`.
- **Any other main class** is launched exactly as configured. Such a class must
  call `io.github.jayyanez.jss.bridge.Steward.start(listener, args)` with a
  `ServiceListener` implementation; otherwise it never connects to the control
  channel, the supervisor applies `wrapper.startup.timeout`, and the JVM is
  restarted. `Steward` also exposes `stop(int)`, `restart()`,
  `signalStarting(int)`, `log(int, String)`, `getProperties()`, `getLogFile()`,
  `getJvmId()`, `getWrapperPid()`, `getJavaPid()`, `isLaunchedAsService()`,
  `isManaged()` and `getVersion()`.

## Local control protocol

The supervisor and the bridge talk over an authenticated TCP connection on IPv4
loopback. The supervisor passes the port and a one-time key to the JVM as
system properties; the JVM connects, sends the key, and receives the minimum
log level, the log-file path, the configuration properties and the start
command. Runtime traffic covers start and stop completion, restart requests,
pings, log messages, service controls, pause/resume and garbage-collection
requests. Packets consist of a type byte, a UTF-8 payload and a NUL terminator.

This is the project's internal contract between `wrapper.exe` and
`wrapper.jar`. It may change between minor versions; always deploy the EXE and
the JAR of the same release together. Sensitive properties (names containing
`password`, `secret`, `token`, `key`, `credential` or `vault`) are not sent to
the JVM.

## Log format

The default log-file format is `LPTM`: a six-character level column, an
eight-character source column (`wrapper` or `jvm <n>`), a local
`YYYY/MM/DD HH:mm:ss` timestamp and the message, joined by literal ` | `
separators. Every record ends in CRLF. Supported format tokens are `L`
(level), `P` (source), `D` (thread), `Q` (blank), `T` (timestamp, seconds),
`Z` (timestamp, milliseconds), `U` (uptime), `G` (zero placeholder) and `M`
(message). Levels are `DEBUG`, `INFO`, `STATUS`, `WARN`, `ERROR`, `FATAL`,
`ADVICE`, `NOTICE` and `NONE`.

Roll modes: `NONE`, `SIZE` (rotate before a record once the active file has
reached `maxsize`), `WRAPPER` (rotate at supervisor start), `JVM` (rotate at
supervisor start and before every later JVM launch), `SIZE_OR_WRAPPER` and
`SIZE_OR_JVM`. Archives are `wrapper.log.1` (newest), `wrapper.log.2`, and so
on; `maxfiles=0` keeps an unlimited history.

Two markers are kept for monitoring compatibility:
`--> Wrapper Started as Service` / `--> Wrapper Started as Console` and
`<-- Wrapper Stopped`. The second line of every run is the product banner
(`Java Service Steward 64-bit 0.3.0`). All other supervisor messages use the
project's own wording.

## Windows service semantics

- `-i` stores `wrapper.exe`, the hidden `-s` command, the configuration path
  and every command-line override in the service `ImagePath`.
  `wrapper.ntservice.password` is passed to the SCM at install time only and is
  never persisted.
- With no explicit `wrapper.ntservice.account` the service runs as LocalSystem.
  A dedicated, least-privilege account is recommended.
- Services are not pausable by default. When `wrapper.pausable=true`, pausing
  stops the JVM (`stop_jvm=true` by default) and resuming launches the next JVM
  invocation.
- `-d` and `--heapdump` send an asynchronous control code; their exit code
  confirms delivery, not completion. Results are written to `wrapper.log`.
- If the service fails before the log file can be opened (for example a
  configuration error), the failure is recorded in the Windows Event Log so
  that the SCM error code is not the only diagnostic.

## Known differences and limits

- Windows x64 only. There is no Unix daemon mode.
- Only the loopback socket backend exists; named pipes are not implemented.
- Standard and Professional edition features (event commands, timers, JMX,
  remote control, and similar) are not implemented. `wrapper.license.*` is
  accepted and ignored.
- The original `wrapper.jar` and `wrapper.dll` are not supported; the bundled
  `wrapper.jar` must be deployed.
- The `org.tanukisoftware.*` Java API is not provided.
- `-Xrs` disables the `CTRL_BREAK` thread-dump path; the supervisor then uses
  `jcmd`, which requires a JDK-style runtime with `jcmd.exe` next to the
  configured `java.exe` or on `PATH`.
- Arbitrary `System.exit(n)` calls cannot expose `n` to the bridge reliably;
  use `Steward.stop(n)` when the exit-code policy matters.

---

Java Service Steward is an independent project; product names are used only
to describe compatibility. See the "Trademarks and affiliation" section of the
[README](../README.md#trademarks-and-affiliation).
