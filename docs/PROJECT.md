# GainSnap

## Summary

While Match is enabled, GainSnap measures an incoming track peak and continuously
applies the gain needed to reach a chosen target. Disabling Match holds that
gain. Normalize sets the target to 0 dBFS and
starts Match in one click. The vertical meter shows a smoothed output peak
in realtime as one thick orange column, with a small dB scale at its left edge.
A small triangle beside the meter marks the target and acts as the target
control.

Match fades playing audio down over 10 ms before a 300 ms fade up. Silent
starts wait for usable signal. Restarts preserve the currently audible gain. Gain increases
use a slower 100 ms response than the 10 ms reductions. A stereo-linked sample
peak guard retains the previous ceiling during the outgoing fade, then caps
Peak-mode matching output at the target and held output at 0 dBFS, with immediate attack and 100 ms recovery. The fade continues if Match
is disabled early. Output metering includes this protection.
The startup fade follows the peak guard, bounding strong bursts throughout the
fade. The 208 × 212 editor groups its controls tightly; the Match button and
status dot share a pulse while Match is enabled. Both macOS and Windows use
Toybox's embedded GPUI host, including keyboard, focus, and DPI conversion.

The PEAK/RMS button is parameter 4 (0 = Peak, 1 = RMS). RMS uses the strongest
complete 300 ms sliding mean-square window in roughly the most recent three
seconds, with the louder channel setting the shared gain. It initially uses peak
correction until a complete window exists; zeroes after a hit remain part of that
window. Peak and RMS evidence age out together. A lower rolling estimate is
published only after recent usable evidence stays within 0.5 dB for about 300 ms,
while newly louder evidence may update promptly. For either mode, a user who
leaves Match enabled while changing the upstream sound should allow roughly 3–4
seconds of steady material for the recent history and settling check to respond.
After two seconds of silence,
returning audio restarts the protected fade. Requested RMS gain is bounded by the
largest recent sample peak's 0 dBFS headroom; the sample peak guard remains the
final safety net. The output meter shows the latest 300 ms sliding RMS window,
using the same full-scale-square = 0 dBFS convention. Normalize explicitly
selects Peak before setting 0 dBFS and engaging Match.

State version 3 stores mode in previously reserved payload byte 5. Version 1
and 2 projects load in Peak mode, preserving the existing Match migration rules.

## Constraints

- Keep audio processing realtime-safe: no allocation, blocking, or secret handling in the audio callback.
- Keep this repository thin; shared GUI/host mechanics belong in Toybox.
- Windows release artifacts are unsigned, are emitted only by GitHub Actions, and are validated as the public Windows sidecar of production nightly schema-3 manifests; they never receive Apple signing/notary credentials or enter the macOS signing path.
- Landing-page content is generated from site/landing-page.json and registered through the staged CLI.
