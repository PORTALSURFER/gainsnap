# GainSnap

## Summary

While Match is enabled, GainSnap measures an incoming track peak and continuously
applies the gain needed to reach a chosen target. Disabling Match holds that
gain. Normalize sets the target to 0 dBFS and
starts Match in one click. The vertical meter shows a smoothed output peak
in realtime as one thick orange column, with a small dB scale at its left edge.
A small triangle beside the meter marks the target and acts as the target
control.

Match starts with a 300 ms fade from silence once signal arrives. Gain increases
use a slower 100 ms response than the 10 ms reductions. A stereo-linked sample
peak guard caps matching output at the target, and otherwise caps output at
0 dBFS, with immediate attack and 100 ms recovery. The fade continues if Match
is disabled early. Output metering includes this protection.

## Constraints

- Keep audio processing realtime-safe: no allocation, blocking, or secret handling in the audio callback.
- Keep this repository thin; shared GUI/host mechanics belong in Toybox.
- Windows release artifacts are unsigned, are emitted only by GitHub Actions, and must not enter the macOS signing/notarization or public PortalSurfer publication path.
- Landing-page content is generated from site/landing-page.json and registered through the staged CLI.
