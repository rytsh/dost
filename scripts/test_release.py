import hashlib
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

from release import ARCHIVES, ASSETS, finalize, package


class ReleaseTests(unittest.TestCase):
    def test_archives(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = root / "binary"
            binary.write_bytes(b"executable")
            for system, arch in ARCHIVES:
                dest = package(system, arch, binary, root / "dist")
                if system == "Windows":
                    with zipfile.ZipFile(dest) as archive:
                        self.assertEqual(set(archive.namelist()), {"dost.exe", "README.md", "LICENSE"})
                        self.assertEqual(archive.read("dost.exe"), b"executable")
                else:
                    with tarfile.open(dest) as archive:
                        self.assertEqual(set(archive.getnames()), {"dost", "README.md", "LICENSE"})
                        self.assertEqual(archive.extractfile("dost").read(), b"executable")
                        self.assertEqual(archive.getmember("dost").mode, 0o755)

    def test_missing_binary(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with self.assertRaisesRegex(ValueError, "missing or empty binary"):
                package("Linux", "arm64", root / "missing", root / "dist")

    def test_manifest_and_notes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            output = root / "dist"
            output.mkdir()
            for name in ASSETS:
                (output / name).write_bytes(name.encode())
            notes = root / "notes.md"
            manifest = finalize(output, "v0.2.3", "0.2.3", notes)
            self.assertEqual(manifest.name, "dost-0.2.3-checksums.txt")
            self.assertEqual(len(manifest.read_text().splitlines()), len(ASSETS))
            self.assertIn("dost_Darwin_arm64.tar.gz", ASSETS)
            self.assertIn("dost_Darwin_x86_64.tar.gz", ASSETS)
            self.assertNotIn("dost_Darwin_universal.zip", ASSETS)
            for line in manifest.read_text().splitlines():
                digest, name = line.split()
                self.assertEqual(digest, hashlib.sha256((output / name).read_bytes()).hexdigest())
            for name in ASSETS:
                self.assertIn(name, notes.read_text())
            self.assertIn("manually once", notes.read_text())

    def test_reject_incomplete_or_mismatched_release(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            notes = root / "notes.md"
            with self.assertRaisesRegex(ValueError, "does not match"):
                finalize(root, "v0.2.2", "0.2.3", notes)
            with self.assertRaisesRegex(ValueError, "missing="):
                finalize(root, "v0.2.3", "0.2.3", notes)
            for name in ASSETS:
                (root / name).write_bytes(b"ok")
            (root / "dost.dmg").write_bytes(b"")
            with self.assertRaisesRegex(ValueError, "empty release asset"):
                finalize(root, "v0.2.3", "0.2.3", notes)
            (root / "unexpected.exe").write_bytes(b"extra")
            with self.assertRaisesRegex(ValueError, "extra="):
                finalize(root, "v0.2.3", "0.2.3", notes)


if __name__ == "__main__":
    unittest.main()
