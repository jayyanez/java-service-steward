# Security policy

## Supported versions

Java Service Steward is pre-1.0. Security fixes are released for the latest
0.x version only; earlier releases are not patched. Please upgrade to the
newest release and confirm that the problem still reproduces there before
reporting it.

| Version | Supported |
| --- | --- |
| Latest 0.x release | Yes |
| Earlier releases | No |

## Reporting a vulnerability

Please do not open a public issue for a security problem.

1. Use GitHub's private vulnerability reporting for this repository:
   <https://github.com/jayyanez/java-service-steward/security/advisories/new>.
2. If private reporting is not available to you, open a regular issue that
   says only that you need a private channel for a security report, with no
   details, and a maintainer will get in touch.

Include the version (`wrapper.exe --version`), the impact as you understand
it, and steps to reproduce. A minimal `wrapper.conf` helps, with every secret,
host name, and internal path removed.

You will receive an acknowledgement within 7 days. After that we keep you
informed about the assessment, the fix, and the release. Please allow a
reasonable time for a fix to be published before disclosing details publicly;
we will credit you in the release notes unless you prefer otherwise.

## Threat model

This section describes what the design protects against and what it
deliberately leaves to the operating system and the operator.

### Control channel between `wrapper.exe` and the JVM

The wrapper and the JVM talk over a TCP socket bound to the IPv4 loopback
address only. It is never reachable from the network, and the wrapper opens
no other listener.

Each JVM launch gets a fresh random 256-bit key. The wrapper hands the key and
the port to the JVM as system properties on the Java command line
(`-Dwrapper.key=...`, `-Dwrapper.port=...`). Consequently any local
administrator, and any process running under the service account, can read
the key from the JVM's command line and talk to either side of the channel.
This is the same boundary as in the configuration format's original design and
it is accepted on purpose: the key prevents accidental cross-talk between
services on the same machine and rejects stray local connections; it is not a
defence against a hostile local administrator, who already controls the
service through the Service Control Manager.

The key and every property whose name looks sensitive (it contains
`password`, `secret`, `token`, `key`, `credential`, or `vault`) are redacted
when the Java command line is written to the log.

### Service account

The service is registered as `LocalSystem` unless `wrapper.ntservice.account`
names another account. `LocalSystem` has complete control of the machine, so
any vulnerability in the hosted Java application becomes a full compromise.

Run production services under a dedicated account with only the rights it
needs (a local user, a virtual service account, or a group managed service
account) by setting `wrapper.ntservice.account` before `wrapper.exe -i`. The
install password (`wrapper.ntservice.password`) is used only during
installation and is never written to the service's `ImagePath` or to the log.
Prefer passing it on the install command line
(`wrapper.exe -i wrapper.conf wrapper.ntservice.password=...`) over keeping
it in the file.

### The configuration file

`wrapper.conf` frequently carries secrets in JVM arguments: keystore
passwords, database credentials, API tokens. The wrapper reads the file as-is
and does not encrypt it. Restrict its NTFS permissions to the service account
and administrators, keep it out of version control (this repository's
`.gitignore` already excludes `wrapper.conf`), and, where the application
allows it, prefer a secret store or environment variables over command-line
arguments, which are visible to every local user that can list processes.

Configuration properties are forwarded to the JVM over the control channel so
that the application can read them; that channel is subject to the boundary
described above.

### Heap dumps

An HPROF file produced by `wrapper.exe --heapdump` (or by HotSpot's
`-XX:+HeapDumpOnOutOfMemoryError`) contains the entire live heap in clear
text: passwords, tokens, session data, personal data, anything the
application had in memory. Treat it as a production secret. Point
`jss.heapdump.directory` at a directory that only administrators can read,
transfer dumps over an encrypted channel, and delete them after analysis. The
wrapper never rotates or deletes heap dumps.

Heap and thread dumps can be requested by anybody who is allowed to send user
control codes to the service, which by default means administrators.

### Log files

`wrapper.log` contains everything the application prints to stdout and
stderr, which may include stack traces, request payloads, or secrets the
application chose to print. Protect the log directory like the configuration
file, and scrub logs before attaching them to bug reports.

### Out of scope

- Attacks by a local administrator or by code already running under the
  service account.
- Vulnerabilities in the hosted Java application or in the JVM itself.
- Network attacks: the wrapper has no network-facing surface.

## Hardening checklist

- [ ] Run under a dedicated service account (`wrapper.ntservice.account`), not `LocalSystem`.
- [ ] Restrict NTFS permissions on the installation directory, `wrapper.conf`, the log directory, and the heap-dump directory to the service account and administrators.
- [ ] Keep `wrapper.conf` out of version control and out of unencrypted backups.
- [ ] Pass the install password on the command line rather than storing `wrapper.ntservice.password` in the file.
- [ ] Give the application its secrets through a mechanism it can protect, not through `wrapper.java.additional.<n>` where they end up on the command line.
- [ ] Set `jss.heapdump.directory` to a protected location and delete dumps after analysis.
- [ ] Leave `wrapper.ntservice.interactive` unset (the service does not interact with the desktop).
- [ ] Keep the JVM's attach mechanism (`jcmd`) available to the service account only if you need thread or heap dumps; `-XX:+DisableAttachMechanism` turns both off.
- [ ] Verify downloaded releases against the published `SHA256SUMS`.
- [ ] Watch the repository's releases and upgrade promptly when a security fix is published.
