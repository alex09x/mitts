# Release binaries and installer verification

Mitts installs a ready binary from the latest stable GitHub Release:

```sh
curl -fsSL https://mittsbuild.com/install.sh | sh
```

The installer supports macOS (Apple Silicon, macOS 11+) and Linux
(ARM64 and x86-64, glibc 2.34+). Linux binaries are packaged in an Amazon Linux
2023 build environment, so they also run in Amazon Linux 2023 containers. The
installer needs curl, tar and either shasum or sha256sum. Rust and a native
linker are needed when building Rust projects; Cargo is not used to install the
ready binary. Windows and musl Linux are not provided as binary targets. Source
installation remains available through Cargo.

Downloads are retried on transient errors (5xx, timeouts). If github.com release
pages fail (for example a 504 while `api.github.com` is healthy), the installer
falls back to the GitHub API for the latest-tag lookup and for each asset, and the
SHA-256 check is applied exactly as before. Set `GITHUB_TOKEN` to raise the API
rate limit on shared CI runners; it is sent only to `api.github.com`.

In GitHub Actions use the setup action, which installs a release and keeps the
Mitts cache between runs: see [github-action.md](github-action.md).

Source-install routes (the release pipeline does not check them after publication):

```sh
cargo install mitts --version 0.3.20 --locked
cargo install --git https://github.com/alex09x/mitts --tag v0.3.20 mitts --locked
git clone --branch v0.3.20 https://github.com/alex09x/mitts
cargo install --path mitts --locked
```

These routes compile from source and require Rust/a native linker. Their binaries
are source-built; Developer ID signing and notarization apply to the prebuilt Mac
archives. No Homebrew formula or cargo-binstall channel is configured by this
release. Do not advertise those as verified installation methods.

The default destination is `$HOME/.cargo/bin/mitts`. `MITTS_INSTALL_ROOT`
overrides the installation prefix; otherwise `CARGO_INSTALL_ROOT` and
`CARGO_HOME` are respected. The installer does not read Cargo's install.root
configuration. To select a published version and an isolated destination:

```sh
curl -fsSL https://mittsbuild.com/install.sh -o /tmp/mitts-install.sh
MITTS_VERSION=v0.3.18 MITTS_INSTALL_ROOT=/tmp/mitts-smoke sh /tmp/mitts-install.sh
/tmp/mitts-smoke/bin/mitts --version
```

Installation resolves the stable tag once through GitHub's latest-release
redirect (without the unauthenticated API rate limit), downloads the matching target
archive and SHA256SUMS from that same tag, verifies the archive checksum,
extracts only mitts and checks its version from the staged destination file
before replacing the destination
atomically. A failed download, checksum or version check preserves the old
installation. Checksums detect corrupt/mismatched downloads; they are not a
claim of independent publisher authentication.

## Packaging and publication gates

1. Use an isolated worktree. Resolve the existing release tag to its commit;
   never move a published tag. Package from that exact source, not current main.
2. Run the Release binaries workflow for the tag. It builds native Linux and
   macOS Apple Silicon binaries, remaps build paths, checks the actual
   binary version/commit and scans for checkout/home paths. Each archive has
   mitts, both licenses and BUILD.txt with compiler/source/platform provenance.
3. For a NEW version, create a draft Release; keep it a draft until all three
   native archives, their checksums and SHA256SUMS are attached and verified.
   Publish only then. Drafts do not become latest; never publish an empty Release
   and add binaries later as a routine process. The existing 0.3.18 empty Release
   is repaired in place without moving its tag.
   Inspect all three job logs and archive checksums. Download the CI artifacts,
   assemble SHA256SUMS, and smoke-test the real archives on their native runners.
   Upload only missing assets to the existing Release. Do not use --clobber.
4. Release assets are immutable. Future versions get new filenames/tags. Record
   source SHA, workflow run/head, asset sizes, checksums and native smoke results.
5. Independently review the installer hotfix. If a site redesign is held for
   preview, base this hotfix on public main and deploy only the installer asset
   and its MIME headers. Do not push the unpublished site branch.
6. Inspect the actual Cloudflare Pages production check: commit, deployment ID
   and success. Fetch the PUBLIC installer. Require a shell shebang, text MIME,
   exact reviewed SHA256 and valid shell syntax. HTTP 200 alone is insufficient:
   unknown routes can return the homepage with status 200.
7. Run the Public installer smoke workflow after deployment. It executes the
   downloaded public script into an isolated prefix on all three native targets;
   checks version/commit, help and installed bytes against the released archive.
   Confirm
   homepage HTML is unchanged during an installer-only deployment. Record these
   receipts in the Mitts KYB release runbook before closing the incident.

Rollback an installer-only deployment with a normal revert of its reviewed
commit followed by a push and another verified Pages receipt. Never force-push,
move a tag, replace old assets or roll out an unrelated site to repair installation.
