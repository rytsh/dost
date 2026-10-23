import json
import os
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch

from notarize_macos import notarize_all, run


class NotarizationTests(unittest.TestCase):
    def exercise_submissions(self, reject=False):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            dmg = root / "Dost.dmg"
            universal = root / "Dost.app"
            apps = [root / arch / "Dost.app" for arch in ("arm64", "x86_64")]
            # Every submit must start before any is allowed to finish: a sequential
            # implementation breaks this barrier instead of passing the test.
            barrier = threading.Barrier(3, timeout=10)
            stapled = []
            lock = threading.Lock()

            def fake_run(command):
                if command[:3] == ["xcrun", "notarytool", "submit"]:
                    barrier.wait()
                    status = "Invalid" if reject and command[3] == str(dmg) else "Accepted"
                    return json.dumps({"id": "submission-id", "status": status})
                if command[:3] == ["xcrun", "stapler", "staple"]:
                    with lock:
                        stapled.append(Path(command[3]))
                return ""

            credentials = {key: "test" for key in ("APPLE_ID", "APPLE_TEAM_ID", "APPLE_APP_PASSWORD")}
            logs = root / "logs"
            with patch.dict(os.environ, credentials), patch("notarize_macos.run", side_effect=fake_run):
                if reject:
                    with self.assertRaisesRegex(RuntimeError, "Apple did not accept"):
                        notarize_all(dmg, universal, apps, logs)
                    self.assertEqual(set(stapled), set(apps))
                else:
                    notarize_all(dmg, universal, apps, logs)
                    self.assertEqual(set(stapled), {dmg, universal, *apps})
            self.assertEqual(len(list(logs.glob("*.json"))), 3)

    def test_three_parallel_submissions(self):
        self.exercise_submissions()

    def test_rejection_fails_without_stapling_rejected_artifact(self):
        self.exercise_submissions(reject=True)

    def test_missing_credentials_fail_before_submission(self):
        with patch.dict(os.environ, {}, clear=True), patch("notarize_macos.run") as command:
            with self.assertRaisesRegex(ValueError, "Missing notarization credentials"):
                notarize_all(Path("dmg"), Path("app"), [], Path("logs"))
            command.assert_not_called()

    def test_subprocess_failure_does_not_expose_command_password(self):
        with patch("notarize_macos.subprocess.run") as command:
            command.return_value.returncode = 1
            command.return_value.stderr = "connection failed"
            with self.assertRaises(RuntimeError) as error:
                run(["xcrun", "--password", "private-password"])
            self.assertNotIn("private-password", str(error.exception))


if __name__ == "__main__":
    unittest.main()
