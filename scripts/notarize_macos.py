#!/usr/bin/env python3
"""Prepare one signed batch, checkpoint its submission UUID, then resume without rebuilding.

macOS tools do the copying, compression and extraction to preserve Apple metadata.
No private signing material belongs in the checkpoint directory.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
import uuid


ARCHES = ("arm64", "x86_64")


def run(command: list[str]) -> str:
    result = subprocess.run(command, text=True, capture_output=True)
    if result.returncode:
        # Never include credential-bearing arguments in exceptions.
        detail = result.stderr or result.stdout
        for key in ("APPLE_APP_PASSWORD", "APPLE_ID", "APPLE_TEAM_ID"):
            if os.environ.get(key):
                detail = detail.replace(os.environ[key], "[redacted]")
        raise RuntimeError(f"{command[0]} failed: {detail}")
    return result.stdout


def write_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_suffix(".tmp")
    temp.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
    temp.replace(path)


def read_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def credentials() -> list[str]:
    keys = ("APPLE_ID", "APPLE_TEAM_ID", "APPLE_APP_PASSWORD")
    missing = [key for key in keys if not os.environ.get(key)]
    if missing:
        raise ValueError(f"Missing notarization credentials: {', '.join(missing)}")
    return ["--apple-id", os.environ["APPLE_ID"], "--team-id", os.environ["APPLE_TEAM_ID"],
            "--password", os.environ["APPLE_APP_PASSWORD"]]


def identity(path: Path, arch: str | None = None) -> str:
    # codesign prints display output on stderr, even on success.
    command = ["/usr/bin/codesign", "--display", "--verbose=4"]
    if arch:
        command += ["--arch", arch]
    result = subprocess.run([*command, str(path)], text=True, capture_output=True)
    if result.returncode:
        raise RuntimeError(f"Cannot inspect signature of {path}")
    match = re.search(r"^CDHash=([0-9a-fA-F]{40})$", result.stderr, re.MULTILINE)
    if not match:
        raise ValueError(f"No CDHash for {path} ({arch})")
    return match[1].lower()


def verify_app(app: Path, arches: set[str], notarized: bool = False) -> None:
    run(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(app)])
    actual = set(run(["/usr/bin/lipo", "-archs", str(app / "Contents/MacOS/dost")]).split())
    if actual != arches:
        raise ValueError(f"Wrong architectures in {app}: {actual}, expected {arches}")
    if notarized:
        run(["xcrun", "stapler", "validate", str(app)])
        run(["/usr/sbin/spctl", "--assess", "--type", "execute", "-v", str(app)])


def verify_dmg(dmg: Path) -> None:
    run(["hdiutil", "verify", str(dmg)])
    run(["/usr/bin/codesign", "--verify", "--strict", str(dmg)])


def prepare(dmg: Path, universal: Path, apps: Path, checkpoint: Path) -> None:
    if checkpoint.exists():
        raise ValueError("Use a new checkpoint directory; never overwrite signed recovery inputs")
    verify_dmg(dmg)
    verify_app(universal, set(ARCHES))
    expected = [{"path": "payload/dost.dmg", "arch": None, "cdhash": identity(dmg)}]
    # Check the actual app inside the final read-only image, not just its build directory.
    with tempfile.TemporaryDirectory() as tmp:
        mount = Path(tmp) / "mount"
        run(["hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", str(mount), str(dmg)])
        try:
            bundled = mount / "Dost.app"
            verify_app(bundled, set(ARCHES))
            for arch in ARCHES:
                code_hash = identity(bundled, arch)
                if code_hash != identity(universal, arch):
                    raise ValueError("DMG app does not match the signed universal app")
                expected.append({"path": "payload/dost.dmg/", "arch": arch, "cdhash": code_hash})
        finally:
            run(["hdiutil", "detach", str(mount)])

    checkpoint.mkdir(parents=True)
    # The staging tree is temporary; only the immutable ZIP and manifest are uploaded.
    with tempfile.TemporaryDirectory() as tmp:
        payload = Path(tmp) / "payload"
        payload.mkdir()
        shutil.copy2(dmg, payload / "dost.dmg")
        for arch in ARCHES:
            app = apps / arch / "Dost.app"
            verify_app(app, {arch})
            expected.append({"path": f"payload/{arch}/Dost.app", "arch": arch,
                             "cdhash": identity(app, arch)})
            run(["ditto", str(app), str(payload / arch / "Dost.app")])
        archive = checkpoint / "submission.zip"
        run(["ditto", "-c", "-k", "--keepParent", str(payload), str(archive)])
        # Confirm metadata/signatures survive the recovery format before submission.
        restored = Path(tmp) / "restored"
        run(["ditto", "-x", "-k", str(archive), str(restored)])
        verify_dmg(restored / "payload/dost.dmg")
        for arch in ARCHES:
            verify_app(restored / f"payload/{arch}/Dost.app", {arch})

    write_json(checkpoint / "inputs.json", {
        "schema": 1, "tag": os.environ["GITHUB_REF_NAME"], "sha": os.environ["GITHUB_SHA"],
        "run_id": os.environ["GITHUB_RUN_ID"], "attempt": os.environ["GITHUB_RUN_ATTEMPT"],
        "archive_sha256": digest(archive), "archive_size": archive.stat().st_size,
        "expected_tickets": expected,
    })


def load_checkpoint(checkpoint: Path) -> dict:
    manifest = read_json(checkpoint / "inputs.json")
    archive = checkpoint / "submission.zip"
    if manifest.get("schema") != 1 or digest(archive) != manifest["archive_sha256"]:
        raise ValueError("Signed checkpoint checksum or schema mismatch")
    return manifest


def submit(checkpoint: Path, record: Path) -> None:
    if record.exists():
        raise ValueError("Submission already recorded; resume it instead of uploading again")
    manifest = load_checkpoint(checkpoint)
    print("Uploading one batch to Apple (DMG + ARM64 + Intel)…", flush=True)
    response = json.loads(run([
        "xcrun", "notarytool", "submit", str(checkpoint / "submission.zip"),
        *credentials(), "--no-wait", "--output-format", "json",
    ]))
    submission_id = str(uuid.UUID(response["id"]))
    # Written before any wait; the next Actions step uploads this small file immediately.
    write_json(record, {"id": submission_id, "archive_sha256": manifest["archive_sha256"],
                        "tag": manifest["tag"], "sha": manifest["sha"],
                        "run_id": manifest["run_id"], "attempt": manifest["attempt"],
                        "response": response})
    print(f"Apple submission: {submission_id}. Saved to {record}; no wait/re-upload needed.", flush=True)


def validate_record(manifest: dict, record: dict) -> str:
    for key in ("archive_sha256", "tag", "sha", "run_id", "attempt"):
        if manifest[key] != record.get(key):
            raise ValueError(f"Submission does not match checkpoint: {key}")
    return str(uuid.UUID(record["id"]))


def verify_coverage(manifest: dict, log: dict, submission_id: str) -> None:
    if log.get("status") != "Accepted" or str(log.get("jobId", "")).lower() != submission_id.lower():
        raise ValueError("Apple log is not Accepted or belongs to another submission")
    for expected in manifest["expected_tickets"]:
        matches = [item for item in (log.get("ticketContents") or [])
                   if item.get("cdhash", "").lower() == expected["cdhash"]
                   and item.get("arch") == expected["arch"]
                   and expected["path"] in item.get("path", "")]
        if not matches:
            raise ValueError(f"Missing Apple ticket coverage: {expected}")


def wait_for_acceptance(submission_id: str, auth: list[str], logs: Path) -> dict:
    logs.mkdir(parents=True, exist_ok=True)
    while True:
        status = json.loads(run(["xcrun", "notarytool", "info", submission_id,
                                 *auth, "--output-format", "json"]))
        write_json(logs / "status.json", status)
        value = status.get("status")
        print(f"Apple submission {submission_id}: {value}", flush=True)
        if value != "In Progress":
            log_path = logs / "apple-log.json"
            run(["xcrun", "notarytool", "log", submission_id, *auth, str(log_path)])
            if value != "Accepted":
                raise RuntimeError(f"Apple did not accept submission: {value}; see {log_path}")
            return read_json(log_path)
        # No local deadline; GitHub's platform limit still applies. A later run uses the same UUID.
        time.sleep(60)


def package(payload: Path, output: Path, notarized: bool) -> None:
    output.mkdir(parents=True, exist_ok=True)
    dmg = payload / "dost.dmg"
    verify_dmg(dmg)
    if notarized:
        run(["xcrun", "stapler", "validate", str(dmg)])
        run(["/usr/sbin/spctl", "--assess", "--type", "open", "--context",
             "context:primary-signature", "-v", str(dmg)])
    shutil.copy2(dmg, output / "dost.dmg")
    for arch in ARCHES:
        app = payload / arch / "Dost.app"
        verify_app(app, {arch}, notarized)
        archive = output / f"dost_Darwin_{arch}.tar.gz"
        run(["/usr/bin/tar", "-czf", str(archive.resolve()), "-C", str(app.parent), "Dost.app"])
        with tempfile.TemporaryDirectory() as tmp:
            run(["/usr/bin/tar", "-xzf", str(archive), "-C", tmp])
            verify_app(Path(tmp) / "Dost.app", {arch}, notarized)


def finish(checkpoint: Path, record_path: Path, output: Path, logs: Path,
           tag: str, sha: str, run_id: str, attempt: str) -> None:
    manifest = load_checkpoint(checkpoint)
    for key, expected in (("tag", tag), ("sha", sha), ("run_id", run_id), ("attempt", attempt)):
        if manifest[key] != expected:
            raise ValueError(f"Checkpoint source mismatch: {key}")
    submission_id = validate_record(manifest, read_json(record_path))
    log = wait_for_acceptance(submission_id, credentials(), logs)
    verify_coverage(manifest, log, submission_id)
    with tempfile.TemporaryDirectory() as tmp:
        run(["ditto", "-x", "-k", str(checkpoint / "submission.zip"), tmp])
        payload = Path(tmp) / "payload"
        # Staple exact originals from the signed checkpoint, without rebuilding or re-signing.
        for target in [payload / "dost.dmg", *(payload / arch / "Dost.app" for arch in ARCHES)]:
            run(["xcrun", "stapler", "staple", str(target)])
            run(["xcrun", "stapler", "validate", str(target)])
        package(payload, output, notarized=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prep = commands.add_parser("prepare")
    prep.add_argument("--dmg", type=Path, required=True)
    prep.add_argument("--universal-app", type=Path, default=Path("target/Dost.app"))
    prep.add_argument("--apps", type=Path, default=Path("target/macos"))
    prep.add_argument("--checkpoint", type=Path, default=Path("target/notary-inputs"))
    send = commands.add_parser("submit")
    send.add_argument("--checkpoint", type=Path, default=Path("target/notary-inputs"))
    send.add_argument("--record", type=Path, default=Path("target/notary-submission/submission.json"))
    resume = commands.add_parser("finish")
    resume.add_argument("--checkpoint", type=Path, default=Path("target/notary-inputs"))
    resume.add_argument("--record", type=Path, default=Path("target/notary-submission/submission.json"))
    resume.add_argument("--output", type=Path, default=Path("dist"))
    resume.add_argument("--logs", type=Path, default=Path("target/notarization"))
    for name in ("tag", "sha", "run-id", "attempt"):
        resume.add_argument(f"--{name}", required=True)
    args = parser.parse_args()
    if args.command == "prepare":
        prepare(args.dmg, args.universal_app, args.apps, args.checkpoint)
    elif args.command == "submit":
        submit(args.checkpoint, args.record)
    else:
        finish(args.checkpoint, args.record, args.output, args.logs,
               args.tag, args.sha, args.run_id, args.attempt)


if __name__ == "__main__":
    main()
