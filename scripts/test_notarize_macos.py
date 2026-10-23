import copy
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import notarize_macos as notary


ID = "2efe2717-52ef-43a5-96dc-0797e4ca1041"
AUTH = {"APPLE_ID": "test@example.com", "APPLE_TEAM_ID": "TEAMID1234",
        "APPLE_APP_PASSWORD": "private-password"}


def manifest():
    return {
        "schema": 1, "tag": "v0.2.4", "sha": "a" * 40, "run_id": "123", "attempt": "1",
        "archive_sha256": "b" * 64,
        "expected_tickets": [
            {"path": "payload/dost.dmg", "arch": None, "cdhash": "1" * 40},
            {"path": "payload/dost.dmg/", "arch": "arm64", "cdhash": "2" * 40},
            {"path": "payload/dost.dmg/", "arch": "x86_64", "cdhash": "3" * 40},
            {"path": "payload/arm64/Dost.app", "arch": "arm64", "cdhash": "4" * 40},
            {"path": "payload/x86_64/Dost.app", "arch": "x86_64", "cdhash": "5" * 40},
        ],
    }


def record(inputs):
    return {key: inputs[key] for key in ("archive_sha256", "tag", "sha", "run_id", "attempt")} | {"id": ID}


def accepted_log(inputs):
    return {"jobId": ID.upper(), "status": "Accepted", "ticketContents": [
        item | {"path": "submission.zip/" + item["path"] +
                ("Dost.app/Contents/MacOS/dost" if item["path"].endswith("/") else "")}
        for item in inputs["expected_tickets"]
    ]}


class NotarizationTests(unittest.TestCase):
    def test_prepare_makes_one_checkpoint_with_final_dmg_and_both_apps(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            dmg = root / "original.dmg"
            dmg.write_bytes(b"signed final disk image")
            apps = root / "apps"
            for arch in notary.ARCHES:
                binary = apps / arch / "Dost.app/Contents/MacOS/dost"
                binary.parent.mkdir(parents=True)
                binary.write_bytes(f"signed {arch}".encode())
            checkpoint = root / "checkpoint"

            def fake_run(command):
                if command[0] == "ditto":
                    if "-c" in command:
                        source, dest = map(Path, command[-2:])
                        with zipfile.ZipFile(dest, "w") as archive:
                            for file in source.rglob("*"):
                                if file.is_file():
                                    archive.write(file, file.relative_to(source.parent))
                    elif "-x" in command:
                        with zipfile.ZipFile(command[-2]) as archive:
                            archive.extractall(command[-1])
                    else:
                        shutil.copytree(command[1], command[2])
                return ""

            env = AUTH | {"GITHUB_REF_NAME": "v0.2.4", "GITHUB_SHA": "a" * 40,
                          "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1"}
            with patch.dict(os.environ, env), patch.object(notary, "run", side_effect=fake_run), \
                    patch.object(notary, "verify_app"), patch.object(notary, "verify_dmg"), \
                    patch.object(notary, "identity", side_effect=lambda path, arch=None: "1" * 40):
                notary.prepare(dmg, root / "universal.app", apps, checkpoint)
                with self.assertRaisesRegex(ValueError, "never overwrite"):
                    notary.prepare(dmg, root / "universal.app", apps, checkpoint)
            inputs = notary.load_checkpoint(checkpoint)
            self.assertEqual(len(inputs["expected_tickets"]), 5)
            self.assertNotIn("private-password", (checkpoint / "inputs.json").read_text())
            self.assertEqual(set(p.name for p in checkpoint.iterdir()), {"inputs.json", "submission.zip"})
            with zipfile.ZipFile(checkpoint / "submission.zip") as archive:
                self.assertEqual(archive.read("payload/dost.dmg"), dmg.read_bytes())
                self.assertEqual(set(archive.namelist()), {
                    "payload/dost.dmg", "payload/arm64/Dost.app/Contents/MacOS/dost",
                    "payload/x86_64/Dost.app/Contents/MacOS/dost",
                })

    def make_checkpoint(self, root):
        archive = root / "submission.zip"
        archive.write_bytes(b"exact signed ZIP")
        inputs = manifest()
        inputs["archive_sha256"] = notary.digest(archive)
        notary.write_json(root / "inputs.json", inputs)
        return inputs

    def test_submit_once_saves_uuid_without_waiting(self):
        with tempfile.TemporaryDirectory() as tmp, patch.dict(os.environ, AUTH):
            root = Path(tmp)
            inputs = self.make_checkpoint(root)
            with patch.object(notary, "run", return_value=json.dumps({"id": ID})) as command:
                dest = root / "record.json"
                notary.submit(root, dest)
                command.assert_called_once()
                args = command.call_args.args[0]
                self.assertIn("--no-wait", args)
                self.assertNotIn("--wait", args)
                self.assertEqual(notary.validate_record(inputs, notary.read_json(dest)), ID)
                with self.assertRaisesRegex(ValueError, "already recorded"):
                    notary.submit(root, dest)
                command.assert_called_once()

    def test_changed_checkpoint_is_never_submitted(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.make_checkpoint(root)
            (root / "submission.zip").write_bytes(b"changed")
            with patch.object(notary, "run") as command:
                with self.assertRaisesRegex(ValueError, "checksum"):
                    notary.submit(root, root / "record.json")
                command.assert_not_called()

    def test_record_must_match_source_and_exact_archive(self):
        inputs = manifest()
        for key in ("tag", "sha", "run_id", "attempt", "archive_sha256"):
            wrong = record(inputs) | {key: "different"}
            with self.assertRaisesRegex(ValueError, key):
                notary.validate_record(inputs, wrong)
        with self.assertRaises(ValueError):
            notary.validate_record(inputs, record(inputs) | {"id": "not-a-uuid"})

    def test_every_architecture_and_container_must_be_covered(self):
        inputs = manifest()
        notary.verify_coverage(inputs, accepted_log(inputs), ID)
        for index in range(5):
            log = accepted_log(inputs)
            del log["ticketContents"][index]
            with self.assertRaisesRegex(ValueError, "Missing Apple ticket"):
                notary.verify_coverage(inputs, log, ID)

    def test_wrong_hash_path_arch_or_submission_is_rejected(self):
        inputs = manifest()
        original = accepted_log(inputs)
        for key, value in (("cdhash", "0" * 40), ("arch", "wrong"), ("path", "another.app")):
            log = copy.deepcopy(original)
            log["ticketContents"][3][key] = value
            with self.assertRaisesRegex(ValueError, "Missing Apple ticket"):
                notary.verify_coverage(inputs, log, ID)
        for key, value in (("status", "Invalid"), ("jobId", "another-id")):
            with self.assertRaises(ValueError):
                notary.verify_coverage(inputs, original | {key: value}, ID)

    def test_resume_polls_same_uuid_and_fetches_log(self):
        inputs = manifest()
        statuses = iter(["In Progress", "In Progress", "Accepted"])
        with tempfile.TemporaryDirectory() as tmp:
            logs = Path(tmp)

            def fake_run(command):
                self.assertEqual(command[3], ID)
                self.assertNotIn("submit", command)
                if command[2] == "info":
                    return json.dumps({"status": next(statuses)})
                Path(command[-1]).write_text(json.dumps(accepted_log(inputs)))
                return ""

            with patch.object(notary, "run", side_effect=fake_run), patch.object(notary.time, "sleep") as sleep:
                self.assertEqual(notary.wait_for_acceptance(ID, [], logs), accepted_log(inputs))
                self.assertEqual(sleep.call_count, 2)
            self.assertEqual(notary.read_json(logs / "status.json")["status"], "Accepted")

    def test_rejected_submission_keeps_log_and_never_packages(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inputs = self.make_checkpoint(root)
            notary.write_json(root / "record.json", record(inputs))

            def fake_run(command):
                if command[2] == "info":
                    return json.dumps({"status": "Invalid"})
                Path(command[-1]).write_text(json.dumps({"status": "Invalid", "issues": ["bad signature"]}))
                return ""

            with patch.dict(os.environ, AUTH), patch.object(notary, "run", side_effect=fake_run), \
                    patch.object(notary, "package") as package:
                with self.assertRaisesRegex(RuntimeError, "did not accept"):
                    notary.finish(root, root / "record.json", root / "dist", root / "logs",
                                  inputs["tag"], inputs["sha"], "123", "1")
                package.assert_not_called()
            self.assertTrue((root / "logs/apple-log.json").exists())

    def test_resume_staples_originals_without_building_or_submitting(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inputs = self.make_checkpoint(root)
            notary.write_json(root / "record.json", record(inputs))
            with patch.dict(os.environ, AUTH), \
                    patch.object(notary, "wait_for_acceptance", return_value=accepted_log(inputs)), \
                    patch.object(notary, "run", return_value="") as command, \
                    patch.object(notary, "package") as package:
                notary.finish(root, root / "record.json", root / "dist", root / "logs",
                              inputs["tag"], inputs["sha"], "123", "1")
                commands = [call.args[0] for call in command.call_args_list]
                self.assertEqual(sum(c[:3] == ["xcrun", "stapler", "staple"] for c in commands), 3)
                self.assertFalse(any("submit" in c or "make" in c or "--sign" in c for c in commands))
                package.assert_called_once()

    def test_wrong_source_never_contacts_apple(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            inputs = self.make_checkpoint(root)
            with patch.object(notary, "wait_for_acceptance") as wait:
                with self.assertRaisesRegex(ValueError, "source mismatch"):
                    notary.finish(root, root / "record.json", root / "dist", root / "logs",
                                  "v9.9.9", inputs["sha"], "123", "1")
                wait.assert_not_called()

    def test_missing_credentials(self):
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(ValueError, "Missing notarization credentials"):
                notary.credentials()

    def test_subprocess_error_redacts_password_and_arguments(self):
        with patch.dict(os.environ, AUTH), patch.object(notary.subprocess, "run") as command:
            command.return_value.returncode = 1
            command.return_value.stderr = "connection failed: private-password test@example.com"
            with self.assertRaises(RuntimeError) as error:
                notary.run(["xcrun", "--password", "private-password"])
            self.assertNotIn("private-password", str(error.exception))
            self.assertNotIn("test@example.com", str(error.exception))

    def test_identity_reads_arch_specific_cdhash_from_stderr(self):
        with patch.object(notary.subprocess, "run") as command:
            command.return_value.returncode = 0
            command.return_value.stderr = "CDHash=" + "A" * 40 + "\n"
            self.assertEqual(notary.identity(Path("Dost.app"), "arm64"), "a" * 40)
            self.assertIn("arm64", command.call_args.args[0])
            command.return_value.stderr = "missing hash"
            with self.assertRaisesRegex(ValueError, "No CDHash"):
                notary.identity(Path("Dost.app"))


if __name__ == "__main__":
    unittest.main()
