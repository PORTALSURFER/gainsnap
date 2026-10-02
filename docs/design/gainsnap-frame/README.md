# GainSnap Frame redesign

Editable design preview using the current local Frame source (844f3b4) and its
Technical Futurist styleguide. The runtime is a self-contained snapshot so this
preview does not change the active Frame checkout.

Open `runtime/index.html` or serve this folder and open `/runtime/`. The current
local preview is at http://127.0.0.1:8769/runtime/.

- `gainsnap.js`: structured 240 × 424 design with stable element IDs.
- `gainsnap.frame.json`: portable Frame project, including the design revision archive.
- `gainsnap-preview.png`: clean exported design.
- `frame-preview.png`: preview inside Frame, with editable annotation tools.

Use Pen or Note for persistent red-line feedback; Download project preserves
annotations and revisions. Interact enables Peak/RMS selection, numeric entry,
Match and Restart preview actions. The state picker shows listening, matched,
silence and held states. Meter data is illustrative. The dial and target marker
are visual design elements; neither implements dragging in this prototype.
Mode selection and Match are independent visual scenarios, not a combined
simulation of the plug-in. The plug-in UI, DSP and installed artifacts were not
changed by this design pass.

Validation: Frame structural checks pass at the intended 240 × 424 viewport for
all six states. Peak/RMS selection, target entry and Match actions were exercised
in the in-app browser. No DAW acceptance or audio behavior is claimed.

The current design uses the updated Frame guide: top-left PORTALSURFER / GAINSNAP
branding, tiny top-right version, R7 graphite surfaces without background texture,
red/orange telemetry, metallic edge insets and selectively clipped action buttons.
The guide reserves mint/cyan for exceptional focus or secondary signals.

Mint is now applied only to the actually focused prototype control in Interact
mode. Top and bottom metal insets use filled chamfered silhouettes.

Design size restores original 240 × 424 dimensions after viewport presets.
The Frame status footer is a 24 px strip; the design pane uses remaining space.
