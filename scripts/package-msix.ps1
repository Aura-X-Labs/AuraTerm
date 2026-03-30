param(
    [string]$Target = 'x86_64-pc-windows-msvc',
    [switch]$SkipBuild,
    [switch]$MsixUpload
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Get-JsonConfig {
    param([string]$Path)

    return Get-Content -Path $Path -Raw | ConvertFrom-Json
}

$defaultMsixPackageName = 'AuraXLabs.AuraTerm'
$defaultMsixPublisher = 'CN=671C654E-E6B4-48F6-9D75-058B100AA46A'
$defaultMsixPublisherDisplayName = 'Aura X Labs'

function Get-OptionalPropertyValue {
    param(
        [object]$Object,
        [string]$PropertyName,
        $DefaultValue = $null
    )

    if ($null -eq $Object) {
        return $DefaultValue
    }

    $property = $Object.PSObject.Properties[$PropertyName]
    if ($property) {
        return $property.Value
    }

    return $DefaultValue
}

function Convert-ToDotQuadVersion {
    param([string]$Version)

    if ($Version -notmatch '^(\d+)\.(\d+)\.(\d+)(?:\.(\d+))?$') {
        throw "Unsupported version format '$Version'. Expected semver like 1.2.3 or 1.2.3.4."
    }

    $revision = if ($Matches[4]) { $Matches[4] } else { '0' }
    return "{0}.{1}.{2}.{3}" -f $Matches[1], $Matches[2], $Matches[3], $revision
}

function Get-ProcessorArchitecture {
    param([string]$RustTarget)

    switch ($RustTarget) {
        'x86_64-pc-windows-msvc' { return 'x64' }
        'i686-pc-windows-msvc' { return 'x86' }
        'aarch64-pc-windows-msvc' { return 'arm64' }
        default { throw "Unsupported Windows target '$RustTarget' for MSIX packaging." }
    }
}

function Get-ReleaseDirectory {
    param(
        [string]$RepoRoot,
        [string]$RustTarget
    )

    if ($RustTarget -eq 'x86_64-pc-windows-msvc') {
        return Join-Path $RepoRoot 'src-tauri\target\release'
    }

    return Join-Path $RepoRoot ("src-tauri\\target\\{0}\\release" -f $RustTarget)
}

function Get-BuildReleaseDirectoryCandidates {
    param(
        [string]$RepoRoot,
        [string]$RustTarget
    )

    $candidates = [System.Collections.Generic.List[string]]::new()

    if ($env:CARGO_TARGET_DIR) {
        $cargoTargetDir = $env:CARGO_TARGET_DIR
        if (-not [System.IO.Path]::IsPathRooted($cargoTargetDir)) {
            $cargoTargetDir = Join-Path $RepoRoot $cargoTargetDir
        }

        $candidates.Add((Join-Path (Join-Path $cargoTargetDir $RustTarget) 'release'))
        if ($RustTarget -eq 'x86_64-pc-windows-msvc') {
            $candidates.Add((Join-Path $cargoTargetDir 'release'))
        }
    }

    $candidates.Add((Join-Path $RepoRoot ("src-tauri\\target\\{0}\\release" -f $RustTarget)))
    if ($RustTarget -eq 'x86_64-pc-windows-msvc') {
        $candidates.Add((Join-Path $RepoRoot 'src-tauri\target\release'))
    }

    return $candidates | Select-Object -Unique
}

function Get-WindowsSdkTool {
    param([string]$ToolName)

    $command = Get-Command $ToolName -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    $roots = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'),
        (Join-Path $env:ProgramFiles 'Windows Kits\10\bin')
    ) | Where-Object { $_ -and (Test-Path $_) }

    foreach ($root in $roots) {
        $tool = Get-ChildItem -Path $root -Filter $ToolName -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\' } |
            Sort-Object FullName -Descending |
            Select-Object -First 1

        if ($tool) {
            return $tool.FullName
        }
    }

    throw "Unable to find $ToolName. Install the Windows SDK build tools on this machine."
}

function Convert-ToPackageString {
    param(
        [string]$Value,
        [string]$Fallback
    )

    $sanitized = ($Value -replace '[^A-Za-z0-9.-]', '-')
    $sanitized = $sanitized.Trim('.')
    if (-not $sanitized -or $sanitized -notmatch '^[A-Za-z0-9]') {
        $sanitized = $Fallback
    }

    return $sanitized
}

function Convert-ToApplicationId {
    param([string]$Value)

    $parts = ($Value -replace '[^A-Za-z0-9.]', '.') -split '\.' | Where-Object { $_ }
    if (-not $parts) {
        return 'AuraTerm'
    }

    $normalized = foreach ($part in $parts) {
        if ($part -match '^[A-Za-z]') {
            $part
        }
        else {
            "A$part"
        }
    }

    return ($normalized -join '.')
}

function New-Directory {
    param([string]$Path)

    if (Test-Path $Path) {
        Remove-Item -Path $Path -Recurse -Force
    }

    New-Item -ItemType Directory -Path $Path | Out-Null
}

function Resize-Png {
    param(
        [string]$Source,
        [string]$Destination,
        [int]$Width,
        [int]$Height
    )

    Add-Type -AssemblyName System.Drawing

    $sourceBitmap = [System.Drawing.Bitmap]::FromFile($Source)
    $resizedBitmap = New-Object System.Drawing.Bitmap $Width, $Height
    $graphics = [System.Drawing.Graphics]::FromImage($resizedBitmap)

    try {
        $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $graphics.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
        $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
        $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $graphics.Clear([System.Drawing.Color]::Transparent)
        $graphics.DrawImage($sourceBitmap, 0, 0, $Width, $Height)
        $resizedBitmap.Save($Destination, [System.Drawing.Imaging.ImageFormat]::Png)
    }
    finally {
        $graphics.Dispose()
        $resizedBitmap.Dispose()
        $sourceBitmap.Dispose()
    }
}

function Write-GitHubOutputValue {
    param(
        [string]$Name,
        [string]$Value
    )

    if ($env:GITHUB_OUTPUT) {
        "$Name=$Value" | Out-File -FilePath $env:GITHUB_OUTPUT -Encoding utf8 -Append
    }
}

$repoRoot = Split-Path -Path $PSScriptRoot -Parent
Push-Location $repoRoot

try {
    $tauriConfig = Get-JsonConfig -Path (Join-Path $repoRoot 'src-tauri\tauri.conf.json')
    $storeConfig = Get-JsonConfig -Path (Join-Path $repoRoot 'src-tauri\tauri.microsoftstore.conf.json')

    $processorArchitecture = Get-ProcessorArchitecture -RustTarget $Target
    $releaseDir = Get-ReleaseDirectory -RepoRoot $repoRoot -RustTarget $Target
    $buildReleaseDirCandidates = Get-BuildReleaseDirectoryCandidates -RepoRoot $repoRoot -RustTarget $Target
    $mainBinaryName = Get-OptionalPropertyValue -Object $tauriConfig -PropertyName 'mainBinaryName'
    $executableName = if ($mainBinaryName) { "$mainBinaryName.exe" } else { 'auraterm.exe' }
    $executablePathCandidates = $buildReleaseDirCandidates | ForEach-Object { Join-Path $_ $executableName }
    $executablePath = $null
    $pdbPath = $null

    if (-not $SkipBuild) {
        Write-Host "Building unpackaged Windows binary for $Target..."
        & npm.cmd 'run' 'tauri' 'build' '--' '--no-bundle' '--target' $Target
        if ($LASTEXITCODE -ne 0) {
            throw 'Tauri build failed while preparing the MSIX package.'
        }
    }

    foreach ($candidate in $executablePathCandidates) {
        if (Test-Path $candidate) {
            $executablePath = $candidate
            $pdbPath = [System.IO.Path]::ChangeExtension($candidate, '.pdb')
            break
        }
    }

    if (-not $executablePath) {
        $candidateList = $executablePathCandidates -join [Environment]::NewLine
        throw "Expected Windows executable was not found. Searched:`n$candidateList"
    }

    $packageNameSource = if ($env:AURATERM_MSIX_PACKAGE_NAME) {
        $env:AURATERM_MSIX_PACKAGE_NAME
    }
    else {
        $defaultMsixPackageName
    }
    $packageName = Convert-ToPackageString -Value $packageNameSource -Fallback 'AuraTerm'

    $appIdSource = if ($env:AURATERM_MSIX_APP_ID) {
        $env:AURATERM_MSIX_APP_ID
    }
    else {
        $tauriConfig.productName
    }
    $appId = Convert-ToApplicationId -Value $appIdSource
    $packageVersion = if ($env:AURATERM_MSIX_VERSION) { $env:AURATERM_MSIX_VERSION } else { Convert-ToDotQuadVersion -Version $tauriConfig.version }
    $displayName = if ($env:AURATERM_MSIX_DISPLAY_NAME) { $env:AURATERM_MSIX_DISPLAY_NAME } else { $tauriConfig.productName }
    $storeBundle = Get-OptionalPropertyValue -Object $storeConfig -PropertyName 'bundle'
    $storePublisher = Get-OptionalPropertyValue -Object $storeBundle -PropertyName 'publisher'
    $storeShortDescription = Get-OptionalPropertyValue -Object $storeBundle -PropertyName 'shortDescription'
    $publisherDisplayName = if ($env:AURATERM_MSIX_PUBLISHER_DISPLAY_NAME) { $env:AURATERM_MSIX_PUBLISHER_DISPLAY_NAME } elseif ($storePublisher) { $storePublisher } else { $defaultMsixPublisherDisplayName }
    $description = if ($env:AURATERM_MSIX_DESCRIPTION) { $env:AURATERM_MSIX_DESCRIPTION } elseif ($storeShortDescription) { $storeShortDescription } else { "$displayName for Windows" }
    $backgroundColor = if ($env:AURATERM_MSIX_BACKGROUND_COLOR) { $env:AURATERM_MSIX_BACKGROUND_COLOR } else { '#1D1D1D' }
    $resourceLanguage = if ($env:AURATERM_MSIX_RESOURCE_LANGUAGE) { $env:AURATERM_MSIX_RESOURCE_LANGUAGE } else { 'en-US' }
    $minVersion = if ($env:AURATERM_MSIX_MIN_VERSION) { $env:AURATERM_MSIX_MIN_VERSION } else { '10.0.19041.0' }
    $maxVersionTested = if ($env:AURATERM_MSIX_MAX_VERSION_TESTED) { $env:AURATERM_MSIX_MAX_VERSION_TESTED } else { '10.0.26100.0' }
    $publisher = if ($env:AURATERM_MSIX_PUBLISHER) { $env:AURATERM_MSIX_PUBLISHER } else { $defaultMsixPublisher }

    $outputDir = if ($env:AURATERM_MSIX_OUTPUT_DIR) {
        $env:AURATERM_MSIX_OUTPUT_DIR
    }
    else {
        Join-Path $releaseDir 'bundle\msix'
    }

    $stagingDir = Join-Path $outputDir 'package'
    $imagesDir = Join-Path $stagingDir 'Images'
    $packageBaseName = '{0}_{1}_{2}' -f $packageName, $packageVersion, $processorArchitecture
    $msixPath = Join-Path $outputDir ("$packageBaseName.msix")
    $msixUploadPath = Join-Path $outputDir ("$packageBaseName.msixupload")
    $appxSymPath = Join-Path $outputDir ("$packageBaseName.appxsym")

    New-Directory -Path $outputDir
    New-Directory -Path $stagingDir
    New-Item -ItemType Directory -Path $imagesDir | Out-Null

    $iconSource = Join-Path $repoRoot 'src-tauri\icons\icon.png'
    if (-not (Test-Path $iconSource)) {
        throw "MSIX asset source icon is missing at $iconSource"
    }

    Resize-Png -Source $iconSource -Destination (Join-Path $imagesDir 'Square44x44Logo.png') -Width 44 -Height 44
    Resize-Png -Source $iconSource -Destination (Join-Path $imagesDir 'Square150x150Logo.png') -Width 150 -Height 150
    Resize-Png -Source $iconSource -Destination (Join-Path $imagesDir 'StoreLogo.png') -Width 50 -Height 50

    Copy-Item -Path $executablePath -Destination (Join-Path $stagingDir $executableName)

    $manifestPath = Join-Path $stagingDir 'AppxManifest.xml'
    $manifestContent = @"
<?xml version="1.0" encoding="utf-8"?>
<Package
  xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
  xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
  xmlns:uap10="http://schemas.microsoft.com/appx/manifest/uap/windows10/10"
  xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
    IgnorableNamespaces="uap uap10 rescap">
  <Identity Name="$packageName" Publisher="$publisher" Version="$packageVersion" ProcessorArchitecture="$processorArchitecture" />
  <Properties>
    <DisplayName>$displayName</DisplayName>
    <PublisherDisplayName>$publisherDisplayName</PublisherDisplayName>
    <Description>$description</Description>
    <Logo>Images\StoreLogo.png</Logo>
  </Properties>
  <Resources>
    <Resource Language="$resourceLanguage" />
  </Resources>
  <Dependencies>
    <TargetDeviceFamily Name="Windows.Desktop" MinVersion="$minVersion" MaxVersionTested="$maxVersionTested" />
  </Dependencies>
  <Capabilities>
    <rescap:Capability Name="runFullTrust" />
  </Capabilities>
  <Applications>
    <Application Id="$appId"
                 Executable="$executableName"
                 uap10:RuntimeBehavior="packagedClassicApp"
                                 uap10:TrustLevel="mediumIL">
      <uap:VisualElements
        DisplayName="$displayName"
        Description="$description"
        BackgroundColor="$backgroundColor"
        Square150x150Logo="Images\Square150x150Logo.png"
        Square44x44Logo="Images\Square44x44Logo.png" />
    </Application>
  </Applications>
</Package>
"@
    Set-Content -Path $manifestPath -Value $manifestContent -Encoding utf8

    $makeAppx = Get-WindowsSdkTool -ToolName 'makeappx.exe'
    Write-Host "Packing MSIX with $makeAppx"
    & $makeAppx pack /d $stagingDir /p $msixPath /o
    if ($LASTEXITCODE -ne 0) {
        throw 'MakeAppx failed while creating the MSIX package.'
    }

    Write-Host 'MSIX package was created unsigned. Microsoft Store will apply production signing during ingestion.'

    if (Test-Path $pdbPath) {
        $tempSymZip = Join-Path $outputDir 'symbols.zip'
        if (Test-Path $tempSymZip) {
            Remove-Item -Path $tempSymZip -Force
        }
        Compress-Archive -LiteralPath $pdbPath -DestinationPath $tempSymZip -Force
        if (Test-Path $appxSymPath) {
            Remove-Item -Path $appxSymPath -Force
        }
        Move-Item -Path $tempSymZip -Destination $appxSymPath
    }

    if ($MsixUpload) {
        $uploadContentsDir = Join-Path $outputDir 'upload'
        New-Directory -Path $uploadContentsDir
        Copy-Item -Path $msixPath -Destination $uploadContentsDir
        if (Test-Path $appxSymPath) {
            Copy-Item -Path $appxSymPath -Destination $uploadContentsDir
        }

        $tempUploadZip = Join-Path $outputDir 'upload.zip'
        if (Test-Path $tempUploadZip) {
            Remove-Item -Path $tempUploadZip -Force
        }
        Compress-Archive -Path (Join-Path $uploadContentsDir '*') -DestinationPath $tempUploadZip -Force
        if (Test-Path $msixUploadPath) {
            Remove-Item -Path $msixUploadPath -Force
        }
        Move-Item -Path $tempUploadZip -Destination $msixUploadPath
        Remove-Item -Path $uploadContentsDir -Recurse -Force
    }

    Remove-Item -Path $stagingDir -Recurse -Force

    Write-Host "MSIX artifact: $msixPath"
    if (Test-Path $appxSymPath) {
        Write-Host "Symbol artifact: $appxSymPath"
    }
    if ($MsixUpload) {
        Write-Host "Upload artifact: $msixUploadPath"
    }

    Write-GitHubOutputValue -Name 'msix_path' -Value $msixPath
    Write-GitHubOutputValue -Name 'appxsym_path' -Value $appxSymPath
    if ($MsixUpload) {
        Write-GitHubOutputValue -Name 'msixupload_path' -Value $msixUploadPath
    }
}
finally {
    Pop-Location
}