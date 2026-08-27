# Release procedure

Audience: maintainers. The version numbering rules (PATCH, MINOR, MAJOR,
pre-release suffixes) are in [versioning.md](versioning.md); this page is the
step-by-step procedure that turns a commit into a published release.

## Before you start

- The working tree is clean and on the branch that will be released.
- CI is green for the commit you intend to release.
- `cargo install cargo-about cargo-deny` has been run at least once if
  dependencies changed since the previous release.
- You have a Temurin 8, 21, and 25 JDK available locally, or you rely on CI
  for the Java matrix.

## Steps

1. **Classify the changes** since the previous release as PATCH, MINOR, or
   MAJOR. During the 0.x series an incompatible change is MINOR and must be
   marked **Breaking** in the changelog together with a migration note.

2. **Bump the version in `Cargo.toml`** (`version = "X.Y.Z"`) and refresh
   `Cargo.lock` with `cargo update --workspace` (any `cargo build` also does
   it). This is the only place the version is written by hand: `--version`,
   `--help`, the log banner, and the `Implementation-Version` of `wrapper.jar`
   all derive from it.

3. **Close the changelog section.** In `CHANGELOG.md`, rename
   `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` and add a new, empty
   `## [Unreleased]` above it. `scripts/verify-version.ps1` fails if the
   section for the Cargo version is missing.

4. **Refresh the license material** when dependencies changed:

   ```powershell
   cargo deny check
   cargo about generate about.hbs -o THIRD_PARTY_NOTICES.md
   ```

   Review the diff of `THIRD_PARTY_NOTICES.md`; every crate that ships in
   `wrapper.exe` must appear under its license.

5. **Run the full local build:**

   ```powershell
   ./scripts/build-release.ps1
   ```

   It validates the version, runs `cargo fmt --check`, `cargo clippy` with
   `-D warnings`, `cargo test`, builds `target\release\wrapper.exe` and
   `target\release\wrapper.jar`, and verifies both artifacts, printing their
   SHA-256 hashes.

6. **Run the Java matrix.** With `JSS_REQUIRE_JAVA_TESTS=1` set, run
   `cargo test --all-targets --all-features` once with each of Temurin 8, 21,
   and 25 first on `PATH`, or wait for the CI matrix of the release commit.

7. **Commit and push** with a message such as `Release X.Y.Z` (signed off,
   like every commit). Wait for CI to finish green.

8. **Tag and push the tag:**

   ```powershell
   git tag -a vX.Y.Z -m "Java Service Steward X.Y.Z"
   git push origin vX.Y.Z
   ```

   The tag must be exactly `v` followed by the Cargo version; the release
   workflow refuses anything else.

9. **Let the workflow publish.** `.github/workflows/release.yml` builds the
   executable with `--locked`, builds the JAR, runs the version verification,
   stages `java-service-steward-X.Y.Z-windows-x64/` with `wrapper.exe`,
   `wrapper.jar`, `LICENSE-APACHE`, `LICENSE-MIT`, `NOTICE`,
   `THIRD_PARTY_NOTICES.md`, `README.md`, and
   `examples/wrapper.conf.example`, zips it, writes `SHA256SUMS`, and creates
   the GitHub Release with generated release notes, attaching the zip and
   `SHA256SUMS`.

10. **Verify the published release.** Download the zip and `SHA256SUMS` from
    the release page and compare:

    ```powershell
    (Get-FileHash .\java-service-steward-X.Y.Z-windows-x64.zip -Algorithm SHA256).Hash.ToLower()
    Get-Content .\SHA256SUMS
    ```

    Unpack the zip, run `wrapper.exe --version`, and confirm that the first
    line is `Java Service Steward 64-bit X.Y.Z`. Then edit the release notes:
    keep the generated list of pull requests if it is useful, and add a link
    to the `CHANGELOG.md` section for the version.

## Rules

- **Never reuse a version number.** If anything in the artifacts must change
  after the tag was pushed, even by one byte, release the next PATCH version.
  Do not replace files on an existing release.
- Delete a tag and its release only when the workflow failed before publishing
  anything; fix the cause, then push the same tag again.
- The JAR packager sorts its entries and uses a fixed timestamp, so two builds
  from the same sources and toolchain must produce the same `wrapper.jar`
  hash. Treat a difference as a release process defect and investigate before
  publishing.
- Every release zip contains `LICENSE-APACHE`, `LICENSE-MIT`, `NOTICE`, and
  `THIRD_PARTY_NOTICES.md`; the workflow fails if any of them is missing.
- Pre-releases (`X.Y.Z-rc.1`) follow the same procedure. Mark the GitHub
  Release as a pre-release by hand after the workflow has created it.

## Future improvements

- **Authenticode code signing.** `wrapper.exe` is currently unsigned, so
  Windows SmartScreen may warn on first launch and some organizations cannot
  deploy it. [SignPath Foundation](https://signpath.org/) provides free code
  signing for open-source projects through a GitHub Actions integration; the
  release workflow would upload the built executable, receive the signed
  file, and package that instead. Signing `wrapper.jar` with `jarsigner`
  could be added at the same time.
- **Sigstore.** Keyless signing of the zip and `SHA256SUMS` with `cosign`
  from the workflow's OIDC identity, so that users can run
  `cosign verify-blob` against the published `.sig` and certificate without
  any key management on the project side. GitHub's
  `actions/attest-build-provenance` offers a similar, GitHub-hosted
  provenance attestation.
- **SBOM.** Attach a CycloneDX bill of materials generated with
  `cargo cyclonedx` to each release.
