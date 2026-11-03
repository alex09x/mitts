# GitHub Flow and releases

Use a working branch, a reviewed pull request and a green CI gate to update
`main`. Before releasing a new version, commit its version/lockfile and applicable
release documentation through a PR. The release workflow never invents a version,
bumps it silently, deploys the website or publishes a crate to crates.io.

## Checks

`CI` runs on PRs and `main`, and is reused by `Release` for its exact source
commit. It checks automation/installer controls, workflow syntax, Rust formatting,
Clippy and the existing full suite on Linux x86-64 and macOS ARM64. Tests run
through `run_tests_isolated.sh`; the per-target `probe_store_isolation.sh` is
mandatory too. Rust1.98.0 includes clippy, rustfmt and llvm-tools-preview. Network
access is required for existing registry fixtures. The stable aggregate check
is `gate`; every native job must succeed. No benchmark is part of CI.

Release reuses an already successful push-to-main CI only when both its source
and workflow-controller commit match exactly. If that CI is running, Release
waits instead of starting duplicate native tests. Failed/cancelled CI blocks
release. Without a matching proof, the reusable CI runs for the exact source;
an explicit fan-in gate requires one valid success path before packaging.

The isolation probe keeps Cargo's compilation HOME stable and gives each actual
test executable its own fresh HOME/XDG through a native Cargo runner. It still
runs every target and rejects every default-store writer. This avoids rebuilding
tests solely because the per-target HOME changed. A tiny real-Cargo fixture
checks compile-once behavior, distinct runtime homes and preserved Cargo runtime
environment on Linux and macOS; the probe needs Python3 as well as Cargo/Bash.
Mitts' own build metadata also watches the provenance inputs that actually
exist in Git checkouts or packaged sources. Git status is read-only, and current
branch/worktree metadata is tracked. Focused Cargo fixtures prove unchanged
builds stay fresh and changed Git/VCS identities invalidate the embedded SHA;
unknown provenance retains the conservative rebuild behavior.

The first CI adoption found historical rustfmt differences in the already
published0.3.18 source. `formatting-baseline.json` freezes only those exact file
hashes, checked against immutable commit8a6ae03. Unchanged legacy files may retain
that formatting; changed/new files must conform completely. Parse/tool errors
cannot be waived. The workflow never reformats a released tag or silently adds
new exceptions. This is a formatting-debt ratchet, not a claim that every legacy
file already conforms. Clippy and test failures remain unconditional failures.

## Release button

Open **Actions → Release → Run workflow**, select **main**, enter a stable
version matching Cargo.toml and Cargo.lock, then choose:

- **verify** (default): test the exact source and build/install all four native
  archives, and compile/verify the actual registry package. Write CI artifacts
  and checksums only. No remote tag, draft, upload
  or publication. Apple signing and notarization jobs are skipped and no Apple
  secrets are loaded. It verifies the current main candidate, even if that version
  already has a released tag. The archive's BUILD.txt records the candidate SHA;
  CI artifacts are not replacements for published release files. A temporary
  local tag inside the disposable build checkout permits packaging without
  pushing or changing the real tag. Draft/publish resolve an existing tag to its
  immutable source; a new version must already be committed on main.
- **draft**: after tests, all four native build/install checks and the Apple
  signing/notarization gates below pass, create an
  annotated immutable tag and a source-locked draft Release. Attach and verify
  Linux/macOS archives for ARM64/x86-64 and the
  complete SHA256SUMS. Keep the Release a draft, excluded from latest.
- **publish**: perform the same checks and preparation, verify every remote asset,
  then publish the complete draft as latest. Run the actual public installer on
  all four native targets and require its selected version/source to match this
  release. Publish the verified package to crates.io, then install the registry
  version and Git tag on all four targets and compile a fixture with each CLI.

Example verification of the existing release (no new release):

```sh
gh workflow run release.yml --ref main -f version=0.3.18 -f mode=verify
```

For a future release, use the version actually reviewed and merged into main.
Selecting `publish` is the operator's publication action. No release runs on an
ordinary PR/main push. GitHub's generated GITHUB_TOKEN supplies release access;
no personal token or workstation secret is copied into the repository.

## Apple release signing

An ordinary main push or PR runs CI only. Developer ID signing and Apple
notarization run exclusively in manually selected **draft** or **publish**
releases from this repository's protected main branch. The raw native compilation
jobs have no Apple credentials. Their ad-hoc Mac signatures are only intermediate
CI output and cannot pass the official release upload/publish validator.

The isolated signing job uses the protected **release-signing** Environment:
restrict deployments to the main branch, require owner review and disallow
administrator bypass. Set repository variable `APPLE_TEAM_ID` to the expected
Developer ID team. Configure these six Environment Secrets through managed
credential injection; never put their values in Git, logs or command arguments
used to provision GitHub:

- `APPLE_CERTIFICATE_P12_BASE64`: only the selected Developer ID Application
  identity, exported as an encrypted PKCS#12 (certificate plus private key).
- `APPLE_CERTIFICATE_PASSWORD`: its encryption password.
- `APPLE_SIGNING_IDENTITY`: the exact certificate SHA-1 fingerprint.
- `APPLE_NOTARY_KEY_P8`, `APPLE_NOTARY_KEY_ID`, `APPLE_NOTARY_ISSUER_ID`:
  the managed App Store Connect API credential authorized for notarization.

The signer imports the identity into a temporary Keychain, requires the expected
Developer ID team, enables hardened runtime and requests a secure timestamp.
Only the two signed Mac executables enter the submitted ZIP. The private
credential directory is separate, and cleanup runs before artifacts are uploaded.
The signer never executes the compiled release inputs while holding credentials.
Missing keys or a non-Accepted Apple result fail the release; there is no ad-hoc
fallback. Both architectures must match Apple's accepted ticket/CDHash.

Separate ARM64 and Intel jobs receive no signing secrets. They verify the actual
Developer ID signature/team, hardened runtime, timestamp and online notarization
ticket, then execute the signed CLI and a small isolated compilation. Only these
verified bytes enter the final archives and SHA256SUMS. BUILD.txt carries the
binary digest, team, CDHash and accepted notary receipt; receipt metadata alone
does not replace the native cryptographic/Apple checks. A bare CLI/ZIP cannot
carry a stapled offline ticket; this pipeline uses Apple's online assessment.

**Apple release credentials check** is a separate manual diagnostic. It imports
the identity and performs read-only Apple authentication, then deletes its
temporary Keychain. It does not sign binaries, submit notarization, create tags
or publish releases. It can validate credential setup without releasing a new
version. Previously published archives remain immutable and are not retroactively
notarized by adding this workflow.

## Failure and retry rules

If a test or native build fails, publication cannot run. A draft may remain;
latest continues to point to the previous stable release. Rerun **failed jobs**
so successful native artifacts are reused. Upload retries skip byte-identical
assets and reject conflicting hashes; there is no `--clobber`. Never move a tag,
replace published assets or silently adopt a manually created/unowned draft.
A published version cannot become the source of a new release with the same tag.
If publication succeeded but its readback failed, a failed-job retry verifies
the already-published source and every asset, makes no mutations and proceeds
to public installation checks. Native packaging runs before any permanent tag;
the tag inside each build runner is local-only until every target passes.

Read the workflow run and all native/public-install results before reporting
completion. The workflow creates no provenance attestation; BUILD.txt and
checksums record the tested source/compiler and download integrity.

## Registry publication and installation

The unprivileged package job runs `cargo package --locked`, compiling the actual
source distribution before any permanent tag or publication. It checks the
package version and embedded `.cargo_vcs_info.json` against the selected source
SHA, then records the exact archive checksum in a CI artifact.

After GitHub publication and all four public binary installs succeed, only
**publish** mode enters the protected **crates-io** Environment. That Environment
allows main only, requires owner review and disallows administrator bypass.
On crates.io, the trusted publisher is `alex09x/mitts`, workflow `release.yml`,
Environment `crates-io`. The pinned official `rust-lang/crates-io-auth-action`
exchanges GitHub OIDC for a short-lived upload token and revokes it after the job.
No long-lived Cargo token is stored in GitHub. `GITHUB_TOKEN` alone cannot publish
a crate. PR/main CI, verify and draft do not request a registry publishing token.

The uploader uses the exact source with `--locked --no-verify`, reusing the prior
package compilation. Public sparse-index and downloaded crate checks must match
the prepared checksum and source SHA. A retry of the same already published
package is read-only; conflicting bytes fail without replacement or yanking.
GitHub and crates.io cannot be published atomically: if registry publication
fails after GitHub succeeds, report that partial outcome and rerun failed jobs.

Registry, Git-tag and clone/path installations run in fresh prefixes on Linux/macOS,
ARM64/x86-64. Each installed CLI must report the exact version and source SHA,
execute help and compile a tiny project using its own isolated Mitts cache.
The registry installation exercises unpinned `cargo install mitts --locked` so
the default latest stable selection must resolve to this release too.
Source-built binaries are not claimed to have upstream Developer ID signatures;
Apple signing applies to the directly downloaded prebuilt Mac binaries.

The website is a separate deployment. Respect any owner preview hold. Workflow
configuration changes can ship independently without publishing held site
content. See [installation gates](release-installation.md) and the KYB
`release-playbook`, `release-installation-gates` and `mitts-release-runbook`.
