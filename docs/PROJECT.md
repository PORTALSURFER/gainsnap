# GainSnap

## Summary

GainSnap starts with manual Gain at 0 dB. The round knob and right meter arrow
edit one shared Gain parameter; the knob value accepts direct text entry. Match
temporarily adjusts that same parameter to the selected Peak or RMS target.
Stopping Match freezes the gain, with no held rematch. The target entry, Match,
and rematch icons share a row below the meter. The tall meter shows output Peak
and RMS in separate colors, and the left marker sets the target.

Closing or minimizing the editor disables Match. Saved projects reopen with
Match off and the last applied gain. The open editor repaints its meters even
after an initial period of silence.

Match fades playing audio down over 10 ms before a 300 ms fade up. Silent
starts wait for usable signal. Restarts preserve the currently audible gain. Gain increases
use a slower 100 ms response than the 10 ms reductions. A stereo-linked sample
peak guard retains the previous ceiling during the outgoing fade, then caps
Peak-mode matching at the target, with immediate attack and 100 ms recovery. The fade continues if Match
is disabled early. Output metering includes this protection.
The startup fade follows the peak guard, bounding strong bursts throughout the
fade. The 250 × 424 editor has a full-width three-stage activity rail at the bottom
that reports Listening, Adjusting, Matched, or Below target while Match runs and Manual after
it stops. A short correction telemetry
hold keeps Adjusting visible for about 200 ms. Both macOS and Windows use
Toybox's embedded GPUI host, including keyboard, focus, and DPI conversion.

The clickable Peak/RMS output readouts select parameter 4 (0 = Peak, 1 = RMS). Peak mode remembers the
highest finite stereo-linked sample peak for the current Match session. A quieter
passage cannot raise the gain; a newly louder peak lowers it. Pressing Restart
beside Match clears that session maximum while leaving Match enabled. RMS uses
the strongest complete 300 ms sliding mean-square window in roughly the most
recent three seconds, with the louder channel setting the shared gain. It
initially uses peak correction until a complete window exists; zeroes after a
hit remain part of that window. RMS evidence ages out, and a lower rolling
estimate is published only after recent usable evidence stays within 0.5 dB for
about 300 ms, while newly louder evidence may update promptly. RMS may take
roughly 3–4 seconds of steady material to respond to an upstream level change.
After two seconds of silence,
returning audio restarts the protected fade. Requested RMS gain is bounded by the
largest recent sample peak's 0 dBFS headroom; the sample peak guard remains the
final safety net. The output meter shows the latest 300 ms sliding RMS window,
using the same full-scale-square = 0 dBFS convention. For peak normalization,
select Peak, set the target to 0 dBFS, and engage Match.
When an RMS target needs more gain than peak headroom or the RMS gain cap permits,
the editor shows Below target; it does not compress or limit transients to reach
that target. The target applies to the strongest measured 300 ms
window; the live RMS meter can read lower between transients.
Peak correction supports up to +120 dB for usable signals above the -120 dBFS
silence floor; RMS correction retains its +24 dB boost cap. The fixed -36 dB
attenuation limit and the existing sample-peak guard still apply.

State version 6 stores one applied Gain value and restores Match off. Version 5
projects migrate the gain selected by their old Auto/Manual mode; versions 1–4
use their saved matched gain. The former completed-match flag no longer arms
background monitoring.

## Constraints

- Keep audio processing realtime-safe: no allocation, blocking, or secret handling in the audio callback.
- Keep this repository thin; shared GUI/host mechanics belong in Toybox.
- Windows release artifacts are unsigned, are emitted only by GitHub Actions, and are validated as the public Windows sidecar of production nightly schema-3 manifests; they never receive Apple signing/notary credentials or enter the macOS signing path.
- Landing-page content is generated from site/landing-page.json and registered through the staged CLI.
