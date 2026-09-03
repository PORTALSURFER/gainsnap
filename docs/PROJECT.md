# GainSnap

## Summary

GainSnap measures an incoming track peak while Match is enabled, applies the
gain needed to reach a chosen target when Match is disabled, and holds that
gain until the next measurement. Normalize sets the target to 0 dBFS and
starts Match in one click. The vertical bar shows realtime incoming (IN) and
adjusted (OUT) peak markers.

## Constraints

- Keep audio processing realtime-safe: no allocation, blocking, or secret handling in the audio callback.
- Keep this repository thin; shared GUI/host mechanics belong in Toybox.
- Windows release artifacts are unsigned, are emitted only by GitHub Actions, and must not enter the macOS signing/notarization or public PortalSurfer publication path.
- Landing-page content is generated from site/landing-page.json and registered through the staged CLI.
