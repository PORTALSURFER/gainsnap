# GainSnap Windows nightly release

GainSnap publishes one public production manifest for each nightly release. It
contains exactly these three artifacts:

- macOS arm64 CLAP, Developer ID signed, notarized, and stapled
- macOS arm64 VST3, Developer ID signed, notarized, and stapled
- Windows x86_64 VST3, unsigned

The Windows archive is always named
`gainsnap-v<publication-version>-windows-x86_64-unsigned.vst3.zip`. It contains
one VST3 bundle member and is accompanied by `windows-artifact-manifest.json`,
which records the source SHA, package/publication versions, build ID,
`released_at`, pinned dependency revisions, runner/toolchain provenance, and
the explicit unsigned status.

## Workflow boundary

`.github/workflows/windows-release.yml` is a reusable Windows Server 2022
workflow. The called nightly path receives the source SHA, package version,
publication version, build ID, and timestamp from the macOS release workflow and
fails if any identity differs. It has `contents: read` only and references no
secrets, Apple credentials, PortalSurfer token, publisher key, or OIDC
permission. Standalone dispatch is available for Windows artifact inspection;
it is not a publication path.

The macOS job validates the Windows sidecar and exact two-file Windows artifact
directory before assembly. It copies only the archive into the final release
root; the sidecar is provenance evidence, not a fourth published artifact.

## Manifest compatibility

Stable and RC releases remain macOS-only schema-2 manifests. Production
nightlies use schema 3 and require the exact three-target set above. All release
channels retain the package version's core semver; RC and nightly publication
versions carry their channel suffix and are checked against the package version.
The default UI screenshot remains `gainsnap-default-300x320.png`.

The generic PortalSurfer publisher is pinned to commit
`12d2c089d3d135c6839013a097dbf3baebf5fdb3`. A protected publisher-integration
job checks the required preflight run and stages only that pinned publisher
script for the production macOS job. `PORTALSURFER_RELEASE_TOKEN` remains the
production upload secret. Windows never receives any of these credentials.
