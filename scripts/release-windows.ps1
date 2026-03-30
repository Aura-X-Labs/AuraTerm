param(
    [switch]$Store,
    [switch]$Msix,
    [switch]$MsixUpload
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
    $selectedModes = @($Store, $Msix, $MsixUpload) | Where-Object { $_ }
    if ($selectedModes.Count -gt 1) {
        throw 'Choose only one Windows packaging mode at a time: standard, -Store, -Msix, or -MsixUpload.'
    }

    if ($Store) {
        Write-Host 'Building Microsoft Store Windows package...'
        npm run tauri:store
    }
    elseif ($MsixUpload) {
        Write-Host 'Building MSIX upload package...'
        powershell -NoProfile -ExecutionPolicy Bypass -File scripts/package-msix.ps1 -MsixUpload
    }
    elseif ($Msix) {
        Write-Host 'Building MSIX package...'
        powershell -NoProfile -ExecutionPolicy Bypass -File scripts/package-msix.ps1
    }
    else {
        Write-Host 'Building standard Windows package...'
        npm run tauri build
    }
}
finally {
    Pop-Location
}