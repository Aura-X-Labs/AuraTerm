param(
    [switch]$Store
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Windows-only check
if ($PSVersionTable.PSVersion.Major -lt 6 -and -not $env:OS.Contains("Windows")) {
    throw 'scripts/release-windows.ps1 must be run on Windows.'
}

$repoRoot = Split-Path -Path $PSScriptRoot -Parent
Push-Location $repoRoot

try {
    if (-not $env:AURATERM_WINDOWS_DIGEST_ALGORITHM) {
        $env:AURATERM_WINDOWS_DIGEST_ALGORITHM = 'sha256'
    }

    if (-not $env:AURATERM_WINDOWS_TIMESTAMP_URL) {
        $env:AURATERM_WINDOWS_TIMESTAMP_URL = 'http://timestamp.digicert.com'
    }

    if (-not $env:AURATERM_WINDOWS_CERT_THUMBPRINT) {
        if ($env:AURATERM_WINDOWS_PFX_PATH -and $env:AURATERM_WINDOWS_PFX_PASSWORD) {
            $pfxPath = (Resolve-Path $env:AURATERM_WINDOWS_PFX_PATH).Path
            $password = ConvertTo-SecureString -String $env:AURATERM_WINDOWS_PFX_PASSWORD -Force -AsPlainText
            $certificate = Import-PfxCertificate -FilePath $pfxPath -CertStoreLocation Cert:\CurrentUser\My -Password $password

            if (-not $certificate) {
                throw 'Failed to import the Windows signing certificate from AURATERM_WINDOWS_PFX_PATH.'
            }

            $env:AURATERM_WINDOWS_CERT_THUMBPRINT = $certificate.Thumbprint
            Write-Host "Imported Windows signing certificate: $($certificate.Thumbprint)"
        }
        else {
            Write-Host 'No signing certificate configured. Build will continue unsigned.'
        }
    }

    if ($env:AURATERM_WINDOWS_CERT_THUMBPRINT) {
        $signtool = Get-Command signtool.exe -ErrorAction SilentlyContinue
        if (-not $signtool) {
            throw 'signtool.exe was not found in PATH. Install the Windows SDK signing tools or add signtool.exe to PATH.'
        }
    }

    if ($Store) {
        Write-Host 'Building Microsoft Store Windows package...'
        npm run tauri:store
    }
    else {
        Write-Host 'Building standard Windows package...'
        npm run tauri build
    }
}
finally {
    Pop-Location
}