# Changelog

## Unreleased

- Keep coarse knob drags active outside the editor, add a thicker inner grab
  ring, and move the editable gain value below the knob.
- Hide the Manual activity label when Match is off.
- Remove the OUTPUT heading and span the three-stage activity rail across the
  bottom of the editor.
- Put numeric target entry, icon Match, and rematch together below the meter;
  select Peak or RMS from the output readouts and remove Normalize from the UI.
- Remove the coarse gain caption and place the dial below the controls.
- Show Below target when RMS peak headroom or the gain range prevents reaching
  the target while preserving transient shape; default new instances to RMS.
- Group output levels and match status at the top of the editor, place the
  coarse gain dial and action buttons at the bottom, and show the dial position.
- Start the visible meters when the editor opens; keep the fine gain slider
  independent from the moving meter and allow direct numeric gain entry.
- Keep gain manual except while Match is active, then retain the matched gain
  when Match stops; remove background rematching.
- Add a local macOS nightly path with Developer ID signing, Apple notarization,
  stapled CLAP/VST3 bundles, and direct PortalSurfer publication.
- Change RMS Match to retain the strongest complete 300 ms sliding RMS window,
  preventing beat gaps and quieter passages from increasing gain; bound requested
  RMS gain by observed sample-peak headroom before the output safety guard.
- Add a saved, automatable PEAK/RMS mode toggle with RMS-based live matching and output metering.

- Fade playing audio down briefly before starting Match, avoiding the immediate mute that caused clicks; preserve the audible gain through rapid restarts and target edits.
- Improve gain-smoothing precision at high sample rates.
- Enable the native Windows editor with shared keyboard, focus, and DPI support; validate its host lifecycle in Windows CI.

- Apply peak matching live and display the protected output level.
- Fade in after output protection to contain strong startup bursts.
- Compact the editor and pulse Match and its status indicator together.
- Publish Windows x86_64 VST3 nightlies alongside signed macOS CLAP and VST3.

## 0.1.0 - 2026-09-03

- Initial plugin scaffold.
- Added a compact Normalize action that sets the target to 0 dBFS and starts Match.
- Added a smoothed realtime orange input meter with a dB scale and target marker.
