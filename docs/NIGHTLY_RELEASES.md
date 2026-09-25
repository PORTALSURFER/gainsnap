# Nightly releases

## Local macOS nightly (current path)

`scripts/local_nightly.py` runs the VST3 and screenshot gates on this Mac,
builds both arm64 bundles, signs them with an installed Developer ID Application
identity, submits both to Apple for notarization, staples and audits them, then
publishes a macOS-only schema-2 nightly directly to PortalSurfer. It does not
dispatch GitHub Actions. The landing page reads the public release catalog, so
the verified release appears at `/plugins/gainsnap/` without a site rebuild.
The Windows VST3 is absent from local nightlies.

Set up local credentials once:

1. Select the intended Developer ID Application fingerprint from
   `security find-identity -v -p codesigning` and set
   `APPLE_CODESIGN_IDENTITY` to that 40-character fingerprint.
2. Run `python3 scripts/local_notary_setup.py` to preview the local Apple Team API Key,
   then rerun with `--execute` in a terminal to enter its issuer ID and store
   the validated `gainsnap-local` profile in Keychain. Alternatively,
   set `APPLE_NOTARY_KEY_PATH`, `APPLE_NOTARY_KEY_ID`, and
   `APPLE_NOTARY_ISSUER_ID` for direct local key use. The key and issuer are
   never committed to this repository.
3. Run `python3 scripts/local_publisher_token.py` to preview the one-time
   publisher rotation, then run it with `--execute`. It sends only a SHA-256
   hash to the existing server admin wrapper, verifies upload authentication,
   and stores the new token in macOS Keychain. Rotation retires the old GitHub
   Actions upload secret; the local token never appears in argv or a plaintext file.
4. Set `VST3_SDK_DIR` to the pinned VST3 SDK checkout containing
   `pluginterfaces/`.

From a clean `main` exactly matching `origin/main`:

```bash
python3 scripts/local_nightly.py            # read-only release plan
python3 scripts/local_nightly.py --package  # local CI, signing and notarization
python3 scripts/local_nightly.py --publish  # above, then direct upload and live verification
```

If the signed package was created but upload failed, retry without rebuilding:

```bash
python3 scripts/local_nightly.py --publish-existing BUILD_ID
```

The planner skips a source commit already published as a nightly; `--force`
creates the next suffix when a deliberate repeat is needed. It refuses a dirty
checkout, a non-main checkout, or a local main that differs from `origin/main`.
Each local release keeps its own immutable build ID and an audited manifest in
`dist/releases/`.

## Hosted GitHub Actions path (legacy, manual dispatch only)

Automatic push, pull-request, and scheduled workflow triggers are disabled to
avoid hosted CI costs. The workflows remain available for explicit dispatch.

Use the `GainSnap nightly scheduler` workflow (`nightly.yml`) to prepare and publish a new nightly:

```bash
gh workflow run nightly.yml --repo PORTALSURFER/gainsnap --ref main
```

The coordinator skips a source commit already published as a nightly. Use `-f force=true` to request a new nightly even without source changes.

Each new nightly advances the package patch version in both `Cargo.toml` and `Cargo.lock`. For example, a prepared release is `0.1.1-nightly.<run-number>`. The workflow sequence distinguishes attempts; it does not replace the package patch increment. If a version was prepared but publication failed, retrying reuses that unpublished package version.

## Protected preparation

The coordinator creates or reuses a version-only PR, explicitly dispatches CI and release preflight for its exact head, and merges only after the checks pass. It then explicitly dispatches the protected preflight on merged `main` and waits for success before requesting the production release. If `main` moves during validation, the run stops instead of publishing a different commit.

Explicit dispatch is necessary because changes made with `GITHUB_TOKEN` do not trigger the usual push workflows. The repository allows Actions to create PRs, while its default workflow token remains read-only. Only the nightly coordinator receives repository contents, pull-request, and Actions write permissions. Branch protection is retained.

GitHub may hold workflows created for the bot's version PR for maintainer approval. Explicit branch dispatches provide exact-head validation but do not approve those PR workflows or satisfy the protected merge gate by themselves. Approve the bot-created PR workflows in Actions when requested; the coordinator waits for that gate and never approves it automatically.

Existing protected environment approvals still apply. Approve the publisher-integration and production environments when requested by GitHub. Neither the coordinator nor its token approves those environments automatically.

## Retries and direct releases

Run the scheduler again after an interrupted attempt. An already prepared, unpublished patch is reused, and completed validation can be reused only for the exact source commit. A failed or incomplete check blocks publication.

Direct production nightly dispatch through `release.yml` requires an already prepared package version newer than public release history. A new run cannot publish another nightly suffix using an already published patch. An exact retry of a published build is a no-op. Stable and RC release behavior is unchanged.

All platform bundles use the same package version, source SHA, publication version, and build identity. macOS signing/notarization, Windows artifact validation, and the approved publisher-preflight gate remain part of production publication.
