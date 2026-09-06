Development
===========

The repository is independent inside the AudioDev meta-workspace.

Local checks:

bash scripts/ci.sh
VST3_SDK_DIR=/path/to/vst3sdk bash scripts/ci.sh --vst3

GitHub Actions release workflows build signed macOS arm64 CLAP/VST3 artifacts
and an explicitly unsigned Windows x86_64 VST3 nightly sidecar. Stable and RC
releases remain schema-2 macOS-only releases; production nightlies publish one
schema-3 manifest containing all three artifacts. The Windows reusable workflow
has no secrets or OIDC permission.

For local-only Toybox iteration, use an untracked .cargo/config.toml path patch. Do not commit path patches or branch pins.
