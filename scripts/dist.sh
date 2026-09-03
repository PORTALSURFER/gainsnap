#!/usr/bin/env bash
# Local, unsigned AudioDev distribution/test producer.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"
slug="gainsnap"
display_name="GainSnap"
output_dir="${AUDIODEV_DIST_DIR:-${repo_root}/dist}"
format="all"
run_checks=true

usage() {
  cat <<EOF
Usage: scripts/dist.sh [--format clap|vst3|all] [--output-dir PATH] [--skip-checks]

Builds unsigned local macOS bundles for host testing under dist/ (or the
explicit output directory). This command never contacts GitHub, Apple, or
PortalSurfer and never consumes production signing credentials.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --format) format="${2:?missing format}"; shift 2 ;;
    --output-dir) output_dir="${2:?missing output directory}"; shift 2 ;;
    --skip-checks) run_checks=false; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ "${format}" == clap || "${format}" == vst3 || "${format}" == all ]] || {
  echo "format must be clap, vst3, or all" >&2
  exit 2
}
[[ "$(uname -s)" == Darwin ]] || {
  echo "local bundle packaging requires macOS" >&2
  exit 1
}
if [[ "${format}" == vst3 || "${format}" == all ]]; then
  : "${VST3_SDK_DIR:?VST3_SDK_DIR must point to a VST3 SDK checkout}"
  [[ -d "${VST3_SDK_DIR}/pluginterfaces" ]] || {
    echo "VST3_SDK_DIR must contain pluginterfaces/" >&2
    exit 1
  }
fi
if [[ "${run_checks}" == true ]]; then
  bash scripts/ci.sh
  if [[ "${format}" == vst3 || "${format}" == all ]]; then
    VST3_SDK_DIR="${VST3_SDK_DIR}" bash scripts/ci.sh --vst3
  fi
fi

version="$(sed -n '/^\[package\]/,/^\[/ { s/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p; }' Cargo.toml | head -n 1)"
[[ -n "${version}" ]] || { echo "Cargo.toml package version is missing" >&2; exit 1; }
mkdir -p "${output_dir}"
lib_name="${slug//-/_}"

validate_binary() {
  local binary="$1" kind="$2"
  [[ -f "${binary}" ]] || { echo "missing ${kind} build output: ${binary}" >&2; exit 1; }
  /usr/bin/file "${binary}" | grep -q arm64 || {
    echo "${kind} local build must contain arm64" >&2
    exit 1
  }
  if [[ "${kind}" == clap ]]; then
    /usr/bin/nm -gU "${binary}" | grep -q _clap_entry || {
      echo "CLAP entrypoint missing from ${binary}" >&2
      exit 1
    }
  else
    for symbol in _GetPluginFactory _bundleEntry _bundleExit; do
      /usr/bin/nm -gU "${binary}" | grep -q "${symbol}" || {
        echo "VST3 symbol ${symbol} missing from ${binary}" >&2
        exit 1
      }
    done
  fi
}

package_bundle() {
  local kind="$1"
  local binary="$2"
  local bundle="${output_dir}/${slug}-v${version}-macos.${kind}"
  [[ ! -e "${bundle}" ]] || {
    echo "refusing to overwrite existing local bundle: ${bundle}" >&2
    exit 1
  }
  mkdir -p "${bundle}/Contents/MacOS"
  cp "${binary}" "${bundle}/Contents/MacOS/${slug}"
  chmod 755 "${bundle}/Contents/MacOS/${slug}"
  cat > "${bundle}/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleDisplayName</key><string>${display_name}</string>
<key>CFBundleExecutable</key><string>${slug}</string>
<key>CFBundleIdentifier</key><string>com.portalsurfer.${slug}.${kind}</string>
<key>CFBundleName</key><string>${display_name}</string>
<key>CFBundlePackageType</key><string>BNDL</string>
<key>CFBundleShortVersionString</key><string>${version}</string>
<key>CFBundleVersion</key><string>${version}</string>
<key>CFBundleSupportedPlatforms</key><array><string>MacOSX</string></array>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF
  printf 'BNDL????' > "${bundle}/Contents/PkgInfo"
  /usr/bin/plutil -lint "${bundle}/Contents/Info.plist" >/dev/null
  codesign --force --deep --sign - "${bundle}" >/dev/null
  codesign --verify --deep --strict "${bundle}"
  echo "wrote ${bundle}"
  /usr/bin/shasum -a 256 "${bundle}/Contents/MacOS/${slug}"
}

if [[ "${format}" == clap || "${format}" == all ]]; then
  cargo build --release
  clap_binary="target/release/lib${lib_name}.dylib"
  validate_binary "${clap_binary}" clap
  package_bundle clap "${clap_binary}"
fi

if [[ "${format}" == vst3 || "${format}" == all ]]; then
  VST3_SDK_DIR="${VST3_SDK_DIR}" \
    cargo rustc --release --features vst3 --lib -- -C link-arg=-Wl,-bundle
  vst3_binary="target/release/lib${lib_name}.dylib"
  validate_binary "${vst3_binary}" vst3
  package_bundle vst3 "${vst3_binary}"
fi
