param(
    [string]$BaseUrl = $(if ($env:OSCIRIS_BASE_URL) { $env:OSCIRIS_BASE_URL } else { "https://oscirislabs.com" }),
    [string]$WorkRoot = $(if ($env:OSCIRIS_WORK_ROOT) { $env:OSCIRIS_WORK_ROOT } else { Join-Path $env:TEMP "osciris-client" }),
    [string]$InstallDir = $(if ($env:OSCIRIS_INSTALL_DIR) { $env:OSCIRIS_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "OSCIRIS\bin" })
)

$ErrorActionPreference = "Stop"

$BinName = "osciris-node.exe"
$BinPath = Join-Path $InstallDir $BinName
$PlatformKey = "windows-x86_64"

New-Item -ItemType Directory -Force -Path $WorkRoot, $InstallDir | Out-Null

function Get-JsonFromUrl {
    param([string]$Url)
    return Invoke-RestMethod -Uri $Url -Headers @{ "User-Agent" = "osciris-windows-bootstrap/0.1" }
}

function Assert-SafeAsset {
    param($Asset)

    if (-not $Asset.filename) {
        throw "selected beta asset is missing filename"
    }

    $filename = [string]$Asset.filename
    if ($filename -ne [System.IO.Path]::GetFileName($filename) -or
        $filename.Contains("/") -or
        $filename.Contains("\") -or
        $filename.Contains("..") -or
        -not ($filename -match '^osciris-node-[A-Za-z0-9_.-]+\.zip$')) {
        throw "selected beta asset filename is not safe: ${filename}"
    }

    if (-not $Asset.sha256 -or -not ([string]$Asset.sha256 -match '^[0-9a-fA-F]{64}$')) {
        throw "selected beta asset ${filename} is missing a valid SHA-256 checksum"
    }
}

function Install-ReleaseAsset {
    $manifestUrl = "$($BaseUrl.TrimEnd('/'))/beta-release-manifest.json"
    $manifest = Get-JsonFromUrl -Url $manifestUrl
    $asset = @($manifest.assets | Where-Object { $_.platform -eq $PlatformKey }) | Select-Object -First 1

    if (-not $asset) {
        $available = @($manifest.assets | ForEach-Object { $_.platform }) -join ", "
        throw "beta manifest does not list a downloadable asset for ${PlatformKey}; available platforms: ${available}"
    }
    Assert-SafeAsset -Asset $asset

    $tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $tempDir | Out-Null
    try {
        $archivePath = Join-Path $tempDir "release-asset.zip"
        Invoke-WebRequest -Uri $asset.url -OutFile $archivePath -Headers @{ "User-Agent" = "osciris-windows-bootstrap/0.1" }

        $actual = (Get-FileHash -Algorithm SHA256 -Path $archivePath).Hash.ToLowerInvariant()
        $expected = [string]$asset.sha256
        if ($actual -ne $expected.ToLowerInvariant()) {
            throw "release asset checksum mismatch for $($asset.filename): expected $expected, actual $actual"
        }

        Expand-Archive -Path $archivePath -DestinationPath $tempDir -Force
        $extractedBin = Join-Path $tempDir $BinName
        if (-not (Test-Path $extractedBin)) {
            throw "release archive does not contain ${BinName}"
        }

        Copy-Item -Force -Path $extractedBin -Destination $BinPath
    }
    finally {
        Remove-Item -Recurse -Force -Path $tempDir -ErrorAction SilentlyContinue
    }
}

if (Get-Command "osciris-node" -ErrorAction SilentlyContinue) {
    $RunBin = (Get-Command "osciris-node").Source
}
else {
    Install-ReleaseAsset
    $RunBin = $BinPath
}

& $RunBin network sync-published --work-root $WorkRoot --base-url $BaseUrl
& $RunBin network check-updates --work-root $WorkRoot --base-url $BaseUrl

Write-Host "OSCIRIS collaborator bootstrap complete."
Write-Host "Binary: $RunBin"
Write-Host "Work root: $WorkRoot"
