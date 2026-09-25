import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1] / "scripts"))
import local_nightly


class LocalNightlyPlanTests(unittest.TestCase):
    def test_changed_source_gets_next_suffix_without_actions_run_number(self) -> None:
        document = {"releases": [
            {"channel": "nightly", "version": "0.1.3-nightly.4", "released_at": "2026-09-24T10:00:00Z", "source": {"repository": "PORTALSURFER/gainsnap", "git_sha": "a" * 40}},
            {"channel": "nightly", "version": "0.1.3-nightly.2", "released_at": "2026-09-23T10:00:00Z", "source": {"repository": "PORTALSURFER/gainsnap", "git_sha": "b" * 40}},
        ]}
        self.assertEqual(local_nightly.plan("c" * 40, "0.1.3", document, False), "0.1.3-nightly.5")
        self.assertIsNone(local_nightly.plan("a" * 40, "0.1.3", document, False))
        self.assertEqual(local_nightly.plan("a" * 40, "0.1.3", document, True), "0.1.3-nightly.5")

    def test_older_package_cannot_replace_newer_public_release(self) -> None:
        document = {"releases": [
            {"channel": "stable", "version": "0.1.4", "released_at": "2026-09-24T10:00:00Z", "source": {"repository": "PORTALSURFER/gainsnap", "git_sha": "a" * 40}},
        ]}
        with self.assertRaisesRegex(local_nightly.LocalReleaseError, "older"):
            local_nightly.plan("b" * 40, "0.1.3", document, False)


if __name__ == "__main__":
    unittest.main()
