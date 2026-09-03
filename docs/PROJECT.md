# GainSnap

## Summary

GainSnap measures an incoming track peak, applies the gain needed to reach a chosen target, and holds that gain when matching is complete.

## Constraints

- Keep audio processing realtime-safe: no allocation, blocking, or secret handling in the audio callback.
- Keep this repository thin; shared GUI/host mechanics belong in Toybox.
- Windows release artifacts are unsigned, are emitted only by GitHub Actions, and must not enter the macOS signing/notarization or public PortalSurfer publication path.
- Landing-page content is generated from site/landing-page.json and registered through the staged CLI.
