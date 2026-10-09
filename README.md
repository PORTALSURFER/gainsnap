# GainSnap

Peak and RMS level matching for your DAW

**Pay what you want, including €0.** Donations are optional; no license activation is needed.

AudioDev plug-in repository (gainsnap), category **Utility**.

## Development

GainSnap starts as a manual gain utility. The knob and right meter arrow edit the
same Gain parameter. While Match is enabled, GainSnap measures the input and
adjusts that parameter toward the selected target. Turning Match off freezes
the setting: later upstream level changes do not rematch. Enter a target below
the meter, then click the adjacent Match icon; the smaller icon restarts matching.
Shared host and GUI mechanics remain in Toybox.

Click the Peak or RMS output readout to select the matching measurement. New
instances use the last explicitly selected Peak/RMS mode across DAW relaunches
(RMS until a preference is saved); saved projects retain their own mode. RMS matching uses
the strongest complete 300 ms sliding mean-square window of each channel and
links gain to the louder channel, preserving stereo balance. The output meter
shows the latest 300 ms sliding RMS window. The scale uses a full-scale square
wave as 0 dBFS RMS; a full-scale sine reads about -3.01 dBFS RMS. The mode is
saved with the project and can be automated in CLAP and VST3.

Both modes retain their strongest evidence for the entire Match session. Peak
keeps the highest stereo-linked sample peak; RMS keeps the strongest complete
300 ms mean-square window.
Quiet passages and silence never erase that evidence or raise the correction.
After the gain settles, the activity rail remains confident until newly stronger
evidence requires another adjustment. Leave Match on until the loudest part has
played. Press Restart, or turn Match off and on, to start a fresh measurement.
Changing the target reuses the retained evidence; changing mode starts a new session.
RMS permits peaks above its average target and above 0 dBFS.
Peak targets that would require more than −36 dB attenuation or +120 dB boost
cannot be reached; peaks at or below −120 dBFS are treated as silence. RMS
matching retains its +24 dB boost cap. The meter
always shows the actual output. The bottom-left N button toggles Normalize:
when matching is off, it selects Peak, sets 0 dBFS, and starts Match;
clicking it again stops matching and holds the audible gain.

Engaging Match holds the audible gain for at least one second, requiring
repeated signal evidence and a measurement stable within 0.5 dB for half a
second before correcting. A lone hit followed by silence cannot trigger a
boost. RMS also waits for a complete window. Automatic corrections change by
at most 24 dB per second in either direction. Restarts and target changes begin
from the gain actually heard. Stopping Match holds that audible gain, even
midway through a correction.

GainSnap applies smooth, linear gain without a sample-peak limiter or clipping
at 0 dBFS. Louder transients can exceed the target while matching adapts;
manual and held gain also pass signals above full scale to the host. Only
non-finite inputs and numeric overflow at the floating-point range boundary
are contained. Output clipping is the responsibility of downstream processing
or the final hardware/export stage.

When the target meter has keyboard focus, Up/Down (and Left/Right) change the
target by 1.0 dB per step. Hold Shift for 0.1 dB steps. The numeric field below
the meter accepts direct dBFS entry as well.

The tall meter shows the output peak in orange and RMS in purple, with numeric
readouts beside it. The left arrow and horizontal line set the target. Select
Peak or RMS to choose which level Match uses. Drag the thick inner ring of the
round gain control for coarse adjustments, even beyond the editor, or drag the
right meter arrow for fine adjustments. The arrow sits at −12 dB when Gain is
0 dB and stays independent of the moving meter bars. Click the value
beneath the knob or meter once to select the entire number and type a replacement.
Arrow keys adjust a focused control; Shift makes smaller keyboard steps. Hold
Shift while dragging either meter arrow or the knob to snap its value to whole
dB steps.
Drag either numeric field vertically to adjust its value; hold Shift for finer steps.
Both meter triangles and numeric field drags continue outside the editor until
the mouse button is released. The gain triangle spans the full meter travel,
with its −36 dB minimum at the bottom and unity at the −12 dB scale position.
Match lights orange and pulses softly while listening or adjusting.
Space passes through to the host's transport, including when a button or numeric
field is focused. Enter activates the focused button.
Editing either gain control stops Match. The default editor is
200 × 424 logical pixels. The approved Sift Hardware r5 design uses a graphite/sage enclosure,
a tall left output meter with coral Peak and purple RMS bars, a smaller lower-right
gain knob, compact numeric fields, and footer Match and icon-only Restart controls.
Edge-connected metal inserts and muted gradients give the surfaces shallow depth. Peak and RMS meters keep separate colored peak rules for two
seconds, then release at 1.5 dB per second without dropping below the live bar.
The three-stage activity rail spans the bottom, while its status text sits
below the output readouts during Match.
Manual and held states omit redundant status text; no-signal feedback remains visible.
Matched means the correction has settled. Below target means the
requested RMS needs more gain than the gain range allows; GainSnap
does not compress or limit the transients to reach it. RMS matching uses the strongest measured 300 ms window; the
live RMS readout may fall between hits. After a correction, the telemetry
keeps Adjusting visible for about 200 ms so short gain changes remain legible
at normal editor refresh rates. Native macOS and Windows editors share this
surface, including host keyboard input, focus, and DPI-aware resizing.
Quieter passages read below the target; newly encountered louder peaks pass
through the smooth gain while the matcher adapts.

The initializer creates a local git repository on main and stages generated files. Review and commit that local repository before remote setup.

Local checks:

bash scripts/ci.sh
VST3_SDK_DIR=/path/to/vst3sdk bash scripts/ci.sh --vst3
bash scripts/dist.sh --format clap
             VST3_SDK_DIR=/path/to/vst3sdk bash scripts/dist.sh

The local macOS nightly path runs CI, Developer ID signing, notarization,
stapling, and direct PortalSurfer publication without GitHub Actions. See
[local nightly releases](docs/NIGHTLY_RELEASES.md) for setup and commands.

GitHub Actions release workflows remain available by manual dispatch and can
also publish signed, notarized, and stapled macOS arm64 CLAP/VST3 releases.
Those hosted production nightlies additionally contain an unsigned Windows
x86_64 VST3 in one immutable schema-3 manifest. The exact
Windows archive is
`gainsnap-v<publication-version>-windows-x86_64-unsigned.vst3.zip`; stable and
RC releases remain macOS-only schema-2 releases. The reusable Windows workflow
and `scripts/windows_release_helper.py` validate the bundle layout, PE
architecture, absence of Authenticode signing, dependency pins, and shared
release identity. Windows receives no Apple or PortalSurfer credentials.

Nightlies prepare a protected patch-version bump before release; retries reuse
the unpublished version. See [docs/NIGHTLY_RELEASES.md](docs/NIGHTLY_RELEASES.md)
for scheduler commands, approval gates, and retry behavior.

## Staged bootstrap

From the AudioDev root, use the dependency-ordered commands below. Every command plans by default; add --execute to allow its own mutation:

cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- init --name gainsnap --display-name GainSnap --category Utility --tagline "Peak and RMS level matching for your DAW" --description "GainSnap measures the selected Peak or RMS level while Match is enabled, adjusts gain toward the target, and holds that gain when Match is disabled. Enter a target below the meter, choose Peak or RMS from the output readouts, and use the adjacent Match and rematch icons. The vertical meter shows output Peak and RMS with a target marker."
cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- remote --plugin gainsnap
cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- credentials --plugin gainsnap
cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- landing --plugin gainsnap --site-root /path/to/portalsurfer.org
cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- deploy --plugin gainsnap --site-root /path/to/portalsurfer.org
cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- publisher --plugin gainsnap

bootstrap runs all six stages in dependency order; publisher follows deploy because the public product endpoint must be live. The credentials stage requires --execute, an interactive terminal, the exact SET CREDENTIALS gainsnap gate, and hidden prompts; deploy asks for DEPLOY gainsnap; publisher asks for PROVISION PUBLISHER gainsnap or ROTATE PUBLISHER gainsnap.

## Landing page

site/product.json is the release/catalog contract and site/landing-page.json is the actual PortalSurfer page content input. The landing stage renders the page, updates the catalog, and registers the backend product locally. See docs/landing-page-contract.md.

## Credentials

The credentials stage handles only the listed ordinary GitHub Actions entries through hidden stdin prompts or the execute-only Apple .p12/.p8 path options; it never persists or logs values and never handles server-side SSH/deploy credentials. Supplied files are checked as regular files with the expected extension and a bounded size, encoded in memory after the confirmation gate, and sent only through gh standard input. The per-product PortalSurfer release credential belongs to the publisher stage. The pinned PORTALSURFER/toybox and GPUI dependencies are public, so no repository credential is required. See docs/RELEASE_CREDENTIALS.md and docs/RELEASE_PUBLISHER.md for the exact contracts.

Match switches off when the plug-in editor is closed, hidden, or minimized.
Stopped matches and saved projects keep their gain without background matching.
The meter updates while the editor is open, including when Match is off.
