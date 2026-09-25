#!/usr/bin/env python3
"""Rotate GainSnap's PortalSurfer publisher token into macOS Keychain."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import secrets
import shlex
import subprocess
import sys
import time
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[1]
KEYCHAIN = ROOT / "scripts/local_keychain.swift"
WRAPPER = "/opt/portalsurfer/hosting/audiodev-publisher-admin.sh"
ORIGIN = "https://portalsurfer.org"
PRODUCT = "gainsnap"


class ProvisionError(RuntimeError):
    pass


def remote(target: str, operation: str, transaction: str = "", digest: str = "") -> dict:
    args = ["sh", WRAPPER, operation]
    if operation == "provision":
        args += ["--product", PRODUCT, "--transaction-id", transaction, "--rotate", "--hash-stdin"]
    elif operation in ("finalize", "rollback"):
        args += ["--transaction-id", transaction]
    command = " ".join(shlex.quote(value) for value in args)
    result = subprocess.run(
        ["ssh", "-o", "StrictHostKeyChecking=yes", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", target, command],
        input=(digest + "\n").encode() if digest else None,
        capture_output=True,
        timeout=90,
        check=False,
    )
    if result.returncode != 0:
        raise ProvisionError(f"remote publisher {operation} failed (exit {result.returncode})")
    try:
        value = json.loads(result.stdout)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ProvisionError(f"remote publisher {operation} returned invalid JSON") from error
    if not isinstance(value, dict):
        raise ProvisionError(f"remote publisher {operation} returned invalid data")
    return value


def keychain(operation: str, value: bytes = b"") -> bytes:
    result = subprocess.run(
        ["swift", str(KEYCHAIN), operation], input=value, capture_output=True,
        timeout=30, check=False,
    )
    if result.returncode != 0:
        raise ProvisionError(f"Keychain {operation} failed (exit {result.returncode})")
    return result.stdout


def current_keychain_token() -> bytes | None:
    result = subprocess.run(
        ["swift", str(KEYCHAIN), "read"], capture_output=True, timeout=30, check=False,
    )
    if result.returncode == 3:
        return None
    if result.returncode:
        raise ProvisionError(f"Keychain read failed (exit {result.returncode})")
    return result.stdout


def auth_check(token: bytes) -> None:
    request = Request(
        f"{ORIGIN}/plugins/api/v1/products/{PRODUCT}/release-uploads/auth-check",
        method="POST", headers={"Authorization": f"Bearer {token.decode('ascii')}"},
    )
    try:
        with urlopen(request, timeout=30) as response:
            status = response.status
    except HTTPError as error:
        status = error.code
    except (OSError, URLError) as error:
        raise ProvisionError("PortalSurfer publisher auth-check could not be reached") from error
    if status != 204:
        raise ProvisionError(f"PortalSurfer publisher auth-check returned HTTP {status}")


def rotate(target: str) -> None:
    version = remote(target, "version")
    if version != {"status": "ok", "version": "audiodev-publisher-credentials/v1"}:
        raise ProvisionError("remote publisher wrapper version is unexpected")
    status = remote(target, "check")
    if status.get("status") != "ok" or status.get("pending_transaction") is not None or PRODUCT not in status.get("products", []):
        raise ProvisionError("remote publisher registry is not ready for GainSnap rotation")
    request = Request(f"{ORIGIN}/plugins/api/v1/products/{PRODUCT}/releases", headers={"Accept": "application/json"})
    try:
        with urlopen(request, timeout=30) as response:
            document = json.load(response)
    except (HTTPError, URLError, OSError, ValueError) as error:
        raise ProvisionError("public GainSnap release endpoint is not available") from error
    if document.get("product") != PRODUCT:
        raise ProvisionError("public GainSnap release endpoint has the wrong identity")

    previous = current_keychain_token()
    token = base64.b64encode(secrets.token_bytes(32))
    digest = hashlib.sha256(token).hexdigest()
    transaction = f"gainsnap-local-publisher-{int(time.time())}-{os.getpid()}"
    try:
        prepared = remote(target, "provision", transaction, digest)
    except ProvisionError as error:
        try:
            pending = remote(target, "check").get("pending_transaction")
            if pending == transaction:
                rollback = remote(target, "rollback", transaction)
                if rollback.get("status") != "rolled_back":
                    raise ProvisionError("remote rollback was not confirmed")
            elif pending is not None:
                raise ProvisionError(f"different remote transaction {pending} is pending")
        except ProvisionError as recovery:
            raise ProvisionError(f"provisioning outcome is uncertain; inspect transaction {transaction}: {recovery}") from error
        raise
    if prepared.get("status") != "prepared" or prepared.get("transaction_id") != transaction:
        raise ProvisionError("remote publisher did not confirm the rotation transaction")
    stored = False
    try:
        auth_check(token)
        keychain("store", token)
        stored = True
        if keychain("read") != token:
            raise ProvisionError("Keychain publisher token roundtrip did not match")
    except Exception as error:
        try:
            if stored or current_keychain_token() == token:
                keychain("store", previous) if previous is not None else keychain("delete")
            rollback = remote(target, "rollback", transaction)
            if rollback.get("status") != "rolled_back":
                raise ProvisionError("remote rollback was not confirmed")
        except Exception as recovery:
            raise ProvisionError(f"rotation failed; transaction {transaction} needs manual recovery: {recovery}") from error
        raise ProvisionError(f"rotation failed before finalization: {error}") from error

    try:
        final = remote(target, "finalize", transaction)
    except ProvisionError as error:
        raise ProvisionError(f"new local token is stored; finalize pending transaction {transaction} on the server before publishing") from error
    if final.get("status") != "finalized":
        raise ProvisionError(f"new local token is stored; finalize pending transaction {transaction} on the server before publishing")
    auth_check(token)
    print("GainSnap publisher token rotated, verified, and stored in macOS Keychain.")
    print("The previous GitHub Actions publisher secret is now retired.")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execute", action="store_true", help="perform the server rotation and Keychain write")
    parser.add_argument("--ssh-target", default=os.environ.get("PORTALSURFER_PUBLISHER_SSH_TARGET", "root@188.245.106.212"))
    args = parser.parse_args()
    if not args.execute:
        print(f"Plan: rotate the {PRODUCT} publisher credential on {args.ssh_target} and store the new token in macOS Keychain.")
        print("The previous GitHub Actions publisher secret will stop working. Rerun with --execute to perform the rotation.")
        return 0
    try:
        rotate(args.ssh_target)
    except ProvisionError as error:
        print(f"local publisher setup failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
