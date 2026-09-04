# GainSnap

## Summary

GainSnap measures an incoming track peak while Match is enabled, applies the
gain needed to reach a chosen target when Match is disabled, and holds that
gain until the next measurement. Normalize sets the target to 0 dBFS and
starts Match in one click. The vertical meter shows a smoothed incoming peak
in realtime as one thick orange column, with a small dB scale at its left edge.
A small triangle beside the meter marks the target and acts as the target
control.

## Constraints

- Keep audio processing realtime-safe: no allocation, blocking, or secret handling in the audio callback.
- Keep this repository thin; shared GUI/host mechanics belong in Toybox.
- Windows release artifacts are unsigned, are emitted only by GitHub Actions, and are validated as the public Windows sidecar of production nightly schema-3 manifests; they never receive Apple signing/notary credentials or enter the macOS signing path.
- Landing-page content is generated from site/landing-page.json and registered through the staged CLI.
