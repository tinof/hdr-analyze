# HDR-Analyze Suite Installer for Windows
# Usage: irm https://github.com/tinof/hdr-analyze/releases/latest/download/install.ps1 | iex

$ErrorActionPreference = 'Stop'

$Repo = "tinof/hdr-analyze"
$InstallDir = if ($env:INSTALL_DIR) { $env:INSTALL_DIR } else { "$env:LOCALAPPDATA\Programs\hdr-analyze" }

function Write-Info { param($Message) Write-Host "[INFO] " -ForegroundColor Blue -NoNewline; Write-Host $Message }
function Write-Success { param($Message) Write-Host "[OK] " -ForegroundColor Green -NoNewline; Write-Host $Message }
function Write-Warn { param($Message) Write-Host "[WARN] " -ForegroundColor Yellow -NoNewline; Write-Host $Message }
function Write-Error { param($Message) Write-Host "[ERROR] " -ForegroundColor Red -NoNewline; Write-Host $Message; exit 1 }

function Get-LatestVersion {
    try {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
        return $release.tag_name
    }
    catch {
        Write-Error "Failed to fetch latest version: $_"
    }
}

function Install-HdrAnalyze {
    Write-Host ""
    Write-Host "  HDR-Analyze Suite Installer" -ForegroundColor Cyan
    Write-Host "  ===========================" -ForegroundColor Cyan
    Write-Host ""

    # Get version
    $version = Get-LatestVersion
    Write-Info "Latest version: $version"

    # Determine target
    $target = "x86_64-pc-windows-msvc"
    Write-Info "Target platform: $target"

    # Build download URL
    $archiveName = "hdr-analyze-$version-$target.zip"
    $downloadUrl = "https://github.com/$Repo/releases/download/$version/$archiveName"

    # Create temp directory
    $tempDir = Join-Path $env:TEMP "hdr-analyze-install-$(Get-Random)"
    New-Item -ItemType Directory -Path $tempDir -Force | Out-Null

    try {
        # Download
        $archivePath = Join-Path $tempDir $archiveName
        Write-Info "Downloading $archiveName..."
        Invoke-WebRequest -Uri $downloadUrl -OutFile $archivePath -UseBasicParsing

        # Extract
        Write-Info "Extracting archive..."
        Expand-Archive -Path $archivePath -DestinationPath $tempDir -Force

        # Create install directory
        if (-not (Test-Path $InstallDir)) {
            New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
        }

        # Find and copy binaries
        $binDir = Join-Path $tempDir "hdr-analyze-$version-$target" "bin"
        if (-not (Test-Path $binDir)) {
            Write-Error "Binary directory not found in archive"
        }

        Write-Info "Installing to $InstallDir..."
        foreach ($binary in @("hdr_analyzer_mvp.exe", "mkvdolby.exe", "verifier.exe")) {
            $srcPath = Join-Path $binDir $binary
            if (Test-Path $srcPath) {
                Copy-Item -Path $srcPath -Destination $InstallDir -Force
                Write-Success "Installed: $binary"
            }
        }
    }
    finally {
        # Cleanup
        Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }

    Write-Host ""
    Write-Success "Installation complete!"
    Write-Host ""

    # Check if in PATH
    $currentPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($currentPath -notlike "*$InstallDir*") {
        Write-Warn "$InstallDir is not in your PATH"
        Write-Host ""
        Write-Host "To add it permanently, run:" -ForegroundColor Yellow
        Write-Host ""
        Write-Host "  `$env:Path += `";$InstallDir`"" -ForegroundColor White
        Write-Host "  [Environment]::SetEnvironmentVariable('Path', `$env:Path + ';$InstallDir', 'User')" -ForegroundColor White
        Write-Host ""
    }

    Write-Host "Verify installation:"
    Write-Host "  hdr_analyzer_mvp --help"
    Write-Host "  mkvdolby --help"
    Write-Host "  verifier --help"
    Write-Host ""
}

Install-HdrAnalyze
