# GainSnap

Toggle peak matching for Ableton tracks

AudioDev plug-in repository (gainsnap), category **Utility**.

## Development

GainSnap measures a stereo track's finite sample peak while Match is enabled,
applies the bounded gain correction needed to reach the selected target when
Match is disabled, and holds that correction until the next measurement. The
Normalize button sets the target to 0 dBFS and starts Match in one click.
Shared host and GUI mechanics remain in Toybox.

When the target slider has keyboard focus, Up/Down (and Left/Right) change the
target by 1.0 dB per step. Hold Shift for 0.1 dB steps. The numeric field below
the slider accepts direct dBFS entry as well.

The vertical bar shows the latest block peaks in realtime: the orange marker on
the left is the incoming (IN) level, and the cyan marker on the right is the
adjusted (OUT) level. The IN and OUT readouts remain the exact dBFS values.

The initializer creates a local git repository on main and stages generated files. Review and commit that local repository before remote setup.

Local checks:

bash scripts/ci.sh
VST3_SDK_DIR=/path/to/vst3sdk bash scripts/ci.sh --vst3
bash scripts/dist.sh --format clap
             VST3_SDK_DIR=/path/to/vst3sdk bash scripts/dist.sh

GitHub Actions release workflows also build an x86_64 Windows artifact for
every declared format. Windows artifacts are intentionally unsigned and remain
Actions artifacts; their archive names include `windows-unsigned`. To reproduce
the package lane on Windows, use `pwsh scripts/release-windows.ps1 -Channel
nightly -SourceSha (git rev-parse HEAD) -Formats "clap,vst3"` with `VST3_SDK_DIR`
set to a pinned SDK checkout. The script audits the installable CLAP file or
VST3 bundle before returning.

## Staged bootstrap

From the AudioDev root, use the dependency-ordered commands below. Every command plans by default; add --execute to allow its own mutation:

cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- init --name gainsnap --display-name GainSnap --category Utility --tagline "Toggle peak matching for Ableton tracks" --description "GainSnap measures an incoming track peak while Match is enabled, applies the gain needed to reach a chosen target when Match is disabled, and holds that gain until the next measurement. Normalize sets the target to 0 dBFS and starts Match in one click. The vertical bar shows realtime incoming (IN) and adjusted (OUT) peak markers."
cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- remote --plugin gainsnap
cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- credentials --plugin gainsnap
cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- landing --plugin gainsnap --site-root /path/to/portalsurfer.org
cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- deploy --plugin gainsnap --site-root /path/to/portalsurfer.org
cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- publisher --plugin gainsnap

bootstrap runs all six stages in dependency order; publisher follows deploy because the public product endpoint must be live. The credentials stage requires --execute, an interactive terminal, the exact SET CREDENTIALS gainsnap gate, and hidden prompts; deploy asks for DEPLOY gainsnap; publisher asks for PROVISION PUBLISHER gainsnap or ROTATE PUBLISHER gainsnap.

## Landing page

site/product.json is the release/catalog contract and site/landing-page.json is the actual PortalSurfer page content input. The landing stage renders the page, updates the catalog, and registers the backend product locally. See docs/landing-page-contract.md.

## Credentials

The credentials stage handles only the listed ordinary GitHub Actions entries through hidden stdin prompts or the execute-only Apple .p12/.p8 path options; it never persists or logs values and never handles server-side SSH/deploy credentials. Supplied files are checked as regular files with the expected extension and a bounded size, encoded in memory after the confirmation gate, and sent only through gh standard input. The per-product PortalSurfer release credential belongs to the publisher stage. The pinned PORTALSURFER/radiant dependency is public, so no repository credential is required. See docs/RELEASE_CREDENTIALS.md and docs/RELEASE_PUBLISHER.md for the exact contracts.
