# wassette installer script for Windows
# Downloads and installs the appropriate wassette binary for your system

[CmdletBinding()]
param(
    [switch]$Help,
    [switch]$UseGitHubCLI
)

# Configuration
$REPO = "microsoft/wassette"
$BINARY_NAME = "wassette"

# Helper functions for logging
function Write-Info {
    param([string]$Message)
    Write-Host "ℹ️  $Message" -ForegroundColor Blue
}

function Write-Success {
    param([string]$Message)
    Write-Host "✅ $Message" -ForegroundColor Green
}

function Write-Warning {
    param([string]$Message)
    Write-Host "⚠️  $Message" -ForegroundColor Yellow
}

function Write-Error {
    param([string]$Message)
    Write-Host "❌ $Message" -ForegroundColor Red
}

# Detect operating system (Windows only)
function Get-OperatingSystem {
    # Verify we're running on Windows
    if ($IsWindows -or $env:OS -eq "Windows_NT" -or [System.Environment]::OSVersion.Platform -eq "Win32NT") {
        return "windows"
    }
    else {
        Write-Error "This installer is designed for Windows only. For other platforms, use install.sh."
        exit 1
    }
}

# Detect architecture (Windows)
function Get-Architecture {
    # Try PowerShell Core method first
    if ([System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture) {
        $runtimeArch = [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture
        switch ($runtimeArch) {
            "X64" { return "amd64" }
            "Arm64" { return "arm64" }
            "X86" { 
                Write-Error "32-bit x86 architecture is not supported"
                exit 1
            }
            default {
                Write-Error "Unsupported architecture: $runtimeArch"
                exit 1
            }
        }
    }
    
    # Fallback to Windows environment variables
    $arch = $env:PROCESSOR_ARCHITECTURE
    switch ($arch) {
        "AMD64" { return "amd64" }
        "ARM64" { return "arm64" }
        "x86" {
            Write-Error "32-bit x86 architecture is not supported"
            exit 1
        }
        default {
            Write-Error "Could not determine system architecture. Found: $arch"
            exit 1
        }
    }
}

# Find the installation directory
function Find-InstallDirectory {
    $installDir = Join-Path $env:LOCALAPPDATA "wassette\bin"

    # Create the directory if it doesn't exist
    if (!(Test-Path $installDir)) {
        try {
            New-Item -ItemType Directory -Path $installDir -Force | Out-Null
            Write-Info "Created directory: $installDir"
        }
        catch {
            Write-Error "Failed to create directory: $installDir"
            exit 1
        }
    }

    # Test if directory is writable
    try {
        $testFile = Join-Path $installDir "test_write_$(Get-Random).tmp"
        New-Item -ItemType File -Path $testFile -Force | Out-Null
        Remove-Item $testFile -Force
    }
    catch {
        Write-Error "$installDir is not a writable directory"
        exit 1
    }

    return $installDir
}

# Get the latest release version from GitHub API or GitHub CLI
function Get-LatestVersion {
    param([bool]$UseGitHubCLI = $false)
    
    if ($UseGitHubCLI) {
        # Use GitHub CLI for private repositories
        try {
            Write-Info "Fetching latest release information using GitHub CLI..."
            
            # Check if gh CLI is available
            if (!(Get-Command gh -ErrorAction SilentlyContinue)) {
                Write-Error "GitHub CLI (gh) is not installed or not in PATH"
                Write-Error "Please install GitHub CLI from https://cli.github.com/ or use the public API method"
                exit 1
            }
            
            # Get latest release info using gh CLI
            $releaseInfo = & gh release view --repo $REPO --json tagName 2>$null
            if ($LASTEXITCODE -ne 0) {
                Write-Error "Failed to get release information using GitHub CLI"
                Write-Error "Make sure you're authenticated with 'gh auth login' and have access to the repository"
                exit 1
            }
            
            $releaseData = $releaseInfo | ConvertFrom-Json
            $version = $releaseData.tagName
            
            if ([string]::IsNullOrEmpty($version)) {
                Write-Error "Failed to get latest release version from GitHub CLI"
                exit 1
            }
            
            return $version
        }
        catch {
            Write-Error "Failed to fetch release information using GitHub CLI: $($_.Exception.Message)"
            exit 1
        }
    }
    else {
        # Use public GitHub API
        try {
            Write-Info "Fetching latest release information from GitHub API..."
            $response = Invoke-RestMethod -Uri "https://api.github.com/repos/$REPO/releases/latest" -ErrorAction Stop
            $version = $response.tag_name
            
            if ([string]::IsNullOrEmpty($version)) {
                Write-Error "Failed to get latest release version from GitHub API"
                exit 1
            }
            
            return $version
        }
        catch {
            Write-Error "Failed to fetch release information from GitHub API: $($_.Exception.Message)"
            Write-Info "If this is a private repository, try using the -UseGitHubCLI parameter"
            exit 1
        }
    }
}

# Download and extract binary
function Install-Binary {
    param(
        [string]$Os,
        [string]$Arch,
        [string]$Version,
        [string]$InstallDir,
        [bool]$UseGitHubCLI = $false
    )

    # For Windows, construct the archive name to match actual release artifacts
    # Remove 'v' prefix from version if present
    $cleanVersion = $Version -replace '^v', ''
    $archiveName = "wassette_${cleanVersion}_${Os}_${Arch}"
    $extension = "zip"
    
    # Create temporary directory
    $tempDir = Join-Path $env:TEMP "wassette-install-$(Get-Random)"
    New-Item -ItemType Directory -Path $tempDir -Force | Out-Null
    
    $archiveFile = Join-Path $tempDir "$archiveName.$extension"

    try {
        Write-Info "Downloading $BINARY_NAME $Version for $Os/$Arch..."
        
        if ($UseGitHubCLI) {
            # Use GitHub CLI to download from private repository
            Write-Info "Using GitHub CLI to download release asset..."
            
            & gh release download $Version --repo $REPO --pattern "$archiveName.$extension" --dir $tempDir 2>$null
            if ($LASTEXITCODE -ne 0) {
                Write-Error "Failed to download release asset using GitHub CLI"
                Write-Error "Make sure you're authenticated and have access to the repository"
                exit 1
            }
        }
        else {
            # Use direct download from public repository
            $downloadUrl = "https://github.com/$REPO/releases/download/$Version/$archiveName.$extension"
            Write-Info "Download URL: $downloadUrl"
            
            # Download the file
            Invoke-WebRequest -Uri $downloadUrl -OutFile $archiveFile -ErrorAction Stop
        }
        
        if (!(Test-Path $archiveFile)) {
            Write-Error "Failed to download release asset"
            exit 1
        }

        Write-Info "Extracting archive..."
        
        # Extract zip file
        Expand-Archive -Path $archiveFile -DestinationPath $tempDir -Force
        
        # Find the binary (should be wassette.exe)
        $binaryPath = Join-Path $tempDir "$BINARY_NAME.exe"
        
        if (!(Test-Path $binaryPath)) {
            Write-Error "Binary not found in archive at $binaryPath"
            exit 1
        }

        Write-Info "Installing to $InstallDir..."
        
        # Copy binary to install directory
        $targetPath = Join-Path $InstallDir "$BINARY_NAME.exe"
        Copy-Item $binaryPath $targetPath -Force
        
        Write-Success "$BINARY_NAME installed successfully!"
        
        return $targetPath
    }
    catch {
        Write-Error "Installation failed: $($_.Exception.Message)"
        exit 1
    }
    finally {
        # Clean up temporary directory
        if (Test-Path $tempDir) {
            Remove-Item $tempDir -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

# Show help message
function Show-Help {
    Write-Host "wassette installer for Windows"
    Write-Host ""
    Write-Host "Usage: .\install.ps1 [options]"
    Write-Host ""
    Write-Host "Options:"
    Write-Host "  -Help         Show this help message"
    Write-Host "  -UseGitHubCLI Use GitHub CLI for private repositories (requires 'gh' CLI)"
    Write-Host ""
    Write-Host "This script will:"
    Write-Host "  1. Detect your OS and architecture"
    Write-Host "  2. Download the latest wassette binary"
    Write-Host "  3. Install it to %LOCALAPPDATA%\wassette\bin"
    Write-Host ""
    Write-Host "Examples:"
    Write-Host "  .\install.ps1                    # Use public GitHub API"
    Write-Host "  .\install.ps1 -UseGitHubCLI      # Use GitHub CLI for private repos"
}

# Main installation process
function Main {
    if ($Help) {
        Show-Help
        exit 0
    }

    Write-Info "Starting wassette installation..."

    # Detect platform
    $os = Get-OperatingSystem
    $arch = Get-Architecture
    Write-Info "Detected platform: $os/$arch"

    # Get latest version
    $version = Get-LatestVersion -UseGitHubCLI $UseGitHubCLI
    Write-Info "Latest version: $version"

    # Find installation directory
    $installDir = Find-InstallDirectory
    Write-Info "Installation directory: $installDir"

    # Download and install
    $installedPath = Install-Binary -Os $os -Arch $arch -Version $version -InstallDir $installDir -UseGitHubCLI $UseGitHubCLI

    # Check if install directory is in PATH
    $pathDirs = $env:PATH -split ';'
    if ($installDir -notin $pathDirs) {
        Write-Warning "Installation directory $installDir is not in your user PATH"
        Write-Info "Adding to user PATH..."
        try {
            # Add to user PATH (no Administrator required)
            $currentPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
            if (-not $currentPath) {
                $currentPath = ""
            }
            if ($currentPath -notlike "*$installDir*") {
                $newPath = if ($currentPath) { "$currentPath;$installDir" } else { $installDir }
                [Environment]::SetEnvironmentVariable('PATH', $newPath, 'User')
                Write-Success "Added $installDir to user PATH"
                Write-Info "You may need to restart your PowerShell session for PATH changes to take effect."
            }
        }
        catch {
            Write-Warning "Failed to add to user PATH: $($_.Exception.Message)"
            Write-Info "You can manually add it by running:"
            Write-Host "    `$userPath = [Environment]::GetEnvironmentVariable('PATH', 'User')" -ForegroundColor Cyan
            Write-Host "    [Environment]::SetEnvironmentVariable('PATH', `"`$userPath;$installDir`", 'User')" -ForegroundColor Cyan
        }
    }

    # Verify installation
    if (Test-Path $installedPath) {
        Write-Success "Installation complete!"
        
        # Test if wassette command is available
        try {
            $foundCommand = Get-Command wassette -ErrorAction SilentlyContinue
            if ($foundCommand -and $foundCommand.Source -eq $installedPath) {
                Write-Success "wassette is ready to use!"
                Write-Info "Try running: wassette --help"
            }
            elseif ($foundCommand) {
                Write-Warning "Different 'wassette' command found at $($foundCommand.Source)"
                Write-Info "Try running: $installedPath --help"
                Write-Info "Or restart your PowerShell session if you just updated PATH"
            }
            else {
                Write-Warning "You may need to restart PowerShell or update your PATH"
                Write-Info "Try running: $installedPath --help"
            }
        }
        catch {
            Write-Info "Try running: $installedPath --help"
        }

        Write-Host ""
        Write-Info "Next steps:"
        Write-Info "Run 'wassette --help' to see available commands"
        Write-Info ""
        Write-Info "For more information, visit: https://github.com/$REPO"
    }
    else {
        Write-Error "Installation verification failed"
        exit 1
    }
}

# Run main function
Main
