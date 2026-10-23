#!/usr/bin/env python3
"""Submit the DMG and two app ZIPs concurrently; staple only Accepted submissions."""

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
import json
import os
from pathlib import Path
import subprocess


def run(command: list[str]) -> str:
    result = subprocess.run(command, text=True, capture_output=True)
    if result.returncode:
        # Do not print the command: notarytool arguments contain the app-specific password.
        raise RuntimeError(f"{command[0]} failed: {result.stderr or result.stdout}")
    return result.stdout


def notarize(artifact: Path, targets: list[Path], credentials: list[str], logs: Path) -> None:
    print(f"Submitting {artifact} to Apple…", flush=True)
    output = run([
        "xcrun", "notarytool", "submit", str(artifact), *credentials,
        "--wait", "--output-format", "json",
    ])
    (logs / f"{artifact.parent.name}-{artifact.name}.json").write_text(output, encoding="utf-8")
    result = json.loads(output)
    status = result.get("status", "Unknown")
    print(f"{artifact}: {status} (submission {result.get('id', 'unknown')})", flush=True)
    if status != "Accepted":
        raise RuntimeError(f"Apple did not accept {artifact}: {status}")
    for target in targets:
        run(["xcrun", "stapler", "staple", str(target)])
        run(["xcrun", "stapler", "validate", str(target)])


def notarize_all(dmg: Path, universal: Path, apps: list[Path], logs: Path) -> None:
    keys = ("APPLE_ID", "APPLE_TEAM_ID", "APPLE_APP_PASSWORD")
    missing = [key for key in keys if not os.environ.get(key)]
    if missing:
        raise ValueError(f"Missing notarization credentials: {', '.join(missing)}")
    credentials = [
        "--apple-id", os.environ["APPLE_ID"], "--team-id", os.environ["APPLE_TEAM_ID"],
        "--password", os.environ["APPLE_APP_PASSWORD"],
    ]
    logs.mkdir(parents=True, exist_ok=True)
    submissions = [(dmg, [dmg, universal])]
    for app in apps:
        archive = app.parent / "notarization.zip"
        run(["ditto", "-c", "-k", "--keepParent", str(app), str(archive)])
        submissions.append((archive, [app]))

    failures = []
    with ThreadPoolExecutor(max_workers=len(submissions)) as pool:
        tasks = {
            pool.submit(notarize, artifact, targets, credentials, logs): artifact
            for artifact, targets in submissions
        }
        # Wait for every submission, even if one fails. Packaging must never run on partial success.
        for task in as_completed(tasks):
            try:
                task.result()
            except Exception as error:
                failures.append(f"{tasks[task]}: {error}")
    if failures:
        raise RuntimeError("Notarization failed:\n" + "\n".join(failures))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dmg", type=Path, required=True)
    parser.add_argument("--universal-app", type=Path, required=True)
    parser.add_argument("--app", type=Path, action="append", required=True)
    parser.add_argument("--logs", type=Path, default=Path("target/notarization"))
    args = parser.parse_args()
    notarize_all(args.dmg, args.universal_app, args.app, args.logs)


if __name__ == "__main__":
    main()
