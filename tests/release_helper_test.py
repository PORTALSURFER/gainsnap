#!/usr/bin/env python3
"""Focused regression tests for the GainSnap release producer."""

from __future__ import annotations

import struct
import sys
import tempfile
import unittest
import zlib
from pathlib import Path


ROOT = Path(__file__).parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import release_helper


TEAM_ID = "TEAM123456"
NOTARY_ID = "12345678-1234-4123-8123-123456789abc"
SOURCE_SHA = "a" * 40


def png(width: int, height: int) -> bytes:
    def chunk(kind: bytes, payload: bytes) -> bytes:
        checksum = zlib.crc32(kind + payload) & 0xFFFFFFFF
        return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", checksum)

    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", b"x") + chunk(b"IEND", b"")


class ReleaseHelperTests(unittest.TestCase):
    def test_publication_version_derivation(self) -> None:
        self.assertEqual(release_helper.derive_publication_version("0.1.0", "stable", 17), "0.1.0")
        self.assertEqual(
            release_helper.derive_publication_version("0.1.0", "rc", 17), "0.1.0-rc.17"
        )
        self.assertEqual(
            release_helper.derive_publication_version("0.1.0", "nightly", 17), "0.1.0-nightly.17"
        )

        for sequence in (0, "0", -1, "-1", "01", "not-a-sequence", True):
            with self.subTest(sequence=sequence), self.assertRaises(ValueError):
                release_helper.derive_publication_version("0.1.0", "nightly", sequence)

        for package_version in ("", "01.2.3", "1.2", "1.2.3-dev.1", "1.2.3+build"):
            with self.subTest(package_version=package_version), self.assertRaises(ValueError):
                release_helper.derive_publication_version(package_version, "nightly", 17)

    def test_publication_version_validation(self) -> None:
        release_helper.validate_publication_version("0.1.0", "0.1.0-nightly.17", "nightly")
        release_helper.validate_publication_version("0.1.0", "0.1.0-rc.17", "rc")
        release_helper.validate_publication_version("0.1.0", "0.1.0", "stable")

        for package_version, publication_version, channel in (
            ("0.1.0", "0.1.1-nightly.17", "nightly"),
            ("0.1.0", "0.1.0-nightly.0", "nightly"),
            ("0.1.0", "0.1.0", "nightly"),
            ("0.1.0", "0.1.0-rc.17", "nightly"),
            ("0.1.0", "01.1.0-nightly.17", "nightly"),
            ("0.1.0", "0.1.0-nightly.-1", "nightly"),
        ):
            with self.subTest(package_version=package_version, publication_version=publication_version):
                with self.assertRaises(ValueError):
                    release_helper.validate_publication_version(package_version, publication_version, channel)

    def _build_manifest(self, root: Path, version: str) -> dict:
        clap = root / f"gainsnap-v{version}-macos.clap.zip"
        vst3 = root / f"gainsnap-v{version}-macos.vst3.zip"
        screenshot = root / "gainsnap-default-300x320.png"
        changelog = root / "CHANGELOG.md"
        clap.write_bytes(b"clap zip")
        vst3.write_bytes(b"vst3 zip")
        screenshot.write_bytes(png(300, 320))
        changelog.write_text("release notes\n", encoding="utf-8")
        return release_helper.build_manifest(
            product="gainsnap",
            repository="PORTALSURFER/gainsnap",
            version=version,
            build_id=f"gainsnap-v{version}-aaaaaaaaaaaa",
            channel="nightly",
            released_at="2026-09-04T00:00:00Z",
            git_sha=SOURCE_SHA,
            clap=clap,
            vst3=vst3,
            screenshot=screenshot,
            changelog=changelog,
            signing_team_id=TEAM_ID,
            clap_notary_id=NOTARY_ID,
            vst3_notary_id=NOTARY_ID,
        )

    def test_manifest_requires_channel_qualified_nightly_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            publication_version = "0.1.0-nightly.17"
            manifest = self._build_manifest(root, publication_version)
            release_helper.validate_manifest(manifest, root)
            self.assertEqual(manifest["version"], publication_version)
            self.assertEqual(
                manifest["artifacts"][0]["name"],
                "gainsnap-v0.1.0-nightly.17-macos.clap.zip",
            )
            self.assertEqual(
                manifest["artifacts"][1]["name"],
                "gainsnap-v0.1.0-nightly.17-macos.vst3.zip",
            )

            manifest["version"] = "0.1.0"
            with self.assertRaisesRegex(ValueError, "nightly release version syntax"):
                release_helper.validate_manifest(manifest, root)

        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(ValueError, "nightly release version syntax"):
                release_helper.build_manifest(
                    product="gainsnap",
                    repository="PORTALSURFER/gainsnap",
                    version="0.1.0",
                    build_id="gainsnap-v0.1.0-aaaaaaaaaaaa",
                    channel="nightly",
                    released_at="2026-09-04T00:00:00Z",
                    git_sha=SOURCE_SHA,
                    clap=Path(directory) / "missing-clap.zip",
                    vst3=Path(directory) / "missing-vst3.zip",
                    screenshot=Path(directory) / "missing.png",
                    changelog=Path(directory) / "missing.md",
                    signing_team_id=TEAM_ID,
                    clap_notary_id=NOTARY_ID,
                    vst3_notary_id=NOTARY_ID,
                )


if __name__ == "__main__":
    unittest.main()
