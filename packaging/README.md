# Releases and signing

For the step-by-step setup guide in Turkish (Apple certificates, GitHub secrets and
Windows signing options), see [CI.md](../CI.md).

Push a tag matching the version in `Cargo.toml` (for example `v0.2.3` for `0.2.3`)
and CI builds, signs (when set up) and publishes:

| File | For |
| --- | --- |
| `dost.dmg` | macOS installer; universal Apple Silicon + Intel app |
| `dost_Darwin_arm64.tar.gz` | macOS Apple Silicon app and updater |
| `dost_Darwin_x86_64.tar.gz` | macOS Intel app and updater |
| `dost_Windows_x86_64.zip` | Windows Intel / AMD |
| `dost_Windows_arm64.zip` | Windows ARM64 |
| `dost_Linux_x86_64.tar.gz` | Linux x86_64 |
| `dost_Linux_arm64.tar.gz` | Linux ARM64 |
| `dost-<version>-checksums.txt` | SHA-256 of all seven downloads |

Windows and Linux archives have a flat layout: `dost.exe` / `dost`, `README.md`, `LICENSE`.
macOS tarballs contain `Dost.app` and are created with macOS `tar` after signing/notarization
to preserve bundle metadata. Each architecture-specific bundle is built and signed
independently; one temporary `ditto` ZIP contains the final signed DMG and both thin apps
(notarytool does not accept tar.gz). Tickets are stapled before creating the tarballs. CI extracts each delivered
tarball and checks its architecture, signature and, when notarized, stapled ticket and
Gatekeeper assessment. The updater also verifies the extracted app's signature before
replacing the installed app. The universal DMG is always named `dost.dmg`, without a
version suffix.

Windows and Linux build on native x86_64 and ARM64 GitHub-hosted runners. macOS builds
both targets on a Mac: separate bundles for the tarballs and a universal bundle for the DMG.
`scripts/release.py`
packages Windows/Linux archives and validates the complete asset set and tag/version
match before generating checksums and download notes. GitHub-generated changelog notes
are appended to the download instructions. Only the release job has `contents: write`.

Dost checks for a new release at start and once a day. With *Update automatically* (settings →
Connect) it downloads the new version, checks its SHA-256, swaps it in and asks to restart;
otherwise it only says a new version is out. The updater selects the archive for the running
binary's architecture, verifies the matching filename in the checksum manifest, and only
then installs it. Debug builds never replace themselves; macOS also requires an app bundle,
and Linux requires a writable executable and parent directory.

**Migration:** old versions expecting `dost-windows.exe`, `Dost-macos.zip`,
`dost-linux-x86_64` and individual `.sha256` files require one manual upgrade. Legacy
aliases are deliberately not published. The release notes call out this change.

## Signing

Branch/PR macOS builds are ad-hoc signed without Apple submissions. Release tags require
all five Apple secrets and fail rather than silently publishing an unsigned/unnotarized
macOS release. Windows signing remains optional; without its secrets the EXE is unsigned.

Add them in GitHub → Settings → Secrets and variables → Actions:

### macOS: Developer ID + notarization

Developer ID signing and notarization run **only for `v*` release tags**. Branch/PR artifacts
stay ad-hoc signed and do not submit anything to Apple. `scripts/notarize_macos.py` creates
one ZIP containing the final signed universal DMG (with its app inside), `arm64/Dost.app`
and `x86_64/Dost.app`. Apple processes nested containers, so all three outputs are submitted
once. Signed recovery inputs are uploaded **before submission**, then `submit --no-wait`
returns a UUID that is immediately uploaded separately. No private keys or credentials
are included in these artifacts; retention is 30 days.

The reusable/manual `.github/workflows/notarization.yml` waits for that UUID, downloads
Apple's log and requires matching CDHashes, paths and architectures for the DMG and all
four app slices. Only then does it staple the original DMG and both thin apps, verify
them, package and validate the delivered tarballs, and publish checksums/release files.
It never rebuilds, re-signs or submits. Source tag, SHA, run/attempt, successful platform
jobs and checkpoint checksums are verified before publishing. Apple status and log JSON
are saved as `notarization-results-<run>-<attempt>` artifacts, not release downloads.

No custom timeout is configured; GitHub-hosted jobs still have their six-hour platform
limit, but Apple keeps processing the saved UUID. To resume, run **Complete notarized
release** on `main` with the original tag CI's `source_run_id` and `source_attempt`
(usually `1`). Both checkpoint artifacts must still exist. This resumes the same UUID
and uses the original Windows/Linux artifacts, without another Apple submission. Do not
rerun the build job as a recovery shortcut; that makes a fresh submission. Old releases
without checkpoint artifacts cannot use this recovery path. See [CI.md](../CI.md) for details.

With these secrets CI signs `Dost.app` with the hardened runtime and `packaging/macos/Dost.entitlements`,
notarizes the dmg with `notarytool`, staples the ticket to the dmg and the app and checks the result
with `spctl`. Gatekeeper then opens Dost without the "unidentified developer" warning. Locally the
same is `make notarize SIGN_ID="Developer ID Application: …"` after
`xcrun notarytool store-credentials dost --apple-id … --team-id …`.

| Secret | Value |
| --- | --- |
| `MACOS_CERT_P12` | `base64 -i DeveloperID.p12` (the *Developer ID Application* certificate with its key) |
| `MACOS_CERT_PASSWORD` | The .p12 export password |
| `APPLE_ID` | Your Apple ID e-mail |
| `APPLE_TEAM_ID` | 10-character team id (developer.apple.com → Membership) |
| `APPLE_APP_PASSWORD` | An app-specific password (appleid.apple.com → Sign-In and Security) |

### Windows: code signing

The current workflow supports a PFX containing an exportable private key. Modern public
code-signing certificates commonly keep keys in hardware or a cloud HSM instead; those
require a provider-specific CI integration, not just these secrets. See [CI.md](../CI.md)
before purchasing a certificate. A valid signature does not guarantee immediate removal
of SmartScreen warnings.

| Secret | Value |
| --- | --- |
| `WINDOWS_CERT_PFX` | `base64 -w0 cert.pfx` |
| `WINDOWS_CERT_PASSWORD` | The .pfx password |
