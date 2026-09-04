# GainSnap

## Summary

GainSnap tracks an incoming running peak while Match is enabled and applies
bounded gain correction toward a chosen target in realtime. When Match is
disabled, the final running-peak correction is locked until the next
measurement. Normalize sets the target to 0 dBFS and starts Match in one click.
The vertical meter shows a smoothed post-gain output peak in realtime as one
thick orange column, with a small dB scale at its left edge. A small triangle
beside the meter marks the target and acts as the target control.

## Constraints

- Keep audio processing realtime-safe: no allocation, blocking, or secret handling in the audio callback.
- Keep this repository thin; shared GUI/host mechanics belong in Toybox.
- Windows release artifacts are unsigned, are emitted only by GitHub Actions, and must not enter the macOS signing/notarization or public PortalSurfer publication path.
- Landing-page content is generated from site/landing-page.json and registered through the staged CLI.
