#!/usr/bin/env python3
"""Store the local GainSnap notary credential in macOS Keychain."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import re
import subprocess
import sys


PROFILE = "gainsnap-local"
KEY_NAME = re.compile(r"AuthKey_([A-Z0-9]{10})\.p8\Z")
ISSUER = re.compile(r"[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}\Z")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execute", action="store_true", help="validate with Apple and store the profile")
    parser.add_argument("--key", type=Path, help="App Store Connect AuthKey_XXXXXXXXXX.p8 path")
    args = parser.parse_args()
    choices = [args.key] if args.key else sorted((Path.home() / "Documents").glob("AuthKey_*.p8"))
    if len(choices) != 1 or choices[0] is None or not choices[0].is_file() or choices[0].is_symlink():
        print("Select exactly one regular App Store Connect key with --key PATH.", file=sys.stderr)
        return 1
    key = choices[0]
    match = KEY_NAME.fullmatch(key.name)
    if match is None:
        print("The key file must be named AuthKey_XXXXXXXXXX.p8.", file=sys.stderr)
        return 1
    if not args.execute:
        print(f"Plan: validate the local {key.name} with Apple and store the {PROFILE} Keychain profile.")
        print("Rerun with --execute; enter the Team API Key issuer ID interactively. Individual keys cannot notarize.")
        return 0
    if not sys.stdin.isatty():
        print("Run --execute in an interactive terminal to enter the issuer ID.", file=sys.stderr)
        return 1
    issuer = input("App Store Connect Team API Key issuer ID: ").strip()
    if not ISSUER.fullmatch(issuer):
        print("A Team API Key issuer ID (UUID) is required for notarization.", file=sys.stderr)
        return 1
    command = ["xcrun", "notarytool", "store-credentials", PROFILE, "--key", str(key), "--key-id", match.group(1), "--validate"]
    command += ["--issuer", issuer]
    result = subprocess.run(command, check=False)
    if result.returncode:
        print("Apple did not validate the notary profile; nothing was published.", file=sys.stderr)
        return 1
    print(f"Notary profile {PROFILE} is ready in macOS Keychain.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
