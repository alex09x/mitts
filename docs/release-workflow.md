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
  archives. Write CI artifacts and checksums only. No remote tag, draft, upload
  or publication. It verifies the current main candidate, even if that version
  already has a released tag. The archive's BUILD.txt records the candidate SHA;
  CI artifacts are not replacements for published release files. A temporary
  local tag inside the disposable build checkout permits packaging without
  pushing or changing the real tag. Draft/publish resolve an existing tag to its
  immutable source; a new version must already be committed on main.
- **draft**: after tests and all four native build/install checks pass, create an
  annotated immutable tag and a source-locked draft Release. Attach and verify
  Linux/macOS archives for ARM64/x86-64 and the
  complete SHA256SUMS. Keep the Release a draft, excluded from latest.
- **publish**: perform the same checks and preparation, verify every remote asset,
  then publish the complete draft as latest. Run the actual public installer on
  all four native targets and require its selected version to match this release.

Example verification of the existing release (no new release):

```sh
gh workflow run release.yml --ref main -f version=0.3.18 -f mode=verify
```

For a future release, use the version actually reviewed and merged into main.
Selecting `publish` is the operator's publication action. No release runs on an
ordinary PR/main push. GitHub's generated GITHUB_TOKEN supplies release access;
no personal token or workstation secret is copied into the repository.

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
checksums record the tested source/compiler and download integrity. Publishing
to crates.io remains a separate credentialed operation from the exact tag;
GITHUB_TOKEN is not a crates.io credential. A later registry automation stage
requires its own managed authentication setup and readback.

The website is a separate deployment. Respect any owner preview hold. Workflow
configuration changes can ship independently without publishing held site
content. See [installation gates](release-installation.md) and the KYB
`release-playbook`, `release-installation-gates` and `mitts-release-runbook`.
