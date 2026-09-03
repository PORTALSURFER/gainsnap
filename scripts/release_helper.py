#!/usr/bin/env python3
"""Versioned manifest-v2 producer helpers for an AudioDev plug-in.

This file is copied into each plug-in repository by ``audiodev-plugin init``.
It deliberately has no third-party dependencies and never accepts GitHub
release asset arguments: public binaries are committed only to PortalSurfer's
staged release endpoint.
"""

from __future__ import annotations

import hashlib
import json
import re
import sys
import datetime as dt
import struct
import subprocess
import zlib
from pathlib import Path
from typing import Any, Callable, Optional
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

TEMPLATE_VERSION = 1
MANIFEST_SCHEMA = 2
MANIFEST_CONTENT_TYPE = "application/vnd.portalsurfer.release-manifest+json;version=2"
PRODUCTION_ORIGIN = "https://portalsurfer.org"
SAFE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
SAFE_BUILD_ID = re.compile(r"^[a-z0-9][a-z0-9._-]{1,127}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
TEAM_ID = re.compile(r"^[A-Z0-9]{10}$")
NOTARY_ID = re.compile(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$")
SEMVER = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$")
SCREENSHOT_WIDTH = 300
SCREENSHOT_HEIGHT = 320


def canonical_json(value: Any) -> bytes:
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode()


def file_digest(path: Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
            size += len(chunk)
    return digest.hexdigest(), size


def validate_png(path: Path, width: int = SCREENSHOT_WIDTH, height: int = SCREENSHOT_HEIGHT) -> tuple[str, int]:
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("screenshot is not a PNG")
    offset = 8
    seen_ihdr = seen_idat = seen_iend = False
    while offset + 12 <= len(data):
        length = struct.unpack(">I", data[offset : offset + 4])[0]
        kind = data[offset + 4 : offset + 8]
        end = offset + length + 12
        if end > len(data):
            raise ValueError("screenshot has a truncated PNG chunk")
        payload = data[offset + 8 : offset + 8 + length]
        actual_crc = struct.unpack(">I", data[offset + 8 + length : end])[0]
        if actual_crc != (zlib.crc32(kind + payload) & 0xFFFFFFFF):
            raise ValueError("screenshot has an invalid PNG chunk CRC")
        if not seen_ihdr and kind != b"IHDR":
            raise ValueError("screenshot must begin with IHDR")
        if kind == b"IHDR":
            if seen_ihdr or length != 13:
                raise ValueError("screenshot has an invalid IHDR")
            seen_ihdr = True
            actual_width, actual_height = struct.unpack(">II", payload[:8])
            if (actual_width, actual_height) != (width, height) or payload[8] != 8 or payload[9] not in (2, 6) or payload[10] != 0 or payload[11] != 0 or payload[12] not in (0, 1):
                raise ValueError(f"screenshot must be {width}x{height} 8-bit RGB/RGBA PNG")
        elif kind == b"IDAT":
            if not seen_ihdr:
                raise ValueError("screenshot IDAT precedes IHDR")
            seen_idat = True
        elif kind == b"IEND":
            if length != 0 or end != len(data):
                raise ValueError("screenshot has an invalid IEND")
            seen_iend = True
            break
        offset = end
    if not (seen_ihdr and seen_idat and seen_iend):
        raise ValueError("screenshot is missing IHDR, IDAT, or IEND")
    return file_digest(path)


def build_manifest(*, product: str, repository: str, version: str, build_id: str, channel: str, released_at: str,
                   git_sha: str, clap: Path, vst3: Path, screenshot: Path, changelog: Path,
                   signing_team_id: str, clap_notary_id: str, vst3_notary_id: str) -> dict[str, Any]:
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,62}", product):
        raise ValueError("invalid product slug")
    if repository != f"PORTALSURFER/{product}":
        raise ValueError("unknown or mismatched Portal product")
    if channel not in {"stable", "rc", "nightly"}:
        raise ValueError("invalid release channel")
    if not SEMVER.fullmatch(version):
        raise ValueError("version must be SemVer")
    if not SAFE_BUILD_ID.fullmatch(build_id):
        raise ValueError("invalid safe build id")
    if "+" in version:
        raise ValueError("release versions may not contain build metadata")
    if channel == "stable" and ("-" in version or "+" in version):
        raise ValueError("stable releases require a stable SemVer")
    if channel == "rc" and not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+-rc\.[1-9][0-9]*", version):
        raise ValueError("RC releases require X.Y.Z-rc.N")
    try:
        parsed = dt.datetime.fromisoformat(released_at.replace("Z", "+00:00"))
    except ValueError as error:
        raise ValueError("released_at must be RFC3339") from error
    if parsed.tzinfo is None:
        raise ValueError("released_at must include a timezone")
    if not re.fullmatch(r"[0-9a-f]{40}", git_sha):
        raise ValueError("source SHA must be exact")
    if not TEAM_ID.fullmatch(signing_team_id) or not NOTARY_ID.fullmatch(clap_notary_id) or not NOTARY_ID.fullmatch(vst3_notary_id):
        raise ValueError("production signing/notarization evidence is incomplete")
    screenshot_hash, screenshot_size = validate_png(screenshot)
    artifacts = []
    for fmt, path in (("clap", clap), ("vst3", vst3)):
        digest, size = file_digest(path)
        expected_name = f"{product}-v{version}-macos.{fmt}.zip"
        if path.name != expected_name:
            raise ValueError(f"{fmt} artifact must be named {expected_name}")
        if not SAFE_NAME.fullmatch(path.name) or size <= 0 or not SHA256.fullmatch(digest):
            raise ValueError(f"invalid {fmt} artifact")
        artifacts.append({"format": fmt, "platform": "macos", "architectures": ["arm64"], "name": path.name,
                          "media_type": "application/zip", "sha256": digest, "size_bytes": size})
    changelog_hash, changelog_size = file_digest(changelog)
    if changelog_size <= 0:
        raise ValueError("CHANGELOG.md must not be empty")
    names = [item["name"] for item in artifacts] + [screenshot.name, changelog.name]
    if len(names) != len(set(names)) or any(not SAFE_NAME.fullmatch(name) for name in names):
        raise ValueError("release file names must be unique safe basenames")
    if screenshot.name != f"{product}-default-{SCREENSHOT_WIDTH}x{SCREENSHOT_HEIGHT}.png" or changelog.name != "CHANGELOG.md":
        raise ValueError("screenshot/changelog names do not match the product contract")
    return {
        "schema_version": MANIFEST_SCHEMA,
        "product": product,
        "build_id": build_id,
        "version": version,
        "channel": channel,
        "released_at": released_at,
        "source": {"repository": repository, "git_sha": git_sha, "dirty": False},
        "distribution": "production",
        "signing": {"identity_class": "Developer ID Application", "notarized": True, "stapled": True,
                     "team_id": signing_team_id, "notary_submissions": {"clap": clap_notary_id, "vst3": vst3_notary_id}},
        "artifacts": artifacts,
        "screenshot": {"role": "default-ui", "name": screenshot.name, "media_type": "image/png", "width": SCREENSHOT_WIDTH,
                       "height": SCREENSHOT_HEIGHT, "logical_width": SCREENSHOT_WIDTH, "logical_height": SCREENSHOT_HEIGHT, "dpi_scale": 1.0,
                       "source_git_sha": git_sha, "sha256": screenshot_hash, "size_bytes": screenshot_size},
        "changelog": {"name": changelog.name, "format": "markdown", "media_type": "text/markdown; charset=utf-8",
                      "sha256": changelog_hash, "size_bytes": changelog_size},
    }


Transport = Callable[[str, str, Optional[bytes], dict[str, str]], tuple[int, bytes]]


def validate_canonical_source(manifest: dict[str, Any], repo_root: Path) -> None:
    def git(*args: str) -> str:
        result = subprocess.run(["git", *args], cwd=repo_root, text=True, capture_output=True, check=False)
        if result.returncode:
            raise ValueError(f"git {' '.join(args)} failed")
        return result.stdout.strip()
    if git("symbolic-ref", "--quiet", "--short", "HEAD") != "main":
        raise ValueError("production release source must be a non-detached main checkout")
    if git("status", "--porcelain", "--untracked-files=all"):
        raise ValueError("production release source must be clean")
    git("fetch", "origin", "main", "--quiet")
    head = git("rev-parse", "HEAD")
    origin = git("rev-parse", "refs/remotes/origin/main")
    source_sha = manifest["source"]["git_sha"]
    if head != origin or head != source_sha:
        raise ValueError("production release source must match HEAD, origin/main, and manifest source SHA")


def validate_manifest(manifest: dict[str, Any], root: Path) -> None:
    """Validate the complete canonical manifest and every referenced file."""
    required = {"schema_version", "product", "build_id", "version", "channel", "released_at", "source", "distribution", "signing", "artifacts", "screenshot", "changelog"}
    if set(manifest) != required or manifest["schema_version"] != MANIFEST_SCHEMA:
        raise ValueError("manifest schema or fields are invalid")
    product = manifest["product"]
    version = manifest["version"]
    source = manifest["source"]
    if not isinstance(source, dict):
        raise ValueError("manifest source must be an object")
    if set(source) != {"repository", "git_sha", "dirty"}:
        raise ValueError("manifest source fields are invalid")
    if not isinstance(product, str) or not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,62}", product):
        raise ValueError("manifest product is invalid")
    if source.get("repository") != f"PORTALSURFER/{product}" or source.get("dirty") is not False or not isinstance(source.get("git_sha"), str) or not re.fullmatch(r"[0-9a-f]{40}", source.get("git_sha", "")):
        raise ValueError("manifest source binding is invalid")
    if manifest["distribution"] != "production":
        raise ValueError("manifest distribution must be production")
    signing = manifest["signing"]
    if not isinstance(signing, dict):
        raise ValueError("manifest signing must be an object")
    if set(signing) != {"identity_class", "notarized", "stapled", "team_id", "notary_submissions"}:
        raise ValueError("manifest signing fields are invalid")
    if signing.get("identity_class") != "Developer ID Application" or signing.get("notarized") is not True or signing.get("stapled") is not True or not TEAM_ID.fullmatch(signing.get("team_id", "")):
        raise ValueError("manifest signing evidence is invalid")
    submissions = signing.get("notary_submissions", {})
    if not isinstance(submissions, dict):
        raise ValueError("manifest notarization evidence must be an object")
    if set(submissions) != {"clap", "vst3"} or any(not NOTARY_ID.fullmatch(value) for value in submissions.values()):
        raise ValueError("manifest notarization evidence is invalid")
    if not isinstance(manifest["channel"], str) or manifest["channel"] not in {"stable", "rc", "nightly"}:
        raise ValueError("manifest channel is invalid")
    if not isinstance(manifest["build_id"], str) or not isinstance(version, str) or not SAFE_BUILD_ID.fullmatch(manifest["build_id"]) or not SEMVER.fullmatch(version) or "+" in version:
        raise ValueError("manifest version/build id is invalid")
    if manifest["channel"] == "stable" and "-" in version:
        raise ValueError("stable manifest cannot contain prerelease metadata")
    if manifest["channel"] == "rc" and not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+-rc\.[1-9][0-9]*", version):
        raise ValueError("RC manifest version is invalid")
    try:
        released_at = dt.datetime.fromisoformat(str(manifest["released_at"]).replace("Z", "+00:00"))
    except ValueError as error:
        raise ValueError("manifest released_at is invalid") from error
    if released_at.tzinfo is None:
        raise ValueError("manifest released_at must include a timezone")
    artifacts = manifest["artifacts"]
    if not isinstance(artifacts, list):
        raise ValueError("manifest artifacts must be an array")
    if len(artifacts) != 2 or any(not isinstance(item, dict) for item in artifacts) or {item.get("format") for item in artifacts} != {"clap", "vst3"}:
        raise ValueError("manifest must contain exactly CLAP and VST3 artifacts")
    names = []
    for item in artifacts:
        if not isinstance(item, dict) or set(item) != {"format", "platform", "architectures", "name", "media_type", "sha256", "size_bytes"}:
            raise ValueError("artifact fields are invalid")
        expected_name = f"{product}-v{version}-macos.{item['format']}.zip"
        if item.get("platform") != "macos" or item.get("architectures") != ["arm64"] or item.get("name") != expected_name or item.get("media_type") != "application/zip":
            raise ValueError("artifact metadata does not match the product contract")
        if not SHA256.fullmatch(item.get("sha256", "")) or not isinstance(item.get("size_bytes"), int) or item["size_bytes"] <= 0:
            raise ValueError("artifact hash/size is invalid")
        names.append(item["name"])
    screenshot = manifest["screenshot"]
    if not isinstance(screenshot, dict):
        raise ValueError("manifest screenshot must be an object")
    if set(screenshot) != {"role", "name", "media_type", "width", "height", "logical_width", "logical_height", "dpi_scale", "source_git_sha", "sha256", "size_bytes"}:
        raise ValueError("screenshot fields are invalid")
    if screenshot.get("role") != "default-ui" or screenshot.get("media_type") != "image/png" or screenshot.get("name") != f"{product}-default-{SCREENSHOT_WIDTH}x{SCREENSHOT_HEIGHT}.png" or screenshot.get("source_git_sha") != source["git_sha"] or screenshot.get("width") != SCREENSHOT_WIDTH or screenshot.get("height") != SCREENSHOT_HEIGHT or screenshot.get("logical_width") != SCREENSHOT_WIDTH or screenshot.get("logical_height") != SCREENSHOT_HEIGHT or screenshot.get("dpi_scale") != 1.0 or not SHA256.fullmatch(screenshot.get("sha256", "")) or not isinstance(screenshot.get("size_bytes"), int) or screenshot["size_bytes"] <= 0:
        raise ValueError("screenshot metadata is invalid")
    changelog = manifest["changelog"]
    if not isinstance(changelog, dict):
        raise ValueError("manifest changelog must be an object")
    if set(changelog) != {"name", "format", "media_type", "sha256", "size_bytes"}:
        raise ValueError("changelog fields are invalid")
    if changelog.get("name") != "CHANGELOG.md" or changelog.get("format") != "markdown" or changelog.get("media_type") != "text/markdown; charset=utf-8" or not SHA256.fullmatch(changelog.get("sha256", "")) or not isinstance(changelog.get("size_bytes"), int) or changelog["size_bytes"] <= 0:
        raise ValueError("changelog metadata is invalid")
    names.extend((screenshot["name"], changelog["name"]))
    if len(names) != len(set(names)) or any(not SAFE_NAME.fullmatch(name) for name in names):
        raise ValueError("manifest file names are not unique safe basenames")
    for name, expected_hash, expected_size in [(item["name"], item["sha256"], item["size_bytes"]) for item in artifacts] + [(screenshot["name"], screenshot["sha256"], screenshot["size_bytes"]), (changelog["name"], changelog["sha256"], changelog["size_bytes"])]:
        path = root / name
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"release file is not a regular file: {name}")
        actual_hash, actual_size = file_digest(path)
        if actual_hash != expected_hash or actual_size != expected_size:
            raise ValueError(f"manifest hash/size mismatch: {name}")
    validate_png(root / screenshot["name"])
    manifest_path = root / "release-manifest.json"
    if manifest_path.is_file() and manifest_path.read_bytes() != canonical_json(manifest):
        raise ValueError("release-manifest.json is not canonical JSON")


def _request(url: str, method: str, body: bytes | None, headers: dict[str, str]) -> tuple[int, bytes]:
    request = Request(url, method=method, data=body, headers=headers)
    try:
        with urlopen(request, timeout=60) as response:
            return response.status, response.read()
    except (HTTPError, URLError) as error:
        detail = error.read().decode("utf-8", "replace")[:400] if isinstance(error, HTTPError) else str(error)
        raise RuntimeError(f"{method} {url} failed: {detail}") from error


def publish_release(*, endpoint: str, token: str, manifest: dict[str, Any], root: Path, repo_root: Optional[Path] = None,
                    transport: Transport = _request) -> None:
    """Stage every file, then atomically commit one canonical manifest.

    The capability request is intentionally first. No upload occurs when the
    product is unknown or the PortalSurfer server does not support schema 2.
    """
    if endpoint != PRODUCTION_ORIGIN:
        raise ValueError("production publishing requires the exact PortalSurfer origin")
    if not token:
        raise ValueError("PORTALSURFER_RELEASE_TOKEN is required for publishing")
    validate_manifest(manifest, root)
    validate_canonical_source(manifest, repo_root or root.parents[2])
    product = manifest["product"]
    status, payload = transport(f"{endpoint}/plugins/api/v1/products/{product}/releases", "GET", None, {"Accept": "application/json"})
    if not 200 <= status < 300:
        raise RuntimeError(f"PortalSurfer capability check failed ({status}); no files were uploaded")
    capability = json.loads(payload)
    if MANIFEST_SCHEMA not in capability.get("release_upload", {}).get("manifest_schema_versions", []):
        raise RuntimeError("PortalSurfer does not support manifest schema 2; no files were uploaded")
    base = f"{endpoint}/plugins/api/v1/products/{product}/release-uploads/{manifest['build_id']}"
    headers = {"Authorization": f"Bearer {token}", "Content-Type": "application/octet-stream",
               "X-PortalSurfer-Release-Version": manifest["version"], "X-PortalSurfer-Release-Channel": manifest["channel"],
               "X-PortalSurfer-Released-At": manifest["released_at"]}
    names = [item["name"] for item in manifest["artifacts"]] + [manifest["screenshot"]["name"], manifest["changelog"]["name"]]
    for name in names:
        path = root / name
        data = path.read_bytes()
        digest, size = file_digest(path)
        expected = next((item["sha256"] for item in manifest["artifacts"] if item["name"] == name), None)
        expected = expected or manifest["screenshot"]["sha256"] if name == manifest["screenshot"]["name"] else expected
        expected = expected or manifest["changelog"]["sha256"]
        if len(data) != size or digest != expected:
            raise ValueError(f"local file changed after manifest validation: {name}")
        transport(f"{base}/staging/files/{name}", "PUT", data, {**headers, "Content-Length": str(size), "X-PortalSurfer-Sha256": digest})
    body = canonical_json(manifest)
    transport(f"{base}/commit", "PUT", body, {"Authorization": f"Bearer {token}", "Content-Type": MANIFEST_CONTENT_TYPE,
                                                   "Content-Length": str(len(body)), "X-PortalSurfer-Manifest-Sha256": hashlib.sha256(body).hexdigest(),
                                                   "X-PortalSurfer-Release-Version": manifest["version"], "X-PortalSurfer-Release-Channel": manifest["channel"],
                                                   "X-PortalSurfer-Released-At": manifest["released_at"]})


if __name__ == "__main__":
    print("This helper is imported by scripts/release.sh; it does not publish by itself.", file=sys.stderr)
    raise SystemExit(2)
