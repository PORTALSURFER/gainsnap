# Changelog

## Unreleased

- Pass signals above 0 dBFS without sample-peak limiting, preserving transients
  and avoiding abrupt limiter attenuation. RMS matching no longer caps gain by
  sample-peak headroom; its +24 dB boost limit remains.
- Add techno Peak presets: Kick −12, Sub −14, Tom / Mid-bass −16, Percs −18,
  Textures / Synths −20, Effects −22 dBFS, and Normalize at 0 dBFS.
- Show actual applied gain, adjustment direction, and retained RMS shortfall.
- Add double-click gain/target resets, clearer labels and keyboard focus,
  and compact Help and Presets panels.
- Make floating Peak/RMS stripes brighter and easier to see as they fall slowly.
- Include versioned PDF and offline HTML manuals in the release package.

## 0.1.0 - 2026-09-03

- Initial plugin scaffold.
- Added a compact Normalize action that sets the target to 0 dBFS and starts Match.
- Added a smoothed realtime orange input meter with a dB scale and target marker.
