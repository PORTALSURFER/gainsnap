#!/usr/bin/env python3
"""Run GainSnap's signed macOS nightly entirely on this Mac."""

from __future__ import annotations

import argparse
import fcntl
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

import release_helper


ROOT = Path(__file__).resolve().parents[1]
RELEASES_URL = "https://portalsurfer.org/plugins/api/v1/products/gainsnap/releases"
KEYCHAIN = ROOT / "scripts/local_keychain.swift"
VERSION = re.compile(r"(?P<core>[0-9]+\.[0-9]+\.[0-9]+)-nightly\.(?P<number>[1-9][0-9]*)\Z")


class LocalReleaseError(RuntimeError):
    pass


def git(*args: str) -> str:
    result = subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True, check=False)
    if result.returncode:
        raise LocalReleaseError(f"git {' '.join(args)} failed")
    return result.stdout.strip()


def source() -> tuple[str, str]:
    if git("status", "--porcelain", "--untracked-files=all"):
        raise LocalReleaseError("release checkout must be clean")
    if git("symbolic-ref", "--quiet", "--short", "HEAD") != "main":
        raise LocalReleaseError("release checkout must be main")
    head = git("rev-parse", "HEAD")
    remote = git("ls-remote", "origin", "refs/heads/main").split()
    if len(remote) != 2 or remote[0] != head:
        raise LocalReleaseError("local main is not the exact origin/main commit")
    package = json.loads(subprocess.run(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version=1"],
        cwd=ROOT, capture_output=True, text=True, check=True,
    ).stdout)
    matches = [p["version"] for p in package["packages"] if Path(p["manifest_path"]).resolve() == ROOT / "Cargo.toml"]
    if len(matches) != 1:
        raise LocalReleaseError("GainSnap package version is unavailable")
    return head, matches[0]


def history() -> dict:
    try:
        with urlopen(Request(RELEASES_URL, headers={"Accept": "application/json"}), timeout=30) as response:
            document = json.load(response)
    except (HTTPError, URLError, OSError, ValueError) as error:
        raise LocalReleaseError("public GainSnap release history is unavailable") from error
    if not isinstance(document, dict) or document.get("product") != "gainsnap" or not isinstance(document.get("releases"), list):
        raise LocalReleaseError("public GainSnap release history is invalid")
    return document


def plan(head: str, package: str, document: dict, force: bool) -> str | None:
    latest_public = release_helper.latest_release_version(document)
    if latest_public and tuple(map(int, latest_public.split("-", 1)[0].split("."))) > tuple(map(int, package.split("."))):
        raise LocalReleaseError("package version is older than a public release")
    releases = [item for item in document["releases"] if isinstance(item, dict) and item.get("channel") == "nightly"]
    if not force and any(item.get("source", {}).get("git_sha") == head for item in releases):
        return None
    numbers = []
    for item in releases:
        match = VERSION.fullmatch(str(item.get("version", "")))
        if match and match.group("core") == package:
            numbers.append(int(match.group("number")))
    return f"{package}-nightly.{max(numbers, default=0) + 1}"


def token() -> str:
    result = subprocess.run(["swift", str(KEYCHAIN), "read"], cwd=ROOT, capture_output=True, timeout=30, check=False)
    if result.returncode or not result.stdout:
        raise LocalReleaseError("GainSnap publisher token is missing from macOS Keychain; run scripts/local_publisher_token.py --execute")
    try:
        return result.stdout.decode("ascii")
    except UnicodeDecodeError as error:
        raise LocalReleaseError("GainSnap publisher token in Keychain is invalid") from error


def signing_environment() -> dict[str, str]:
    env = dict(os.environ)
    sdk = Path(env.get("VST3_SDK_DIR", str(ROOT.parent / "vst3sdk"))).resolve()
    if not (sdk / "pluginterfaces").is_dir() or git("-C", str(sdk), "rev-parse", "HEAD") != "58f8da7936800732561402d7936584ca4505de07":
        raise LocalReleaseError("local VST3 SDK is missing or is not the pinned revision")
    env["VST3_SDK_DIR"] = str(sdk)
    if not env.get("APPLE_CODESIGN_IDENTITY"):
        result = subprocess.run(["security", "find-identity", "-v", "-p", "codesigning"], capture_output=True, text=True, check=True)
        identities = set(re.findall(r'([A-Fa-f0-9]{40}) "Developer ID Application: [^\"]+ \(DKTKQ8U5T8\)"', result.stdout))
        if len(identities) != 1:
            raise LocalReleaseError("set APPLE_CODESIGN_IDENTITY to the intended Developer ID fingerprint")
        env["APPLE_CODESIGN_IDENTITY"] = identities.pop()
    if not env.get("APPLE_NOTARY_KEYCHAIN_PROFILE") and not env.get("APPLE_NOTARY_KEY_PATH"):
        env["APPLE_NOTARY_KEYCHAIN_PROFILE"] = "gainsnap-local"
    if env.get("APPLE_NOTARY_KEYCHAIN_PROFILE"):
        result = subprocess.run(
            ["xcrun", "notarytool", "history", "--keychain-profile", env["APPLE_NOTARY_KEYCHAIN_PROFILE"], "--output-format", "json"],
            capture_output=True, check=False, timeout=30,
        )
        if result.returncode:
            raise LocalReleaseError("notarytool Keychain profile is unavailable; configure gainsnap-local before packaging")
    return env


def verify_published(manifest: dict) -> None:
    releases = history()["releases"]
    matches = [item for item in releases if item.get("build_id") == manifest["build_id"]]
    if len(matches) != 1:
        raise LocalReleaseError("published release is not visible in the public catalog")
    item = matches[0]
    if item.get("version") != manifest["version"] or item.get("source", {}).get("git_sha") != manifest["source"]["git_sha"]:
        raise LocalReleaseError("published release identity differs from the local manifest")
    expected = {part["name"]: part["sha256"] for part in manifest["artifacts"]}
    actual = {part["name"]: part["sha256"] for part in item.get("files", [])}
    if expected != actual:
        raise LocalReleaseError("public artifact hashes differ from the local manifest")
    print(f"Published and verified: {item['version']} at https://portalsurfer.org/plugins/gainsnap/")


def publish_existing(root: Path, package: str) -> None:
    manifest_path = root / "release-manifest.json"
    if root.parent.resolve() != (ROOT / "dist/releases").resolve() or not manifest_path.is_file():
        raise LocalReleaseError("existing release must be under dist/releases with a manifest")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("schema_version") != 2 or manifest.get("channel") != "nightly" or manifest.get("distribution") != "production":
        raise LocalReleaseError("existing release is not a local production nightly")
    release_helper.publish_release(
        endpoint="https://portalsurfer.org", token=token(), manifest_path=manifest_path,
        root=root, repo_root=ROOT, package_version=package, allow_macos_only_nightly=True,
    )
    verify_published(manifest)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--package", action="store_true", help="run local CI, sign, notarize and package")
    mode.add_argument("--publish", action="store_true", help="package, publish and verify on the landing page")
    mode.add_argument("--publish-existing", metavar="BUILD_ID", help="audit and publish an already packaged local nightly")
    parser.add_argument("--force", action="store_true", help="release an unchanged source with the next nightly suffix")
    args = parser.parse_args()
    try:
        head, package = source()
        if args.publish_existing:
            publish_existing(ROOT / "dist/releases" / args.publish_existing, package)
            return 0
        document = history()
        publication = plan(head, package, document, args.force)
        if publication is None:
            print("Current source already has a public nightly; nothing to release.")
            return 0
        build_id = f"gainsnap-v{publication}-{head[:12]}"
        print(f"Local nightly: {publication} from {head} ({build_id})")
        if not args.package and not args.publish:
            print("Plan only. Use --package or --publish to run the signed local pipeline.")
            return 0
        if args.publish:
            token()  # Fail before a long build if upload credentials are missing.
        env = signing_environment()
        lock = ROOT / "target/local-nightly.lock"
        lock.parent.mkdir(parents=True, exist_ok=True)
        with lock.open("w") as handle:
            try:
                fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise LocalReleaseError("another local GainSnap nightly is running") from error
            subprocess.run([
                "bash", "scripts/release.sh", "--package-only", "--local-signing", "--macos-only-nightly",
                "--channel", "nightly", "--publication-version", publication,
                "--build-id", build_id, "--source-ref", "main", "--source-sha", head,
            ], cwd=ROOT, env=env, check=True)
            if args.publish:
                publish_existing(ROOT / "dist/releases" / build_id, package)
            else:
                print(f"Signed bundle ready: {ROOT / 'dist/releases' / build_id}")
    except (LocalReleaseError, ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"local nightly failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
