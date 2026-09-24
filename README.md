# GainSnap

Peak and RMS level matching for your DAW

AudioDev plug-in repository (gainsnap), category **Utility**.

## Development

While Match is enabled, GainSnap measures a stereo track's finite sample peak
and applies the bounded gain correction needed to reach the selected target.
Peak mode remembers the highest sample peak for the current Match session;
quieter passages never raise the gain, and louder peaks lower it. Disabling
Match holds that correction unless a louder input exceeds the target, when it
reduces the held gain. The Normalize button sets the target to 0 dBFS
and starts Match in one click.
Shared host and GUI mechanics remain in Toybox.

The PEAK/RMS button selects the matching measurement and output meter. Peak
remains the default, including when loading older projects. RMS matching uses
the strongest complete 300 ms sliding mean-square window of each channel and
links gain to the louder channel, preserving stereo balance. The output meter
shows the latest 300 ms sliding RMS window. The scale uses a full-scale square
wave as 0 dBFS RMS; a full-scale sine reads about -3.01 dBFS RMS. The mode is
saved with the project and can be automated in CLAP and VST3.

RMS matching observes the first 300 ms using conservative peak correction,
then follows the strongest complete window in roughly the most recent three
seconds. RMS evidence ages out, allowing Match to settle toward a sustained
upstream level change while the stability check helps avoid chasing decaying
tails and short gaps.
A lower rolling estimate is published only after recent usable evidence remains
within 0.5 dB for about 300 ms; newly louder evidence updates promptly. Silence
after an observed hit still completes that hit's window, and after two seconds of
silence, returning audio gets a fresh protected startup. RMS permits peaks above
its average target, but requested gain is bounded by the headroom of the largest
recent sample peak; the existing 0 dBFS sample-peak guard remains active as a
safety net.
Peak mode keeps its session maximum until Match is turned off or Restart is
pressed. A newly louder upstream peak automatically lowers the matched gain;
use Restart to discard the old maximum when upstream audio becomes quieter.
RMS mode retains its rolling adaptation behavior and may take
roughly 3–4 seconds of steady material to respond.
Peak targets that would require more than −24 dB attenuation or +120 dB boost
cannot be reached; peaks at or below −120 dBFS are treated as silence. RMS
matching retains its +24 dB boost cap and peak-headroom protection. The meter
always shows the actual protected output. Normalize switches to PEAK, sets 0
dBFS, and starts Match.

Engaging Match while audio is playing first fades the audible gain down over
10 ms, then fades the matched output up over 300 ms. This avoids the waveform
jump caused by an immediate mute. Starting from silence waits for usable audio
before fading up. Rapid restarts and target changes begin from the gain actually
being heard. Gain boosts use a 100 ms response and reductions use 10 ms, with
higher-precision smoothing at high sample rates.

A stereo-linked sample-peak guard reacts immediately to bursts and recovers over
100 ms. The short outgoing transition retains its previous ceiling while fading
down; the Peak-mode path remains capped at the selected target after Match is
held. Turning Match off early lets the transition finish.
Protection adds no latency, and the meter includes its attenuation.

When the target meter has keyboard focus, Up/Down (and Left/Right) change the
target by 1.0 dB per step. Hold Shift for 0.1 dB steps. The numeric field below
the meter accepts direct dBFS entry as well.

The tall meter shows the output peak in orange and RMS in teal, with numeric
readouts beside it. The left arrow and horizontal line set the target. Select
Peak or RMS to choose which level Match uses. In Manual mode, drag the round
gain control vertically for coarse adjustments or drag the right meter arrow
for fine adjustments. Arrow keys adjust a focused control; Shift makes smaller
steps. Using either gain control enters Manual mode. Match and Normalize return
to Auto mode. Manual gain remains fixed when upstream audio gets louder; the
sample-peak guard still prevents output above 0 dBFS. The default editor is
250 × 424 logical pixels. The activity rail below Normalize has three short
stages: Listening, Adjusting, and Matched. It reports live audio activity
independently of the Match lifecycle; disabled instances show Ready, Held, or
No signal. Matched means the correction has settled within the available gain
and peak-headroom limits, so an unreachable target can still settle at the
closest protected level. After a correction or peak-guard event, the telemetry
keeps Adjusting visible for about 200 ms so short gain changes remain legible
at normal editor refresh rates. Native macOS and Windows editors share this
surface, including host keyboard input, focus, and DPI-aware resizing.
Quieter passages read below the target; while matching, the peak guard contains
newly encountered louder peaks as the gain settles. The guard limits sample
peaks, rather than reconstructed intersample peaks.

The initializer creates a local git repository on main and stages generated files. Review and commit that local repository before remote setup.

Local checks:

bash scripts/ci.sh
VST3_SDK_DIR=/path/to/vst3sdk bash scripts/ci.sh --vst3
bash scripts/dist.sh --format clap
             VST3_SDK_DIR=/path/to/vst3sdk bash scripts/dist.sh

GitHub Actions release workflows publish signed, notarized, and stapled macOS
arm64 CLAP/VST3 releases. Public production nightlies additionally contain an
unsigned Windows x86_64 VST3 in one immutable schema-3 manifest. The exact
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

cargo run --manifest-path audiodev-plugin-bootstrap/Cargo.toml -- init --name gainsnap --display-name GainSnap --category Utility --tagline "Peak and RMS level matching for your DAW" --description "GainSnap measures an incoming track peak while Match is enabled, continuously applies the gain needed to reach a chosen target while Match is enabled, and holds that gain when Match is disabled. Normalize sets the target to 0 dBFS and starts Match in one click. The vertical meter shows a smoothed orange output peak with a dB scale and target marker."
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

Match switches off when the plug-in editor is closed, hidden, or minimized. The audio processor holds the last match and checks for a level increase that would exceed the target. It lowers the held gain only when needed, without running continuous matching history or GUI polling. Reopen the editor to inspect or restart the match.
Completed matches remain protected after a host processing restart or a saved
project is reopened. Untouched instances and older saved states begin unarmed.
