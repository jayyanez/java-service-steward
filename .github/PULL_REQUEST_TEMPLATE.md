## What and why

Describe the change and the problem it solves. Link the issue it addresses
(`Fixes #123`).

## How it was tested

Which tests were added or changed, and which JDK versions you ran them with.

## Checklist

- [ ] `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-targets --all-features` pass locally with `JSS_REQUIRE_JAVA_TESTS=1` and a JDK on `PATH`.
- [ ] New behaviour is covered by tests; a bug fix includes a regression test.
- [ ] `src/help.txt`, `docs/`, and `examples/wrapper.conf.example` are updated for any new command or property.
- [ ] `CHANGELOG.md` has an entry under `Unreleased`; incompatible changes are marked **Breaking** with a migration note.
- [ ] Everything is written in English.
- [ ] No third-party source code, documentation text, binaries, or real deployment files were used or included (see the provenance rules in `CONTRIBUTING.md`).
- [ ] Every commit is signed off under the Developer Certificate of Origin (`git commit -s`).
- [ ] I agree that this contribution is licensed under `Apache-2.0 OR MIT`.
