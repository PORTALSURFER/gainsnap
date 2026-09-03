Development
===========

The repository is independent inside the AudioDev meta-workspace.

Local checks:

bash scripts/ci.sh
VST3_SDK_DIR=/path/to/vst3sdk bash scripts/ci.sh --vst3

GitHub Actions release workflows build the declared formats for macOS arm64 and
x86_64 Windows. Windows archives are explicitly unsigned and are uploaded as
short-lived Actions artifacts only; the macOS producer and PortalSurfer
publication contract remain unchanged.

For local-only Toybox iteration, use an untracked .cargo/config.toml path patch. Do not commit path patches or branch pins.
