#!/usr/bin/env bash
# AudioDev release producer template v1; expanded by scripts/audiodev-plugin init.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"
slug="gainsnap"
endpoint="https://portalsurfer.org"
screenshot_width=300
screenshot_height=320
screenshot_name="${slug}-default-${screenshot_width}x${screenshot_height}.png"
mode=""
channel="stable"
usage() { cat <<EOF
Usage: scripts/release.sh --package-only|--publish --channel stable|rc|nightly
Production contract: macOS arm64; exactly CLAP+VST3; Developer ID Application
signing; notarization/stapling; extracted ZIP audits; fresh ${screenshot_width}x${screenshot_height} screenshot
tied to exact source SHA; non-empty changelog; canonical manifest-v2; hashes and
sizes; staged uploads followed by one atomic commit. Placeholder UI fails.
EOF
}
while [[ $# -gt 0 ]]; do
  case "$1" in
    --package-only|--publish) [[ -z "${mode}" ]] || { echo "choose one release mode" >&2; exit 2; }; mode="${1#--}"; shift ;;
    --channel) channel="${2:?missing channel}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done
[[ -n "${mode}" ]] || { usage >&2; exit 2; }
[[ "${channel}" == stable || "${channel}" == rc || "${channel}" == nightly ]] || { echo "invalid channel" >&2; exit 2; }
[[ "$(uname -s)" == Darwin ]] || { echo "production packaging requires macOS" >&2; exit 1; }

branch="$(git symbolic-ref --quiet --short HEAD || true)"
[[ "${branch}" == main ]] || { echo "release source must be a non-detached main checkout" >&2; exit 1; }
[[ -z "$(git status --porcelain --untracked-files=all)" ]] || { echo "release source must be clean" >&2; exit 1; }
git fetch origin main --quiet
source_sha="$(git rev-parse HEAD)"
[[ "${source_sha}" == "$(git rev-parse refs/remotes/origin/main)" ]] || { echo "release source must equal origin/main" >&2; exit 1; }
version="$(sed -n '/^\[package\]/,/^\[/ { s/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p; }' Cargo.toml | head -n 1)"
[[ -n "${version}" ]] || { echo "Cargo.toml package version is missing" >&2; exit 1; }
if [[ "${channel}" == stable && ! "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "stable release requires a stable SemVer without prerelease/build metadata" >&2; exit 1
fi
if [[ "${channel}" == rc && ! "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+-rc\.[1-9][0-9]*$ ]]; then
  echo "RC release requires X.Y.Z-rc.N" >&2; exit 1
fi
build_id="${slug}-v${version}-${source_sha:0:12}"
[[ -s CHANGELOG.md ]] || { echo "CHANGELOG.md must not be empty" >&2; exit 1; }

# Explicit environment-only production credentials. Values are never printed.
[[ -n "${APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64:-}" ]] || { echo "missing Apple certificate" >&2; exit 1; }
[[ -n "${APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD:-}" ]] || { echo "missing Apple certificate password" >&2; exit 1; }
[[ -n "${APPLE_NOTARY_KEY_BASE64:-}" && -n "${APPLE_NOTARY_KEY_ID:-}" && -n "${APPLE_NOTARY_ISSUER_ID:-}" ]] || { echo "missing Apple notarization credentials" >&2; exit 1; }
if [[ "${mode}" == publish ]]; then
  [[ -n "${PORTALSURFER_RELEASE_TOKEN:-}" ]] || { echo "--publish requires PORTALSURFER_RELEASE_TOKEN" >&2; exit 1; }
  [[ "${endpoint}" == "https://portalsurfer.org" ]] || { echo "production origin mismatch" >&2; exit 1; }
fi
[[ -d "${VST3_SDK_DIR:-}" ]] || { echo "VST3_SDK_DIR must point to a pinned SDK checkout" >&2; exit 1; }
if grep -R -n -E 'TODO: build UI|pub fn placeholder\(\)' src >/dev/null 2>&1; then
  echo "release readiness failed: replace the init placeholder UI/DSP first" >&2
  exit 1
fi

release_parent="${repo_root}/dist/releases"
mkdir -p "${release_parent}" "${repo_root}/target"
tmp_root="$(mktemp -d "${repo_root}/target/release-build.XXXXXX")"
staged="${tmp_root}/${build_id}"
mkdir -p "${staged}"
original_keychains=()
cleanup() {
  if [[ -f "${original_keychains_file:-}" && "${#original_keychains[@]}" -gt 0 ]]; then
    security list-keychains -d user -s "${original_keychains[@]}" >/dev/null 2>&1 || true
  fi
  [[ -z "${keychain:-}" ]] || security delete-keychain "${keychain}" >/dev/null 2>&1 || true
  rm -rf "${tmp_root}"
}
trap cleanup EXIT
decode_base64() { printf '%s' "$1" | base64 --decode > "$2" 2>/dev/null || printf '%s' "$1" | base64 -D > "$2"; }
cert_path="${tmp_root}/developer-id-application.p12"
notary_key_path="${tmp_root}/AuthKey_${APPLE_NOTARY_KEY_ID}.p8"
decode_base64 "${APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64}" "${cert_path}"
decode_base64 "${APPLE_NOTARY_KEY_BASE64}" "${notary_key_path}"
chmod 600 "${cert_path}" "${notary_key_path}"
keychain="${tmp_root}/release.keychain-db"
keychain_password="$(uuidgen | tr -d '-')"
original_keychains_file="${tmp_root}/original-keychains.txt"
security list-keychains -d user | sed 's/[[:space:]]*"//g; s/"$//' > "${original_keychains_file}"
original_keychains=()
while IFS= read -r item; do [[ -n "${item}" ]] && original_keychains+=("${item}"); done < "${original_keychains_file}"
security create-keychain -p "${keychain_password}" "${keychain}" >/dev/null
security set-keychain-settings -lut 21600 "${keychain}"
security unlock-keychain -p "${keychain_password}" "${keychain}"
security list-keychains -d user -s "${keychain}" "${original_keychains[@]}" >/dev/null
security import "${cert_path}" -P "${APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD}" -A -t cert -f pkcs12 -k "${keychain}" >/dev/null
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "${keychain_password}" "${keychain}" >/dev/null
identity="${APPLE_CODESIGN_IDENTITY:-$(security find-identity -v -p codesigning "${keychain}" | sed -n 's/.*\"\(Developer ID Application:.*\)\".*/\1/p' | head -n 1)}"
[[ "${identity}" == Developer\ ID\ Application:* ]] || { echo "no Developer ID Application identity found" >&2; exit 1; }

rm -rf target/ui-screenshots
bash scripts/ci.sh --screenshots
screenshot_count="$(find target/ui-screenshots -type f -name "*${screenshot_width}x${screenshot_height}*.png" -print | wc -l | tr -d ' ')"
[[ "${screenshot_count}" == 1 ]] || { echo "release requires exactly one fresh ${screenshot_width}x${screenshot_height} screenshot" >&2; exit 1; }
screenshot="$(find target/ui-screenshots -type f -name "*${screenshot_width}x${screenshot_height}*.png" -print | head -n 1)"
cp "${screenshot}" "${staged}/${screenshot_name}"

build_bundle() {
  local format="$1"
  local binary="$2"
  local bundle="${tmp_root}/${slug}.${format}"
  local archive="${staged}/${slug}-v${version}-macos.${format}.zip"
  local contents="${bundle}/Contents"
  mkdir -p "${contents}/MacOS"
  cp "${binary}" "${contents}/MacOS/${slug}"
  chmod 755 "${contents}/MacOS/${slug}"
  cat > "${contents}/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleExecutable</key><string>${slug}</string><key>CFBundleIdentifier</key><string>com.portalsurfer.${slug}.${format}</string><key>CFBundleName</key><string>GainSnap</string><key>CFBundlePackageType</key><string>BNDL</string><key>CFBundleShortVersionString</key><string>${version}</string><key>CFBundleVersion</key><string>${version}</string></dict></plist>
EOF
  printf 'BNDL????' > "${contents}/PkgInfo"
  /usr/bin/plutil -lint "${contents}/Info.plist" >/dev/null
  codesign --force --deep --timestamp --options runtime --keychain "${keychain}" --sign "${identity}" "${bundle}" >/dev/null
  codesign --verify --deep --strict "${bundle}"
  /usr/bin/ditto -c -k --sequesterRsrc --keepParent "${bundle}" "${tmp_root}/notary-${format}.zip"
  notary_json="${tmp_root}/notary-${format}.json"
  xcrun notarytool submit "${tmp_root}/notary-${format}.zip" --key "${notary_key_path}" --key-id "${APPLE_NOTARY_KEY_ID}" --issuer "${APPLE_NOTARY_ISSUER_ID}" --wait --output-format json > "${notary_json}"
  notary_status="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["status"])' "${notary_json}")"
  notary_id="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "${notary_json}")"
  [[ "${notary_status}" == Accepted ]] || { echo "${format} notarization was not accepted" >&2; exit 1; }
  notary_log="${tmp_root}/notary-${format}-${notary_id}.json"
  xcrun notarytool log "${notary_id}" --key "${notary_key_path}" --key-id "${APPLE_NOTARY_KEY_ID}" --issuer "${APPLE_NOTARY_ISSUER_ID}" --output-format json > "${notary_log}"
  python3 - "${notary_log}" "${format}" <<'PY'
import json, sys
payload = json.load(open(sys.argv[1], encoding="utf-8"))
for issue in payload.get("issues") or []:
    severity = str(issue.get("severity", "")).lower()
    message = str(issue.get("message", "")).strip() or "unspecified issue"
    if severity == "error":
        raise SystemExit(f"notarization error ({sys.argv[2]}): {message}")
    if severity == "warning":
        print(f"notarization warning ({sys.argv[2]}): {message}", file=sys.stderr)
PY
  xcrun stapler staple "${bundle}" >/dev/null
  xcrun stapler validate "${bundle}" >/dev/null
  codesign -vvvv -R=notarized --check-notarization "${bundle}" >/dev/null
  file "${contents}/MacOS/${slug}" | grep -q arm64
  [[ "$(lipo -archs "${contents}/MacOS/${slug}")" == arm64 ]] || { echo "${format} must be arm64-only" >&2; exit 1; }
  /usr/bin/ditto -c -k --sequesterRsrc --keepParent "${bundle}" "${archive}"
  printf '%s' "${notary_id}"
}

lib_name="${slug//-/_}"
clap_target="${tmp_root}/clap-target"
vst3_target="${tmp_root}/vst3-target"
TOYBOX_ACTIVE_ARTIFACT=clap CARGO_TARGET_DIR="${clap_target}" cargo build --release
clap_notary_id="$(build_bundle clap "${clap_target}/release/lib${lib_name}.dylib")"
TOYBOX_ACTIVE_ARTIFACT=vst3 VST3_SDK_DIR="${VST3_SDK_DIR}" CARGO_TARGET_DIR="${vst3_target}" cargo rustc --release --features vst3 -- -C link-arg=-Wl,-bundle
vst3_notary_id="$(build_bundle vst3 "${vst3_target}/release/lib${lib_name}.dylib")"

# Re-extract and audit both ZIPs before any staged upload.
audit_zip() {
  local format="$1"
  local audit="${tmp_root}/audit-${format}"
  local bundle
  mkdir -p "${audit}"
  /usr/bin/ditto -x -k "${staged}/${slug}-v${version}-macos.${format}.zip" "${audit}"
  bundle="${audit}/${slug}.${format}"
  python3 - "${audit}" "${slug}" "${format}" <<'PY'
import os, pathlib, sys
root = pathlib.Path(sys.argv[1]); slug = sys.argv[2]; fmt = sys.argv[3]
bundle = root / f"{slug}.{fmt}"
allowed = {bundle, bundle / "Contents", bundle / "Contents" / "Info.plist", bundle / "Contents" / "PkgInfo", bundle / "Contents" / "MacOS", bundle / "Contents" / "MacOS" / slug, bundle / "Contents" / "_CodeSignature", bundle / "Contents" / "_CodeSignature" / "CodeResources"}
for current, directories, files in os.walk(root, followlinks=False):
    current_path = pathlib.Path(current)
    if current_path.is_symlink():
        raise SystemExit("release ZIP contains a symlink")
    for name in directories + files:
        child = current_path / name
        if child.is_symlink() or child not in allowed:
            raise SystemExit(f"release ZIP contains unexpected topology: {child.relative_to(root)}")
PY
  test -x "${bundle}/Contents/MacOS/${slug}"
  /usr/bin/plutil -lint "${bundle}/Contents/Info.plist" >/dev/null
  [[ "$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "${bundle}/Contents/Info.plist")" == "com.portalsurfer.${slug}.${format}" ]] || { echo "${format} ZIP bundle identifier is invalid" >&2; exit 1; }
  [[ "$(/usr/bin/plutil -extract CFBundlePackageType raw -o - "${bundle}/Contents/Info.plist")" == BNDL ]] || { echo "${format} ZIP package type is invalid" >&2; exit 1; }
  codesign --verify --deep --strict "${bundle}"
  xcrun stapler validate "${bundle}" >/dev/null
  codesign -vvvv -R=notarized --check-notarization "${bundle}" >/dev/null
  codesign -dv --verbose=4 "${bundle}" 2>&1 | grep -q '^Authority=Developer ID Application:'
  codesign -dv --verbose=4 "${bundle}" 2>&1 | grep -q "^TeamIdentifier=${signing_team_id}$"
  file "${bundle}/Contents/MacOS/${slug}" | grep -q arm64
  [[ "$(lipo -archs "${bundle}/Contents/MacOS/${slug}")" == arm64 ]] || { echo "${format} ZIP binary must be arm64-only" >&2; exit 1; }
  if [[ "${format}" == clap ]]; then
    /usr/bin/nm -gU "${bundle}/Contents/MacOS/${slug}" | grep -q _clap_entry || { echo "CLAP ZIP entrypoint missing" >&2; exit 1; }
  else
    for symbol in _GetPluginFactory _bundleEntry _bundleExit; do
      /usr/bin/nm -gU "${bundle}/Contents/MacOS/${slug}" | grep -q "${symbol}" || { echo "VST3 ZIP symbol ${symbol} missing" >&2; exit 1; }
    done
  fi
}
signing_team_id="$(codesign -dv --verbose=4 "${tmp_root}/${slug}.clap" 2>&1 | sed -n 's/^TeamIdentifier=//p' | head -n 1)"
[[ "${signing_team_id}" =~ ^[A-Z0-9]{10}$ ]] || { echo "could not capture Developer ID team identifier" >&2; exit 1; }
audit_zip clap
audit_zip vst3
cp CHANGELOG.md "${staged}/CHANGELOG.md"
python3 - "${staged}" "${version}" "${build_id}" "${channel}" "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "${source_sha}" "${signing_team_id}" "${clap_notary_id}" "${vst3_notary_id}" <<'PY'
import pathlib, sys
folder = pathlib.Path(sys.argv[1]); sys.path.insert(0, str(folder.parents[2] / "scripts"))
from release_helper import build_manifest, canonical_json, validate_manifest
out, version, build_id, channel, released_at, source_sha, team_id, clap_id, vst3_id = sys.argv[1:]
folder = pathlib.Path(out)
manifest = build_manifest(product="gainsnap", repository="PORTALSURFER/gainsnap", version=version, build_id=build_id, channel=channel,
    released_at=released_at, git_sha=source_sha, clap=folder / f"gainsnap-v{version}-macos.clap.zip", vst3=folder / f"gainsnap-v{version}-macos.vst3.zip",
    screenshot=folder / "gainsnap-default-300x320.png", changelog=folder / "CHANGELOG.md", signing_team_id=team_id, clap_notary_id=clap_id, vst3_notary_id=vst3_id)
(folder / "release-manifest.json").write_bytes(canonical_json(manifest))
validate_manifest(manifest, folder)
PY

final_dir="${release_parent}/${build_id}"
[[ ! -e "${final_dir}" ]] || { echo "release already exists locally: ${final_dir}" >&2; exit 1; }
mv "${staged}" "${final_dir}"
if [[ "${mode}" == publish ]]; then
  python3 - "${final_dir}" "${endpoint}" <<'PY'
import json, os, pathlib, sys
root = pathlib.Path(sys.argv[1]); sys.path.insert(0, str(root.parents[1].parent / "scripts"))
from release_helper import publish_release
publish_release(endpoint=sys.argv[2], token=os.environ.get("PORTALSURFER_RELEASE_TOKEN", ""), manifest=json.loads((root / "release-manifest.json").read_text()), root=root, repo_root=root.parents[2])
PY
fi
echo "[release] ${mode} bundle ready: ${final_dir}"
