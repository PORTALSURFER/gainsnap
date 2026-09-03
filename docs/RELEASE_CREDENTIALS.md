# Release credentials and configuration

Target repository: PORTALSURFER/gainsnap

This document is generated from the same checklist shown by the staged bootstrapper. The credentials stage can create or update only the GitHub Actions entries listed below. It never handles server-side SSH/deployment credentials.

Credentials checklist for PORTALSURFER/gainsnap

GitHub Actions secrets handled by the credentials stage (each value is prompted with terminal echo disabled and is sent only to gh over standard input):
- APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64 (required when configured workflow path is used)
  destination: GitHub > PORTALSURFER/gainsnap > Settings > Secrets and variables > Actions > Environments > production > Environment secrets
  when: Required before the first release workflow run, including package-only; not needed by release-preflight.
  why: Base64 of a password-protected Developer ID Application .p12 containing the signing certificate and private key; the workflow imports it into an ephemeral keychain.
- APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD (required when configured workflow path is used)
  destination: GitHub > PORTALSURFER/gainsnap > Settings > Secrets and variables > Actions > Environments > production > Environment secrets
  when: Required before the first release workflow run, including package-only; not needed by release-preflight.
  why: Password used to import the Developer ID Application .p12.
- APPLE_NOTARY_KEY_BASE64 (required when configured workflow path is used)
  destination: GitHub > PORTALSURFER/gainsnap > Settings > Secrets and variables > Actions > Environments > production > Environment secrets
  when: Required before the first release workflow run, including package-only; not needed by release-preflight.
  why: Base64 of the App Store Connect API private .p8 key used by notarytool.
- APPLE_NOTARY_KEY_ID (required when configured workflow path is used)
  destination: GitHub > PORTALSURFER/gainsnap > Settings > Secrets and variables > Actions > Environments > production > Environment secrets
  when: Required before the first release workflow run, including package-only; not needed by release-preflight.
  why: App Store Connect API key ID paired with APPLE_NOTARY_KEY_BASE64.
- APPLE_NOTARY_ISSUER_ID (required when configured workflow path is used)
  destination: GitHub > PORTALSURFER/gainsnap > Settings > Secrets and variables > Actions > Environments > production > Environment secrets
  when: Required before the first release workflow run, including package-only; not needed by release-preflight.
  why: App Store Connect issuer ID paired with the notary API key; no separate Apple team-ID field is read.
- APPLE_CODESIGN_IDENTITY (optional)
  destination: GitHub > PORTALSURFER/gainsnap > Settings > Secrets and variables > Actions > Environments > production > Environment secrets
  when: Optional; set only when automatic Developer ID Application identity selection is ambiguous or selects the wrong certificate.
  why: Explicit Developer ID Application identity override; the release script discovers it when this is absent.

GitHub Actions variables: none are referenced by the generated workflows; the credentials stage creates no variables.

The pinned PORTALSURFER/radiant dependency is public; generated workflows do not require a repository token.
The per-product PortalSurfer release credential is provisioned by the separate publisher stage after the landing page has been deployed; it is not entered in this ordinary credentials stage.

Not managed by the credentials stage; configure these directly before the relevant operation:
- AUDIODEV_PRODUCTS_FILE [PortalSurfer server variable]
  destination: PortalSurfer server compose environment / mounted configuration
  when: Required for the deployed product registry; normally already supplied by the PortalSurfer compose file.
  why: Path to the mounted AudioDev product registry, normally /config/audiodev-products.json.
- PORTALSURFER_DEPLOY_SERVER [PortalSurfer deployment setting]
  destination: PortalSurfer scripts/deploy.sh environment or command-line setting
  when: Only needed when using the CLI deploy stage and the default server is not correct.
  why: SSH host for the established PortalSurfer deployment script; not a GitHub Actions release setting.
- PORTALSURFER_DEPLOY_USER [PortalSurfer deployment setting]
  destination: PortalSurfer scripts/deploy.sh environment or command-line setting
  when: Only needed when using the CLI deploy stage and the default SSH user is not correct.
  why: SSH user for the established PortalSurfer deployment script.
- PORTALSURFER_DEPLOY_KEY_PATH [PortalSurfer deployment setting]
  destination: PortalSurfer scripts/deploy.sh environment or command-line setting
  when: Optional; only when SSH-agent authentication is not used.
  why: Local path passed to SSH; the bootstrapper never reads or uploads the key contents.
- PORTALSURFER_REMOTE_PATH [PortalSurfer deployment setting]
  destination: PortalSurfer scripts/deploy.sh environment or command-line setting
  when: Only needed when the PortalSurfer installation is not at the deploy script default.
  why: Remote installation path for the established deployment script.
- PORTALSURFER_SITE_DOMAIN [PortalSurfer deployment setting]
  destination: PortalSurfer scripts/deploy.sh environment or command-line setting
  when: Only needed when the deployed site uses a domain other than the deploy script default.
  why: Caddy/site-domain value for the established deployment script.
- PORTALSURFER_PUBLIC_ORIGIN [PortalSurfer deployment setting]
  destination: PortalSurfer scripts/deploy.sh environment or command-line setting
  when: Only needed to override the public origin used for deployment/payment-return configuration.
  why: HTTP(S) origin used by the established deployment script and the CLI reachability check.
- PORTALSURFER_ANCHOR_PATH [PortalSurfer deployment setting]
  destination: PortalSurfer scripts/deploy.sh environment or command-line setting
  when: Optional; only when the Anchor checkout is not the site's sibling ../anchor directory.
  why: Local Anchor checkout path used while packaging the established PortalSurfer deployment archive.

Credential safety: plan mode is the default. Execute requires an interactive terminal, gh auth/permission preflight, --execute, and the exact SET CREDENTIALS <slug> gate. Values are never accepted from arguments, environment variables, plan output, generated files, or logs; server SSH/deploy credentials are never read by this CLI. Apple signing/notary material is accepted through a hidden prompt or an explicit execute-only .p12/.p8 path, read only after the gate, encoded in memory, and forwarded directly to gh standard input. Supplied files must be regular files with the expected extension and a bounded size; they are never copied or persisted. Existing GitHub entries may be kept by submitting a blank hidden prompt. GITHUB_TOKEN is supplied automatically by GitHub Actions.
Use: cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- credentials --plugin /path/to/gainsnap [--execute]

## Credential-stage execution

Preview without mutation:

cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- credentials --plugin /path/to/PLUGIN

Execute only from an interactive terminal after reviewing the plan:

cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- credentials --plugin /path/to/PLUGIN --execute

The stage first runs `gh auth status` without `--show-token`, verifies repository `ADMIN` access, checks the repository and production-environment Actions public keys, and inventories names only. It then displays the checkpoint and requires the exact confirmation `SET CREDENTIALS gainsnap`. Secret prompts disable terminal echo; values go to `gh secret set` through child-process standard input with no value-bearing argument (the `--body` option is omitted so gh reads standard input), are not put in arguments/files/environment/logs, and are dropped after each update. An existing value can be retained with a blank prompt. For Apple credentials, `--apple-cert-path PATH` reads a regular `.p12` and `--apple-notary-key-path PATH` reads a regular `.p8` only after the gate; each is bounded, base64-encoded in memory, and streamed to its matching GitHub secret. These file options require `--execute`; plan mode never reads them. The CLI rejects non-interactive execute mode and never accepts secret values through arguments or environment variables.

`PORTALSURFER/radiant` is public, so generated workflows fetch its pinned dependency directly without a repository credential.

The separate publisher stage provisions the per-product PortalSurfer release credential after the product is registered and reachable. It does not ask you to invent or paste a bearer token.

The generated release workflows use GitHub's automatic `GITHUB_TOKEN` only for the metadata-only GitHub Release operation. No separate GitHub team ID, provisioning profile, SSH private key, or server credential is read by this stage. Apple key files are read only when their explicit execute-only path options are supplied, and are never copied or persisted.
