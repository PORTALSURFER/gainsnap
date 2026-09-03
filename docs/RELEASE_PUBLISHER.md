# PortalSurfer publisher credential

Target product: GainSnap (`gainsnap`)
Target repository: PORTALSURFER/gainsnap

The publisher stage creates the per-product release credential used by GitHub Actions to publish this product. It generates the bearer value locally from 32 operating-system random bytes; you never enter or see that value. Only its SHA-256 hash is sent to PortalSurfer's fixed Compose-backed wrapper (`sh /opt/portalsurfer/hosting/audiodev-publisher-admin.sh` by default). The wrapper runs the helper in the isolated admin service and owns the private-file path. The raw value is sent only to the GitHub production environment secret and is then dropped.

## Preview

cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- publisher --plugin /path/to/PLUGIN

Plan mode performs no SSH, GitHub, HTTP, randomness, prompt, or file mutation. Run it after the landing page has been deployed so the public product endpoint exists.

## Provision

cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- publisher --plugin /path/to/PLUGIN --execute

The command performs read-only SSH/wrapper, public product endpoint, GitHub ADMIN/Actions-key, and transaction-state checks. It then requires the exact confirmation `PROVISION PUBLISHER gainsnap`. A missing GitHub `production` environment is created only after that gate.

To replace an existing product credential, add `--rotate` and confirm `ROTATE PUBLISHER gainsnap`. `--confirm-publisher` may supply that exact phrase for an already interactive terminal; it never bypasses the terminal requirement.

## Connection options

`--server`, `--user`, `--remote-path`, and `--public-origin` resolve from `PORTALSURFER_DEPLOY_SERVER`, `PORTALSURFER_DEPLOY_USER`, `PORTALSURFER_REMOTE_PATH`, and `PORTALSURFER_PUBLIC_ORIGIN` when omitted (with the established deployment defaults). `--key-path PATH` resolves from `PORTALSURFER_DEPLOY_KEY_PATH` when omitted and passes the path only to `ssh`; `--no-key-path` forces the active SSH agent. SSH uses strict host-key checking and batch mode, never accepts a password, and never reads, copies, or persists a private key.

## Failure and recovery

The remote wrapper runs a locked, checksummed, recoverable transaction through its isolated helper. A failure before the GitHub secret process starts rolls that transaction back automatically. Once `gh secret set` has started, any result is ambiguous because GitHub may have persisted the secret even when `gh` exits nonzero; the remote transaction remains pending and must not be rolled back. Verify the production GitHub secret, inspect the wrapper's `check` state if needed, and explicitly run `finalize` for the printed transaction ID. A remote finalization failure follows the same finalization-only recovery. The raw bearer value is not included in diagnostics or recovery commands.
