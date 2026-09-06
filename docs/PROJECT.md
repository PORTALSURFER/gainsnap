# GainSnap

## Summary

While Match is enabled, GainSnap measures an incoming track peak and continuously
applies the gain needed to reach a chosen target. Disabling Match holds that
gain. Normalize sets the target to 0 dBFS and
starts Match in one click. The vertical meter shows a smoothed output peak
in realtime as one thick orange column, with a small dB scale at its left edge.
A small triangle beside the meter marks the target and acts as the target
control.

Match fades playing audio down over 10 ms before a 300 ms fade up. Silent
starts wait for usable signal. Restarts preserve the currently audible gain. Gain increases
use a slower 100 ms response than the 10 ms reductions. A stereo-linked sample
peak guard retains the previous ceiling during the outgoing fade, then caps
matching output at the target and held output at 0 dBFS, with immediate attack and 100 ms recovery. The fade continues if Match
is disabled early. Output metering includes this protection.
The startup fade follows the peak guard, bounding strong bursts throughout the
fade. The 208 × 212 editor groups its controls tightly; the Match button and
status dot share a pulse while Match is enabled. Both macOS and Windows use
Toybox's native Radiant host, including keyboard, focus, and DPI conversion.

## Constraints

- Keep audio processing realtime-safe: no allocation, blocking, or secret handling in the audio callback.
- Keep this repository thin; shared GUI/host mechanics belong in Toybox.
- Windows release artifacts are unsigned, are emitted only by GitHub Actions, and are validated as the public Windows sidecar of production nightly schema-3 manifests; they never receive Apple signing/notary credentials or enter the macOS signing path.
- Landing-page content is generated from site/landing-page.json and registered through the staged CLI.
