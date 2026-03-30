# Windows Release Guide

This document describes how AuraTerm packages the standard Windows installer and the Microsoft Store installer, and how Windows signing is applied.

## Build Variants

### Standard Windows Build

Use the default Tauri configuration:

```bash
npm run tauri build
```

Output files are written to `src-tauri/target/release/bundle/`.

This build uses [src-tauri/tauri.conf.json](../src-tauri/tauri.conf.json).

### Microsoft Store Build

Use the Store-specific override configuration:

```bash
npm run tauri:store
```

This command runs:

```bash
npm run sync:version && tauri build --no-bundle && tauri bundle --config src-tauri/tauri.microsoftstore.conf.json
```

The Store build uses [src-tauri/tauri.microsoftstore.conf.json](../src-tauri/tauri.microsoftstore.conf.json), which currently overrides:

- Store metadata such as publisher and descriptions
- `bundle.windows.webviewInstallMode = offlineInstaller`

The offline WebView2 installer is required for Microsoft Store distribution.

## Signing Behavior

Windows signing is configured in [src-tauri/tauri.conf.json](../src-tauri/tauri.conf.json) via `bundle.windows.signCommand`.

Behavior:

- If `AURATERM_WINDOWS_CERT_THUMBPRINT` is set, Tauri signs Windows bundles with `signtool.exe`
- If `AURATERM_WINDOWS_CERT_THUMBPRINT` is not set, signing is skipped and the build remains unsigned
- `AURATERM_WINDOWS_DIGEST_ALGORITHM` defaults to `sha256` when unset
- `AURATERM_WINDOWS_TIMESTAMP_URL` is optional but strongly recommended

Required tools:

- A valid Windows code signing certificate already imported into `Cert:\CurrentUser\My`
- `signtool.exe` available in `PATH`

## Environment Variables

Set these in PowerShell before building signed Windows packages:

```powershell
$env:AURATERM_WINDOWS_CERT_THUMBPRINT = "YOUR_CERT_THUMBPRINT"
$env:AURATERM_WINDOWS_DIGEST_ALGORITHM = "sha256"
$env:AURATERM_WINDOWS_TIMESTAMP_URL = "http://timestamp.digicert.com"
```

If you prefer importing a `.pfx` file automatically for local releases, set these instead:

```powershell
$env:AURATERM_WINDOWS_PFX_PATH = "C:\path\to\certificate.pfx"
$env:AURATERM_WINDOWS_PFX_PASSWORD = "YOUR_PFX_PASSWORD"
```

Then run one of the packaging commands:

```powershell
npm run tauri build
npm run tauri:store
```

Or use the local Windows release wrappers, which reuse the same signing environment variables and import the `.pfx` automatically when needed:

```powershell
npm run release:windows
npm run release:windows:store
```

## GitHub Actions

The release workflow imports the Windows certificate and maps it into the same runtime variables used locally.

Configure these repository secrets for [release.yml](../.github/workflows/release.yml):

- `AURATERM_WINDOWS_PFX_BASE64`: Base64-encoded `.pfx` certificate content
- `AURATERM_WINDOWS_PFX_PASSWORD`: Password for the `.pfx` file
- `AURATERM_WINDOWS_DIGEST_ALGORITHM`: Optional, defaults to `sha256`
- `AURATERM_WINDOWS_TIMESTAMP_URL`: Optional, defaults to `http://timestamp.digicert.com`

At workflow runtime, the Windows job imports the certificate into `Cert:\CurrentUser\My`, extracts the thumbprint, and sets:

- `AURATERM_WINDOWS_CERT_THUMBPRINT`
- `AURATERM_WINDOWS_DIGEST_ALGORITHM`
- `AURATERM_WINDOWS_TIMESTAMP_URL`

## Certificate Import Example

If you already have a `.pfx` certificate file, you can import it into the current user certificate store:

```powershell
$password = ConvertTo-SecureString -String "YOUR_PFX_PASSWORD" -Force -AsPlainText
Import-PfxCertificate -FilePath .\certificate.pfx -CertStoreLocation Cert:\CurrentUser\My -Password $password
```

After import, open `certmgr.msc`, locate the certificate under `Personal > Certificates`, and copy its thumbprint.

## Notes

- Microsoft Store submission requires signed installers
- Standard Windows releases can be built unsigned, but users may see SmartScreen warnings
- If your certificate provider requires a different timestamp flow than RFC 3161 `/tr`, adjust `bundle.windows.signCommand` accordingly
