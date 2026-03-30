# Windows Release Guide

This document describes how AuraTerm packages the standard Windows installer, the Microsoft Store installer, and the MSIX/MSIXUPLOAD artifacts used for Partner Center uploads.

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

### MSIX Package Build

Use the dedicated MSIX packaging script:

```powershell
npm run package:msix
```

This command:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/package-msix.ps1
```

It builds the unpackaged Windows binary if needed, stages a desktop-app `AppxManifest.xml`, generates logo assets from `src-tauri/icons/icon.png`, and then runs `makeappx.exe` to produce a standalone `.msix` file.

The generated MSIX is intentionally unsigned. For Store submission, Microsoft applies the production signature during ingestion.

Output files are written to `src-tauri/target/release/bundle/msix/`.

### MSIX Upload Build

To produce a Store-uploadable `.msixupload` file, run:

```powershell
npm run package:msixupload
```

This wraps the generated `.msix` together with an optional `.appxsym` symbol archive into a `.msixupload` file, which is the format Microsoft recommends for Partner Center uploads.

Local wrapper commands are also available:

```powershell
npm run release:windows:msix
npm run release:windows:msixupload
```

## Signing Behavior

The repository no longer depends on a local Windows signing certificate for MSIX packaging.

Behavior:

- `npm run package:msix` and `npm run package:msixupload` produce unsigned artifacts for Partner Center upload
- GitHub CI and GitHub release builds also produce unsigned MSIX artifacts
- Microsoft Store signs the uploaded package during the Store ingestion flow
- Standard Tauri Windows bundles are also produced unsigned unless you add signing back later

Required tools:

- `makeappx.exe` from the Windows SDK for MSIX packaging

## Environment Variables

Then run one of the packaging commands:

```powershell
npm run tauri build
npm run tauri:store
npm run package:msix
npm run package:msixupload
```

Or use the local Windows release wrappers:

```powershell
npm run release:windows
npm run release:windows:store
npm run release:windows:msix
npm run release:windows:msixupload
```

## MSIX Identity Variables

The MSIX packaging script now defaults to the Product Identity in [docs/Product-Identity.md](../docs/Product-Identity.md):

- `Package/Identity/Name = AuraXLabs.AuraTerm`
- `Package/Identity/Publisher = CN=671C654E-E6B4-48F6-9D75-058B100AA46A`
- `Package/Properties/PublisherDisplayName = Aura X Labs`

You can still override these values in CI or locally if needed:

- `AURATERM_MSIX_PACKAGE_NAME`: Package `Identity Name`; set this to the exact reserved package identity name in Partner Center
- `AURATERM_MSIX_PUBLISHER`: Package `Identity Publisher`; for Store uploads this should match the reserved Store identity
- `AURATERM_MSIX_PUBLISHER_DISPLAY_NAME`: Store-facing publisher display name
- `AURATERM_MSIX_DISPLAY_NAME`: Store-facing app display name
- `AURATERM_MSIX_DESCRIPTION`: Manifest description
- `AURATERM_MSIX_VERSION`: Optional dot-quad override such as `0.2.5.0`
- `AURATERM_MSIX_MIN_VERSION`: Default `10.0.19041.0`
- `AURATERM_MSIX_MAX_VERSION_TESTED`: Default `10.0.26100.0`

In the normal Store flow you should not need any extra signing variables.

## GitHub Actions

Configure these repository variables for both [ci.yml](../.github/workflows/ci.yml) and [release.yml](../.github/workflows/release.yml) when publishing Store-ready MSIX artifacts:

- `AURATERM_MSIX_PACKAGE_NAME`
- `AURATERM_MSIX_PUBLISHER`
- `AURATERM_MSIX_PUBLISHER_DISPLAY_NAME`
- `AURATERM_MSIX_DISPLAY_NAME`
- `AURATERM_MSIX_DESCRIPTION`
- `AURATERM_MSIX_MIN_VERSION`
- `AURATERM_MSIX_MAX_VERSION_TESTED`

The CI workflow now also uploads `*.msix`, `*.msixupload`, and `*.appxsym` as Windows workflow artifacts. The tag-based release workflow builds the same files and attaches them to the GitHub release draft.

## Notes

- Microsoft Store submission does not require you to locally sign the uploaded MSIX when the Store is responsible for final signing
- MSIX packages for desktop apps require the `runFullTrust` restricted capability, so Partner Center will ask you to justify that during review
- Standard Windows releases can be built unsigned, but users may see SmartScreen warnings
- The MSIX package does not run the NSIS hook that adds the Explorer context-menu entry, so that Windows integration remains specific to the NSIS installer
