# Contributing to Java Service Steward

Thank you for considering a contribution. This document explains how to
propose changes, what is checked before a merge, and the provenance rules that
keep the project safe to use under a permissive license.

## Ground rules

- **English only.** Code, comments, commit messages, documentation, issues,
  and pull requests are written in English.
- **Keep the project independent.** The provenance rules below are not
  negotiable; they protect every user of the project.
- **Prefer small, focused pull requests.** Open an issue first for anything
  that changes documented behaviour, adds a configuration property, or touches
  the EXE-to-JAR protocol.
- Follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Governance

The repository has a single maintainer. Every change reaches `main` through a
pull request that must pass the CI checks and receive the maintainer's
approving review; nobody else can merge, and the branch ruleset blocks direct
pushes and force pushes. Contributions are welcome, and the maintainer decides
what is merged.

What protects the project's license and its users:

- Every commit is signed off under the Developer Certificate of Origin (see
  below). By signing off you certify that you wrote the change or have the
  right to submit it under this project's license. CI rejects unsigned
  commits.
- Contributions are licensed `Apache-2.0 OR MIT`. Under the Apache-2.0 terms
  a contribution carries a patent license from its contributor for that
  contribution, and the license terminates for anyone who brings a patent
  claim over the work.
- CI checks the license headers of every source file, refuses binaries and
  configuration files, rejects copyleft license text and third-party product
  names in source files, and (`cargo-deny`) rejects dependencies whose license
  is not on the allow list.
- The provenance rules below forbid consulting or copying code, documentation
  text or artifacts of other products. A pull request that cannot state where
  its interface facts came from is not merged.

These mechanisms make the origin of every line traceable to a person who
certified it; they cannot prove the absence of copying, so the maintainer's
review remains the last line of defense.

## Development setup

You need:

- Rust 1.88 or newer. `rust-toolchain.toml` selects the stable channel and
  installs `rustfmt` and `clippy` automatically.
- A JDK 8 or newer on `PATH` (Temurin works well). The bridge JAR is compiled
  with `--release 8`; the integration tests also look for `jar`, `jcmd`, and
  `jlink` next to that `javac`.
- Windows 10, Windows 11, or Windows Server. The service integration and the
  integration tests are Windows-only.

Build and check everything:

```powershell
cargo build --release
./scripts/build-java-bridge.ps1
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

`./scripts/build-release.ps1` runs all of the above plus the version
consistency checks in one command.

### JVM tests

The tests under `tests/` compile small synthetic Java programs and run real
JVMs through `wrapper.exe`. When no JDK is found they print a "skipping"
message and pass. Set `JSS_REQUIRE_JAVA_TESTS=1` to turn those skips into
failures. CI sets it, so a pull request cannot go green with vacuous tests;
run the same way locally before pushing:

```powershell
$env:JSS_REQUIRE_JAVA_TESTS = "1"
cargo test --all-targets --all-features
```

Some tests also read `JSS_REFERENCE_DIR`, an optional directory of your own
`*.conf` and `*.log` files kept outside the repository, to check that they
parse without warnings and that the log format is preserved. They skip when it
is unset. Never commit such files.

If you move or rename the repository directory, run `cargo clean` before
testing: the test binaries bake the manifest directory into the executable at
compile time.

## Provenance rules

Java Service Steward reads the `wrapper.conf` format and follows the
command-line conventions of the Java Service Wrapper, but it is an independent
implementation and it must stay that way. When working on this project:

1. **Never consult the source code, javadoc, or documentation text of the
   Java Service Wrapper**, of any mirror or fork of it, or of any other
   implementation whose license is incompatible with `Apache-2.0 OR MIT`, and
   never copy code, comments, text, message catalogs, or resource files from
   them. Interface facts come only from this repository's own documentation
   under `docs/`, from the behaviour of the project's own code and tests, and
   from black-box observation of externally visible behaviour (command lines,
   files, logs, sockets). If you need a fact that those sources do not give
   you, open an issue and ask; do not go and look.
2. **Never upload third-party binaries** (an original `wrapper.exe`,
   `wrapper.jar`, or native DLL), real deployment configurations, logs, or heap
   dumps to the repository, to an issue, or to a pull request. Use the
   synthetic fixtures under `tests/` and `examples/`, and sanitize anything
   you paste.
3. **Never use material from the Standard or Professional editions** of the
   original product. Their license terms are incompatible with this work.
4. **Use the vendor's names nominatively only.** The names of the original
   product and of its vendor may appear in documentation to describe
   compatibility (see the notice at the end of [README.md](README.md)). They
   never appear in banners, `--version` output, log messages, code
   identifiers, file names, or the product name. Do not cite a version number
   of the original product; describe compatibility as "the Community Edition
   feature set" or property by property.
5. **Write messages and documentation in the project's own voice.** Only the
   log markers `--> Wrapper Started as Service`, `--> Wrapper Started as
   Console`, and `<-- Wrapper Stopped` are kept deliberately for monitoring
   compatibility; everything else is our own wording.

The full statement is in [docs/provenance.md](docs/provenance.md). A pull
request that appears to break these rules is closed, and the affected commits
are removed from history.

## Developer Certificate of Origin

Every commit must be signed off. The sign-off certifies that you wrote the
change or otherwise have the right to submit it under the project's license,
as defined by the Developer Certificate of Origin 1.1
(<https://developercertificate.org/>):

```text
Developer Certificate of Origin
Version 1.1

Copyright (C) 2004, 2006 The Linux Foundation and its contributors.

Everyone is permitted to copy and distribute verbatim copies of this
license document, but changing it is not allowed.


Developer's Certificate of Origin 1.1

By making a contribution to this project, I certify that:

(a) The contribution was created in whole or in part by me and I
    have the right to submit it under the open source license
    indicated in the file; or

(b) The contribution is based upon previous work that, to the best
    of my knowledge, is covered under an appropriate open source
    license and I have the right under that license to submit that
    work with modifications, whether created in whole or in part
    by me, under the same open source license (unless I am
    permitted to submit under a different license), as indicated
    in the file; or

(c) The contribution was provided directly to me by some other
    person who certified (a), (b) or (c) and I have not modified
    it.

(d) I understand and agree that this project and the contribution
    are public and that a record of the contribution (including all
    personal information I submit with it, including my sign-off) is
    maintained indefinitely and may be redistributed consistent with
    this project or the open source license(s) involved.
```

Add the sign-off with `git commit -s`, which appends
`Signed-off-by: Your Name <you@example.com>` from your Git identity. Use a
real name and a working e-mail address. Pull requests containing unsigned
commits are not merged.

## Licensing of contributions

The project is licensed under `Apache-2.0 OR MIT`. By submitting a
contribution you agree that it is licensed under those same terms, without
any additional terms or conditions, as stated in section 5 of the Apache-2.0
license and in the [License](README.md#license) section of the README. There
is no contributor license agreement to sign.

New source files start with the SPDX header
`SPDX-License-Identifier: Apache-2.0 OR MIT` (`//` in Rust and Java, `#` in
PowerShell).

## Commit messages

- Subject line in the imperative mood, at most 72 characters, no trailing
  period: `Add jss.heapdump.timeout`, `Fix packet framing after a read timeout`.
- An optional area prefix helps: `config:`, `supervisor:`, `protocol:`,
  `bridge:`, `docs:`, `ci:`.
- Leave a blank line, then explain *why* the change is needed and any change
  in behaviour, wrapped at 72 columns. Reference issues with `Fixes #123`.
- One logical change per commit; keep formatting-only changes separate.
- End with the `Signed-off-by:` line.

## Pull request checklist

Before opening a pull request, confirm that:

- [ ] `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-targets --all-features` pass with `JSS_REQUIRE_JAVA_TESTS=1` and a JDK on `PATH`.
- [ ] New behaviour has tests, and a bug fix has a regression test.
- [ ] `src/help.txt`, the relevant page under `docs/`, and `examples/wrapper.conf.example` are updated for any new command or property.
- [ ] `CHANGELOG.md` has an entry under `Unreleased` (Keep a Changelog style; incompatible changes are marked **Breaking** and come with a migration note).
- [ ] Everything is in English and follows the provenance rules above.
- [ ] No binaries, real configurations, logs, heap dumps, or secrets are included.
- [ ] Every commit is signed off (`git commit -s`).

Reviews focus on correctness, on keeping the supervisor loop non-blocking and
every buffer bounded, and on leaving the `wrapper.conf` contract intact.
Expect questions; they are not a rejection.
