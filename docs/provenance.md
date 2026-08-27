# Provenance

## Statement

> Java Service Steward was written from scratch in Rust and Java. Its
> compatibility with the Java Service Wrapper configuration format, command-line
> conventions, log format and local control protocol was achieved by
> (1) reading the publicly available product documentation for property and
> command semantics, (2) observing the externally visible behavior of a
> GPL-licensed Community Edition build (command lines, log files, PID files and
> loopback socket traffic), and (3) confirming a small set of interface facts
> (packet type codes, system property names and the packet framing) that are
> functional interface elements rather than creative expression. No source code,
> documentation text or resource files from Tanuki Software have been copied
> into this project, and the project deliberately does not reproduce the
> `org.tanukisoftware.wrapper` Java API. Contributors must not consult Tanuki
> Software source code while working on this project.

## Rules for contributors

- Never open, download, or consult the Java Service Wrapper's source code, javadoc, or
  decompiled binaries, including mirrors and forks of the Community Edition.
- Never copy sentences from the Java Service Wrapper documentation into help text,
  messages, comments, or documentation. Describe behavior in your own words.
- The only permitted sources for interoperability facts are the public product
  documentation (property and command semantics), this repository's tests, and
  black-box observation of externally visible behavior (command lines, log
  files, PID files, loopback socket traffic).
- Never add artifacts of the Java Service Wrapper (executables, JARs, DLLs) or real
  deployment configurations to the repository.
- Use the names "Java Service Wrapper" and "Tanuki Software" only
  nominatively, in documentation, to describe compatibility. Never cite a Java
  Service Wrapper version number.
- Everything in the repository is written in English.

## Review log

- 2026-08-26: structural-similarity review of src/ and java/ found no comments,
  identifiers or file references derived from the original product's source; the log-message
  catalog was reworded in the project's own voice (see CHANGELOG 0.3.0).
- 2026-08-26: the Java bridge (`java/bridge`) was written from the project's
  own implementation specification. Its classes, method names, threading
  model and messages are the project's design; it shares with the original
  product only the functional interface facts needed to interoperate (packet
  type numbers, the NUL-terminated packet framing, and the names of the system
  properties the executable passes to the JVM). The executable contains no
  third-party class names; launcher substitution is a generic name rule.
