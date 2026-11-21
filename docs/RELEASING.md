# Releasing GPUI Kit Block Editor

Repository: https://github.com/nrf110/gpui-kit-block-editor

## Current state

No release tag or publication is created by installing these workflows.

- `CI` runs for pull requests, merge queues, and manual dispatch. `Release` calls the same checks for each push to `main`, including when release credentials are absent.
- The `macOS checks` job uses `macos-15` with its preinstalled Xcode and the toolchain/components in `rust-toolchain.toml`. It checks formatting, runs native library tests, compiles the demo, and runs `cargo publish --locked --dry-run` with package build verification enabled. This is macOS coverage, not a Linux/Windows support claim.
- Release PR preparation is skipped until the GitHub App variable and secret below are configured. It then manages the version and `CHANGELOG.md`; there is no auto-merge.
- Publication is disabled by both the absent/false `RELEASE_PUBLISH_ENABLED` repository variable and `publish = false` in `release-plz.toml`. Tags and GitHub Releases are independently disabled in that file.
- Workflow actions are pinned to commit SHAs; the release-plz executable is pinned to `0.4.3`. Review pin/version updates together.

## Configure GitHub once

1. Push the repository and workflows to `nrf110/gpui-kit-block-editor`, with `main` as the default branch. This local checkout originally had no remote; adding a remote and pushing is a separate operation.
2. Enable GitHub Actions. Protect `main`: require a pull request, human review, and the `macOS checks` status from the PR's `CI` workflow. Require the branch to be up to date, or use the merge queue. Do not exempt the release bot from review/checks.
3. Create a GitHub App with repository **Contents: read/write** and **Pull requests: read/write** permissions. Disable webhooks and install it on this repository only. No administration or Actions/workflow write permission is needed for ordinary release PRs.
4. Add repository Actions variable `RELEASE_PLZ_APP_ID` containing its App ID, and Actions secret `RELEASE_PLZ_APP_PRIVATE_KEY` containing its PEM private key. The token action requests only Contents and Pull requests write access, scopes the installation token to this repository, and revokes it after the job.
5. Allow release PR preparation to run on the next `main` push, or dispatch `Release` on `main`. The App token is intentional: PRs created with the default `GITHUB_TOKEN` do not trigger ordinary PR CI. Do not replace it with a default-token fallback.

Leave `RELEASE_PUBLISH_ENABLED` unset or `false` throughout bootstrap. Release preparation needs no crates.io token.

## First publication (manual)

Trusted publishing cannot create a new crate. Before publishing, confirm that the crates.io name is available or controlled by the intended maintainers, and review the public API, license/NOTICE, dependency compatibility, and generated changelog.

1. Review the initial release PR. Choose the initial Cargo version in that PR; no initial version/tag has been selected by this setup. The tag convention is `v` followed by the Cargo version. Merge only after required CI succeeds.
2. From a clean checkout of that exact reviewed merge commit, run:

   ```sh
   cargo fmt --all -- --check
   cargo test --locked --lib
   cargo check --locked --example demo
   cargo publish --locked --dry-run
   ```

   Unlike local development checks on an intentionally dirty tree, the real release must not use `--allow-dirty` or `--no-verify`.
3. Authenticate locally with crates.io using a securely stored, appropriately scoped token, then explicitly run `cargo publish --locked`. Do not store the bootstrap token in this repository or its Actions secrets. The workflow does not run this first publication for you.
4. Confirm the published version, then create its matching `vVERSION` tag and GitHub Release targeting that same reviewed commit. Use the release PR's changelog as the release notes. Do not tag a newer `main` commit accidentally.

## Enable automated publication after bootstrap

1. Create the GitHub Actions environment named `release`. Limit deployments to `main`; require an approving maintainer and prevent self-review where the repository's GitHub plan supports those controls. Configure these protections before enabling publishing.
2. On the existing crates.io crate, configure a GitHub trusted publisher with:
   - Owner: `nrf110`
   - Repository: `gpui-kit-block-editor`
   - Workflow filename: `release.yml`
   - Environment: `release`
3. In a reviewed PR, change these three `[workspace]` settings in `release-plz.toml` to `true`:

   ```toml
   publish = true
   git_tag_enable = true
   git_release_enable = true
   ```

   Keep `release_always = false`, package build verification enabled, and dirty publication disabled. Do not add `CARGO_REGISTRY_TOKEN`: release-plz exchanges the publish job's OIDC identity for a short-lived crates.io credential itself.
4. After that PR passes CI and is merged, set repository Actions variable `RELEASE_PUBLISH_ENABLED` to `true`.
5. Review and merge the next release PR. Its exact commit passes the reusable checks before the release job runs. Only a commit associated with a merged `release-plz-` PR targeting `main` can enter the protected publishing environment; release-plz independently applies its `release_always = false` gate. Approve that deployment to publish, tag, and create the GitHub Release.

Ordinary `main` commits can update the next release PR but do not publish. Forks/non-main manual runs cannot prepare or publish releases for this repository. PR CI has read-only repository access and no release credentials. The publish job alone has `id-token: write` and repository Contents write permission; the App installation token is restricted to release PR preparation.

## Routine releases and recovery

- Use conventional commit subjects (`feat:`, `fix:`, `refactor:`, and `!`/`BREAKING CHANGE` where appropriate). Review release-plz's proposed version and changelog rather than treating inferred semver as approval. Semver checking is enabled when a published baseline exists.
- Review changes to public APIs separately from changes to the document-format marker. A crate version bump does not automatically migrate document schemas.
- Keep the release PR's `release-plz-` branch prefix: both the workflow and release-plz use it to identify release candidates.
- If a release fails, inspect the crates.io version and GitHub tag/release before retrying. Rerun the failed workflow for the same reviewed SHA, not a later unrelated commit; never attempt to overwrite a published crate version or move an existing release tag.
- To stop automatic publication, unset `RELEASE_PUBLISH_ENABLED` or set it to `false`. Also restore the three configuration flags to `false` if returning to bootstrap mode. Already-running jobs need to be canceled separately.

Remote App installation, secrets/variables, branch protection, environment protection, and trusted-publisher registration are administrator setup, not changes these workflow files apply themselves.

References: [release-plz quickstart](https://release-plz.dev/docs/github/quickstart), [release-only-after-PR configuration](https://release-plz.dev/docs/config#the-release_always-field), [GitHub token behavior](https://release-plz.dev/docs/github/token), and [crates.io trusted publishing](https://crates.io/docs/trusted-publishing).
