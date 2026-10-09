---
title: GainSnap Manual
subtitle: Match and hold gain in your DAW
author: PORTALSURFER
lang: en
---

GainSnap matches a track's Peak or RMS level to a target, then lets you hold the
resulting gain. It applies smooth, linear gain without clipping or limiting
signals at 0 dBFS. The macOS release is a VST3 for Apple Silicon Macs.
GainSnap is **pay what you want, including €0**. Donations are optional;
it needs no account or license activation.

# Install and update

1. Quit your DAW before installing or replacing the plug-in.
2. Copy `GainSnap.vst3` from the release ZIP into
   `~/Library/Audio/Plug-Ins/VST3/`. In Finder, choose **Go > Go to Folder** and
   paste that path. Create the folder if it does not exist.
3. Remove duplicate older GainSnap bundles from other VST3 folders.
4. Open your DAW, rescan VST3 plug-ins if needed, and load GainSnap on an audio track.

The release bundle is signed and notarized for macOS. It contains the Apple
Silicon build only; this release does not include Intel Mac or Windows binaries.
Keep your Live Set saved before replacing a plug-in. Updating retains the saved
mode, target and gain; it does not start a new Match pass automatically.

# Match a track

1. Choose **Peak** or **RMS** using the output readouts.
2. Set a target using the left meter triangle or the number below the meter.
   You can also use Normalize.
3. Turn on **Match** and play the loudest relevant section of the track.
4. Wait for the correction to settle, then turn Match off to hold that gain.

Match holds the current gain for at least one second and waits for repeated
signal evidence and a measured level stable within 0.5 dB for half a second.
An isolated hit followed by silence does not establish confidence. RMS also
waits for a complete measurement window. Automatic gain changes slew at no
more than 24 dB per second in either direction. Turning Match off holds the
currently audible gain, even during a correction. Leave Match
on while stronger passages play: newly stronger evidence can reduce the gain.
Quiet passages and silence do not erase the session measurement or raise the
correction. **Restart** begins a fresh measurement. Changing the target reuses
retained evidence; switching Peak/RMS starts a new measurement.

![GainSnap actively listening to incoming audio](assets/editor.png){width=200px}

# Peak and RMS

Your last explicit Peak/RMS selection becomes the default for new GainSnap
instances, including after restarting your DAW. Saved projects restore each
instance's own mode. Project loading and automation do not change your saved
default. If no preference has been saved, new instances start in RMS.

**Peak** uses the highest stereo sample peak encountered during the Match
session. It links both channels to one gain, preserving their balance. Use it
for transient material or to normalize a sample's peak.

**RMS** uses the strongest complete 300 ms sliding mean-square window of the
louder channel. The live RMS meter shows the latest window, so it can fall
between hits even though the correction remains settled. A full-scale square
wave reads 0 dBFS RMS; a full-scale sine reads about −3.01 dBFS RMS.
These are dBFS measurements, not LUFS loudness targets.

Peak correction ranges from −36 to +120 dB. RMS correction retains a +24 dB
boost cap. Signals at or below −120 dBFS are treated as silence. Gain range
limits can prevent matching the selected target exactly.

# Normalize

The **N** button to the left of the bottom target field toggles Normalize.
When matching is off, it selects Peak mode, sets the target to 0 dBFS, and starts
Match. Clicking it while matching is active stops Match and holds the resulting
gain. Peak also becomes the remembered mode for new instances.

# Controls and feedback

The orange bar shows output Peak; the purple bar shows output RMS. Each lane
has a floating stripe that holds its highest level for two seconds, then falls
at 1.5 dB per second without dropping below the live bar.

The **left triangle** sets the target in dBFS. The **right triangle** sets gain
in dB on its own scale. Its −36 dB minimum sits at the meter bottom, and unity
gain sits beside the meter's −12 dB mark. Its position does not represent the
output level. The knob adjusts the same gain. Editing either gain control
turns Match off.

The matching indicator shows Listening, Adjusting, or Matched. The gain knob
and its number field show the gain correction. Orange Match pulses softly
while listening or adjusting. **Below
Target** and the shortfall in dB mean the gain range prevents reaching the RMS
target. The shortfall uses the retained measurement, not a quieter current beat.

# Editing and shortcuts

- Drag either triangle or the knob. Drags continue beyond the editor until you
  release the mouse button.
- Drag a numeric field up or down, or click it and type a value. Press Enter to
  confirm numeric entry. Hold Shift for finer numeric-field dragging.
- Focus a control and use arrow keys for steps; Shift gives smaller keyboard steps.
  Shift while dragging a triangle or knob snaps to whole dB steps.
- Double-click a gain control to reset to 0 dB and stop Match. Double-click a
  target control to reset to −12 dBFS.
- Space passes through to the DAW transport. Enter activates a focused button.
- Open **?** for compact help. Escape closes Help.

# Floating-point headroom

GainSnap does not hard-clip at 0 dBFS, enforce a Peak target as an instantaneous
ceiling, or limit RMS transients. Newly louder samples pass through the smooth
gain while Match adapts. Manual and held gain can also produce output above
0 dBFS. Only non-finite samples and numeric overflow at the floating-point
range boundary are contained.

Your DAW and later effects receive the louder signal. Downstream processors
may distort or clip, and final hardware or fixed-point export needs appropriate
output headroom. Reduce gain later in the chain when necessary.

# Troubleshooting

**The plug-in is missing:** confirm the VST3 folder, remove duplicate copies,
restart your DAW and rescan. Use the Apple Silicon build on an Apple Silicon Mac.

**Match keeps listening:** play the loudest relevant section for at least one
second, with repeated signal evidence and a stable measurement. A lone hit
followed by silence does not provide enough confidence. Loop short material
while matching if necessary.

**The meter falls below the target between hits:** this is expected. GainSnap
retains the strongest evidence rather than raising gain in quieter gaps.

**The sound still distorts above 0 dBFS:** inspect later effects and the final
output. GainSnap passes over-full-scale audio rather than limiting it.

Download updates from the [GainSnap product page](https://portalsurfer.gumroad.com/l/gainsnap).
See the included changelog for changes in this release.
