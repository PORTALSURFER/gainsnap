# Landing-page and release integration contract

The bootstrap flow keeps product release metadata and rendered page content separate. site/product.json is the release/catalog input; site/landing-page.json is the complete page-template content consumed by the PortalSurfer generator.

## Stages

1. init creates the independent local Toybox repository and both site inputs.
2. remote optionally creates/configures PORTALSURFER/gainsnap and pushes only with --execute. The generated release workflows publish to /plugins/api/v1/products/gainsnap/releases.
3. credentials configures only ordinary GitHub Actions entries after its SET CREDENTIALS gainsnap gate; the per-product release credential is not entered here.
4. landing renders /plugins/gainsnap/, updates the catalog, and idempotently upserts hosting/audiodev-products.json.
5. deploy invokes PortalSurfer's scripts/deploy.sh only with --execute, after showing the target and requiring DEPLOY gainsnap. It then checks the public page title with curl.
6. publisher runs after deploy, validates the registered remote product, and requires PROVISION PUBLISHER gainsnap or ROTATE PUBLISHER gainsnap before creating the per-product release credential.

## Product release contract

- Stable slug: gainsnap
- Repository: PORTALSURFER/gainsnap
- Release API: /plugins/api/v1/products/gainsnap/releases
- Page: /plugins/gainsnap/
- Formats: clap, vst3
- Public catalog: signed macOS arm64 CLAP/VST3 stable, RC, and nightly releases
- Public nightly additionally includes one unsigned Windows x86_64 VST3 artifact in a schema-3 manifest
- Stable and RC remain macOS-only schema-2 manifests
- Exact Windows archive: `gainsnap-v<publication-version>-windows-x86_64-unsigned.vst3.zip`

## Safety

Plan mode is the default for every stage and for bootstrap. Input is validated before an execute stage mutates anything. Existing generated output is resumed only when its identity matches; unrelated landing pages are never overwritten. Only explicitly prompted GitHub Actions values pass transiently through gh standard input; SSH keys and server .env values remain outside this CLI.
