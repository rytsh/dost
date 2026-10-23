#!/usr/bin/env python3
"""Package Windows/Linux binaries; validate release assets, checksums and download notes.

macOS bundles are packaged with macOS tar in CI to preserve signatures and metadata.
Only Python's standard library is needed. Run tests with:
    python3 -m unittest discover -s scripts -p 'test_*.py'
"""

import argparse
import hashlib
import tarfile
import tomllib
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ARCHIVES = {
    ("Windows", "x86_64"): "dost_Windows_x86_64.zip",
    ("Windows", "arm64"): "dost_Windows_arm64.zip",
    ("Linux", "x86_64"): "dost_Linux_x86_64.tar.gz",
    ("Linux", "arm64"): "dost_Linux_arm64.tar.gz",
}
ASSETS = (
    "dost.dmg",
    "dost_Darwin_arm64.tar.gz",
    "dost_Darwin_x86_64.tar.gz",
    *ARCHIVES.values(),
)


def package(system: str, arch: str, binary: Path, output: Path) -> Path:
    """Keep a flat layout: dost[.exe], README.md and LICENSE, not a target/ path."""
    name = ARCHIVES[(system, arch)]
    if not binary.is_file() or binary.stat().st_size == 0:
        raise ValueError(f"missing or empty binary: {binary}")
    output.mkdir(parents=True, exist_ok=True)
    dest = output / name
    files = [(binary, "dost.exe" if system == "Windows" else "dost")]
    files += [(ROOT / file, file) for file in ("README.md", "LICENSE")]
    if system == "Windows":
        with zipfile.ZipFile(dest, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for source, member in files:
                archive.write(source, member)
    else:
        with tarfile.open(dest, "w:gz") as archive:
            for source, member in files:
                info = archive.gettarinfo(str(source), arcname=member)
                info.mode = 0o755 if member == "dost" else 0o644
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                with source.open("rb") as stream:
                    archive.addfile(info, stream)
    return dest


def finalize(output: Path, tag: str, version: str, notes: Path) -> Path:
    """Fail closed on missing/extra assets or a tag that disagrees with Cargo.toml."""
    if tag != f"v{version}":
        raise ValueError(f"tag {tag!r} does not match Cargo.toml version v{version}")
    actual = {file.name for file in output.iterdir()}
    if actual != set(ASSETS):
        raise ValueError(
            f"unexpected release assets; missing={sorted(set(ASSETS) - actual)}, "
            f"extra={sorted(actual - set(ASSETS))}"
        )
    lines = []
    for name in sorted(ASSETS):
        file = output / name
        if not file.is_file() or not file.stat().st_size:
            raise ValueError(f"missing or empty release asset: {name}")
        with file.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        lines.append(f"{digest}  {name}\n")
    manifest = output / f"dost-{version}-checksums.txt"
    manifest.write_text("".join(lines), encoding="utf-8")
    notes.write_text(release_notes(version), encoding="utf-8")
    return manifest


def release_notes(version: str) -> str:
    base = f"https://github.com/rytsh/dost/releases/download/v{version}"
    return f"""## Downloads

| Platform | Architecture | Download |
| --- | --- | --- |
| macOS | Apple Silicon + Intel | [dost.dmg]({base}/dost.dmg) |
| macOS | Apple Silicon ARM64 | [tar.gz]({base}/dost_Darwin_arm64.tar.gz) |
| macOS | Intel x86_64 | [tar.gz]({base}/dost_Darwin_x86_64.tar.gz) |
| Windows | Intel / AMD x86_64 | [ZIP]({base}/dost_Windows_x86_64.zip) |
| Windows | ARM64 | [ZIP]({base}/dost_Windows_arm64.zip) |
| Linux | x86_64 | [tar.gz]({base}/dost_Linux_x86_64.tar.gz) |
| Linux | ARM64 | [tar.gz]({base}/dost_Linux_arm64.tar.gz) |

**macOS:** open the universal DMG and drag Dost into Applications. For a smaller download,
extract the tar.gz for your Mac's architecture and move `Dost.app` into Applications.
The updater downloads only the architecture it is running as (Intel under Rosetta stays Intel).

**Windows:** extract the ZIP and run `dost.exe`. Unsigned builds may show a SmartScreen warning.

**Linux:** extract the archive and run `./dost`. Requires X11 or XWayland and a
StatusNotifierItem host for the tray icon.

### Verify your download

Download [dost-{version}-checksums.txt]({base}/dost-{version}-checksums.txt)
alongside the desired archive. The manifest covers all seven downloads, including the DMG.

```sh
# Linux: verify files you downloaded (ignore the other platforms).
sha256sum --check --ignore-missing dost-{version}-checksums.txt
# macOS: compare this hash with the dost.dmg entry in the manifest.
shasum -a 256 dost.dmg
```

```powershell
# Windows: compare with the matching entry in the manifest.
Get-FileHash .\\dost_Windows_x86_64.zip -Algorithm SHA256
```

> **Upgrading from the old asset layout:** releases using `dost-windows.exe`,
> `Dost-macos.zip` or `dost-linux-x86_64` cannot automatically install this layout.
> Download and install this version manually once; its updater understands the new archives.

"""


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    pack = commands.add_parser("package")
    pack.add_argument("--os", choices=("Windows", "Linux"), required=True)
    pack.add_argument("--arch", choices=("x86_64", "arm64"), required=True)
    pack.add_argument("--binary", type=Path, required=True)
    pack.add_argument("--output", type=Path, default=Path("dist"))
    finish = commands.add_parser("finalize")
    finish.add_argument("--tag", required=True)
    finish.add_argument("--output", type=Path, default=Path("dist"))
    finish.add_argument("--notes", type=Path, default=Path("release-notes.md"))
    args = parser.parse_args()
    if args.command == "package":
        print(package(args.os, args.arch, args.binary, args.output))
    else:
        version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
        print(finalize(args.output, args.tag, version, args.notes))


if __name__ == "__main__":
    main()
