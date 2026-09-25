import contextlib
import io
import json
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).parents[1] / "scripts"))
import local_publisher_token as publisher


class PublisherRotationTests(unittest.TestCase):
    def exercise_rotation(self, fail_auth=False):
        operations = []
        stored = [None]

        def remote(_target, operation, transaction="", digest=""):
            operations.append(operation)
            if operation == "version":
                return {"status": "ok", "version": "audiodev-publisher-credentials/v1"}
            if operation == "check":
                return {"status": "ok", "pending_transaction": None, "products": ["gainsnap"]}
            if operation == "provision":
                self.assertEqual(len(digest), 64)
                return {"status": "prepared", "transaction_id": transaction}
            return {"status": "finalized" if operation == "finalize" else "rolled_back"}

        def keychain(operation, value=b""):
            if operation == "store":
                stored[0] = value
                return b""
            if operation == "read":
                return stored[0]
            raise AssertionError(operation)

        def auth_check(_token):
            if fail_auth:
                raise publisher.ProvisionError("auth rejected")

        endpoint = io.BytesIO(json.dumps({"product": "gainsnap"}).encode())
        output = io.StringIO()
        with patch.object(publisher, "remote", side_effect=remote), \
             patch.object(publisher, "current_keychain_token", return_value=None), \
             patch.object(publisher, "keychain", side_effect=keychain), \
             patch.object(publisher, "auth_check", side_effect=auth_check), \
             patch.object(publisher, "urlopen", return_value=endpoint), \
             patch.object(publisher.secrets, "token_bytes", return_value=b"x" * 32), \
             contextlib.redirect_stdout(output):
            if fail_auth:
                with self.assertRaisesRegex(publisher.ProvisionError, "before finalization"):
                    publisher.rotate("test-host")
            else:
                publisher.rotate("test-host")
        self.assertNotIn("eHh4", output.getvalue())
        return operations, stored[0]

    def test_rotation_finalizes_only_after_keychain_store_and_auth(self):
        operations, stored = self.exercise_rotation()
        self.assertEqual(operations, ["version", "check", "provision", "finalize"])
        self.assertIsNotNone(stored)

    def test_auth_failure_rolls_back_without_storing_token(self):
        operations, stored = self.exercise_rotation(fail_auth=True)
        self.assertEqual(operations, ["version", "check", "provision", "rollback"])
        self.assertIsNone(stored)


if __name__ == "__main__":
    unittest.main()
