# Changelog

## Unreleased

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
