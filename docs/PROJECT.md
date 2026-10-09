# GainSnap

## Summary

GainSnap starts with manual Gain at 0 dB. The round knob and right meter arrow
edit one shared Gain parameter. The knob has a thick inner grab ring, keeps
tracking drags beyond the editor, and places its editable value beneath it. Match
temporarily adjusts that same parameter to the selected Peak or RMS target.
Stopping Match freezes the gain, with no held rematch. The target entry, Match,
and rematch icons share a row below the meter. The tall meter shows output Peak
and RMS in separate colors, and the left marker sets the target.

Closing or minimizing the editor disables Match. Saved projects reopen with
Match off and the last applied gain. The open editor repaints its meters even
after an initial period of silence.

Engaging Match holds the audible gain for at least one second and requires
repeated signal evidence and a measured level stable within 0.5 dB for half a
second. Isolated hits followed by silence cannot trigger a boost. RMS also
waits for a complete window. Restarts and target changes preserve the audible
gain. Automatic gain movement is bounded to 24 dB per second in both directions.
Stopping Match holds the currently audible correction, including mid-slew.
There is no sample-peak limiter: finite signals above 0 dBFS pass through to
the host, with only non-finite input and floating-point overflow contained.
The 200 × 424 editor follows the approved Sift Hardware r5 composition: a tall
left output meter, aligned upper-right readouts and lower-right gain controls,
and a compact target/Match/Restart footer. Graphite/sage surface gradients and
edge-connected chassis inserts provide shallow depth. Its
Peak and RMS meters retain independent colored peak rules for two seconds,
then release at 1.5 dB per second while staying above the live bars. The
three-stage activity rail reports Listening, Adjusting, Matched, or Below
target while Match runs; manual and held states omit status text, while
no-signal feedback stays visible. A short
correction telemetry hold keeps Adjusting visible for about 200 ms. Both
macOS and Windows use Toybox's embedded GPUI host, including keyboard, focus,
and DPI conversion.

The clickable Peak/RMS output readouts select parameter 4 (0 = Peak, 1 = RMS). Peak mode remembers the
highest finite stereo-linked sample peak for the current Match session. A quieter
passage cannot raise the gain; a newly louder peak lowers it. Pressing Restart
beside Match clears that session maximum while leaving Match enabled. RMS uses
the strongest complete 300 ms sliding mean-square window in the entire Match
session, with the louder channel setting the shared gain. Zeroes after a hit
remain part of its measurement window. Neither Peak nor RMS evidence ages out:
quiet passages and long silence retain the gain and settled confidence. Only
stronger evidence triggers another correction. Restart or a new Match session
clears the retained maxima; target changes reuse them. Changing mode begins a
new measurement. New instances remember the last explicit Peak/RMS choice;
project loading and automation preserve each instance's own mode. The output
meter shows the latest 300 ms RMS window, using full-scale-square = 0 dBFS.
The bottom-left N button toggles Normalize: from Match off it selects Peak,
sets 0 dBFS and starts Match; clicking again stops matching.
When an RMS target needs more gain than the RMS gain cap permits,
the editor shows Below target; it does not compress or limit transients to reach
that target. The target applies to the strongest measured 300 ms
window; the live RMS meter can read lower between transients.
Peak correction supports up to +120 dB for usable signals above the -120 dBFS
silence floor; RMS correction retains its +24 dB boost cap. The fixed -36 dB
attenuation limit still applies; floating-point headroom is preserved.

State version 6 stores one applied Gain value and restores Match off. Version 5
projects migrate the gain selected by their old Auto/Manual mode; versions 1–4
use their saved matched gain. The former completed-match flag no longer arms
background monitoring.

## Constraints

- Keep audio processing realtime-safe: no allocation, blocking, or secret handling in the audio callback.
- Keep this repository thin; shared GUI/host mechanics belong in Toybox.
- Windows release artifacts are unsigned, are emitted only by GitHub Actions, and are validated as the public Windows sidecar of production nightly schema-3 manifests; they never receive Apple signing/notary credentials or enter the macOS signing path.
- Landing-page content is generated from site/landing-page.json and registered through the staged CLI.
