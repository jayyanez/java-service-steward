---
name: Bug report
about: Something does not behave as documented
title: ""
labels: bug
assignees: ""
---

## Summary

A clear, one-paragraph description of the problem.

## Steps to reproduce

1. Configuration: paste a minimal, sanitized `wrapper.conf` (remove passwords,
   tokens, host names, and internal paths).
2. Command or action: for example `wrapper.exe -c wrapper.conf`, a service
   start from the Service Control Manager, `wrapper.exe -d wrapper.conf`.
3. What happened.

## Expected behaviour

What you expected to happen instead.

## Log excerpt

Relevant lines from `wrapper.log`, ideally with `wrapper.debug=true`. Remove
secrets before pasting.

```text

```

## Environment

- Java Service Steward version (`wrapper.exe --version`):
- Windows version and edition:
- Java vendor and version (`java -version`):
- Console mode or service; service account if a service:

## Checklist

- [ ] I removed secrets, host names, and internal paths from everything above.
- [ ] I did not attach third-party binaries, real deployment files, or heap dumps.
- [ ] I am using the latest release (or I stated the commit I built from).
