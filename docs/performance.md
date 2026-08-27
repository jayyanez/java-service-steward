# Resource baseline

This is a development baseline for the supervisor before any network telemetry
transport is enabled. Future telemetry work must repeat the same measurement
and report the delta.

## 2026-08-26 idle sample (0.2.x development build)

- Host: Windows 11 Pro 10.0.26200, Intel Core Ultra 9 386H, 16 logical
  processors, 31.5 GiB visible memory.
- Supervisor: optimized release build with thin LTO, 709,632-byte executable.
- JVM: Oracle GraalVM/HotSpot 25.0.4 running the project's own Java
  8-compatible `wrapper.jar`; no native library.
- Fixture: authenticated, started, no application output during the sample;
  console logging disabled and log-file logging enabled.
- Sampling: `System.Diagnostics.Process` values recorded after the readiness
  marker, followed by the change in `TotalProcessorTime` over five wall-clock
  seconds.

| Metric | Observed value |
| --- | ---: |
| Working set | 6.82 MiB |
| Private bytes | 1.41 MiB |
| Handles | 87 |
| Threads | 7 |
| CPU time over 5 idle seconds | 0 ms |

The application remained idle for the full sample and then requested an
orderly stop through the bridge API. The supervisor returned exit code 0 and
left no Java process. These values are a single local sample, not a
cross-machine guarantee or a formal benchmark distribution.

The bridge uses a one-second socket read timeout for its parent-ping watchdog
rather than allocating a separate watchdog thread.

## Nonblocking-socket note

During an earlier measurement an accepted TCP stream was found to inherit the
listener's nonblocking state on Windows, causing one supervisor thread to spin.
The runtime connection now explicitly switches to blocking mode with a bounded
read timeout, and an automated regression test verifies that an idle receive
waits instead of busy-looping.
