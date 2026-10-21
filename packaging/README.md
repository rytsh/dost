# Releases and signing

Push a tag like `v0.2.0` and CI builds, signs (when set up) and publishes:

| File | For |
| --- | --- |
| `Dost.dmg` | People installing on macOS |
| `Dost-macos.zip` + `.sha256` | Dost's own updater (macOS) |
| `dost-windows.exe` + `.sha256` | Windows (download and updater) |

Dost checks for a new release at start and once a day. With *Update automatically* (settings →
Connect) it downloads the new version, checks its SHA-256, swaps it in and asks to restart;
otherwise it only says a new version is out. Builds from source (debug, or not inside `Dost.app`)
never replace themselves.

## Signing (optional)

Without these secrets CI still works: the macOS app is ad-hoc signed and the Windows exe is
unsigned (users see a Gatekeeper / SmartScreen warning the first time).

Add them in GitHub → Settings → Secrets and variables → Actions:

### macOS: Developer ID + notarization

| Secret | Value |
| --- | --- |
| `MACOS_CERT_P12` | `base64 -i DeveloperID.p12` (the *Developer ID Application* certificate with its key) |
| `MACOS_CERT_PASSWORD` | The .p12 export password |
| `APPLE_ID` | Your Apple ID e-mail |
| `APPLE_TEAM_ID` | 10-character team id (developer.apple.com → Membership) |
| `APPLE_APP_PASSWORD` | An app-specific password (appleid.apple.com → Sign-In and Security) |

### Windows: code signing

| Secret | Value |
| --- | --- |
| `WINDOWS_CERT_PFX` | `base64 -w0 cert.pfx` |
| `WINDOWS_CERT_PASSWORD` | The .pfx password |
