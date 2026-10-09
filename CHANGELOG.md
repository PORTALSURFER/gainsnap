# Changelog

## Unreleased

- Make the Help button smaller and use a compact Sift-style shortcut menu with
  orange key labels, omitting general usage and host transport shortcuts.

- Extend target levels and gain attenuation down to −60 dB, including meter
  dragging, numeric entry, matching, and saved state.

## 0.1.8

- Remove the duplicate Applied dB and adjustment-status readout. Keep the
  matching indicator, gain controls, and RMS shortfall feedback.

## 0.1.7

- Hold automatic matching until repeated signal evidence and a stable measurement
  establish confidence; isolated hits and silent preroll cannot trigger a boost.
  Bound matching gain changes to 24 dB per second and hold the audible gain when
  matching stops midway through a correction.

- Replace the preset list with a compact Normalize toggle left of the bottom
  target field. Enabling it selects Peak, sets the target to 0 dBFS, and starts
  Match; clicking again stops matching and holds the gain.

- Remember the last explicitly chosen Peak/RMS mode for new instances across
  DAW relaunches, while preserving each saved project's mode.

## 0.1.6

- Pass signals above 0 dBFS without sample-peak limiting, preserving transients
  and avoiding abrupt limiter attenuation. RMS matching no longer caps gain by
  sample-peak headroom; its +24 dB boost limit remains.
- Add mode-aware techno presets for a +12 dB main-bus boost. Peak presets
  target −12 dBFS or lower; RMS presets use lower average targets. Normalize
  targets −12 dBFS Peak or −24 dBFS RMS.
- Remove the meter-triangle hover tooltip so it no longer obscures the view.
- Show actual applied gain, adjustment direction, and retained RMS shortfall.
- Add double-click gain/target resets, clearer labels and keyboard focus,
  and compact Help and Presets panels.
- Make floating Peak/RMS stripes brighter and easier to see as they fall slowly.
- Include versioned PDF and offline HTML manuals in the release package.

## 0.1.0 - 2026-09-03

- Initial plugin scaffold.
- Added a compact Normalize action that sets the target to 0 dBFS and starts Match.
- Added a smoothed realtime orange input meter with a dB scale and target marker.
