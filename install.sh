#!/bin/bash

#####################################################################
# Wassette Binary Installer Script
#####################################################################
#
# This script automatically downloads and installs the latest Wassette 
# binary for your platform (Linux or macOS, ARM64 or AMD64).
#
# WHAT IT DOES:
# - Detects your operating system and architecture
# - Downloads the latest Wassette release from GitHub
# - Extracts and installs the binary to ~/.local/bin
# - Configures your shell PATH for immediate access
# - Works with bash, zsh, and other POSIX-compliant shells
#
# USAGE:
#   curl -fsSL https://raw.githubusercontent.com/microsoft/wassette/main/install.sh | bash
#
# REQUIREMENTS:
# - curl (for downloading)
# - tar (for extraction)
# - bash or compatible shell
#
# SUPPORTED PLATFORMS:
# - Linux (x86_64, ARM64)
# - macOS (Intel, Apple Silicon)
#
# The binary will be installed to: ~/.local/bin/wassette
# PATH will be updated in: ~/.bashrc, ~/.zshrc, ~/.profile
#
#####################################################################

set -e  # Exit on any error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Configuration
BINARY_NAME="wassette"  # Change this to your binary name
GITHUB_REPO="microsoft/wassette"  # GitHub repository (owner/repo)
BASE_URL="https://api.github.com/repos/${GITHUB_REPO}/releases/latest"  # Base download URL
INSTALL_DIR="$HOME/.local/bin"  # Installation directory

# OS and Architecture detection
get_os() {
    local os
    os=$(uname -s)
    case "$os" in
        Linux*)     echo "linux" ;;
        Darwin*)    echo "darwin" ;;
        *)          
            print_error "Unsupported OS: $os"
            print_error "This installer supports Linux and macOS only"
            exit 1
            ;;
    esac
}

get_arch() {
    local arch
    arch=$(uname -m)
    case "$arch" in
        x86_64|amd64)   echo "amd64" ;;
        aarch64|arm64)  echo "arm64" ;;
        *)              
            print_error "Unsupported architecture: $arch"
            print_error "This installer supports amd64 and arm64 only"
            exit 1
            ;;
    esac
}

# Detect current system
OS=$(get_os)
ARCH=$(get_arch)
PLATFORM="${OS}_${ARCH}"

print_status "Detected platform: $PLATFORM"

get_latest_release_info() {
    print_status "Fetching latest release information..."
    
    if ! command -v curl >/dev/null 2>&1; then
        print_error "curl is required but not installed. Please install curl."
        exit 1
    fi
    
    local api_response
    api_response=$(curl -s "$BASE_URL")
    
    if [[ -z "$api_response" ]]; then
        print_error "Failed to fetch release information from GitHub API"
        exit 1
    fi
    
    # Extract tag name (version) - using sed instead of grep -P for macOS compatibility
    local tag_name
    tag_name=$(echo "$api_response" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)
    
    if [[ -z "$tag_name" ]]; then
        print_error "Could not extract version from GitHub API response"
        exit 1
    fi
    
    # Remove 'v' prefix if present for filename
    local version="${tag_name#v}"
    
    print_status "Latest version: $tag_name"
    
    # Wassette binary naming: wassette_0.11.0_darwin_amd64.tar.gz (no 'v' prefix in filename)
    BINARY_ARCHIVE="${BINARY_NAME}_${version}_${PLATFORM}.tar.gz"
    
    # Extract download URL for our platform - using sed instead of grep -P
    DOWNLOAD_URL=$(echo "$api_response" | sed -n 's/.*"browser_download_url": *"\([^"]*'"$BINARY_ARCHIVE"'[^"]*\)".*/\1/p' | head -1)
    
    if [[ -z "$DOWNLOAD_URL" ]]; then
        print_error "No binary found for platform: $PLATFORM"
        print_error "Looking for: $BINARY_ARCHIVE"
        print_status "Available assets:"
        echo "$api_response" | sed -n 's/.*"name": *"\([^"]*\.[tar\.gz|zip].*\)".*/\1/p'
        exit 1
    fi

    print_status "Download URL: $DOWNLOAD_URL"
}

download_and_extract() {
    print_status "Downloading and extracting wassette binary..."
    
    # Create temporary directory
    local tmp_dir
    tmp_dir=$(mktemp -d)
    print_status "Using temporary directory: $tmp_dir"
    
    # Download the binary archive
    local archive_path="$tmp_dir/$BINARY_ARCHIVE"
    print_status "Downloading $BINARY_ARCHIVE..."
    
    if ! curl -L -o "$archive_path" "$DOWNLOAD_URL"; then
        print_error "Failed to download binary from: $DOWNLOAD_URL"
        rm -rf "$tmp_dir"
        exit 1
    fi
    
    # Verify download
    if [[ ! -f "$archive_path" ]]; then
        print_error "Downloaded file not found: $archive_path"
        rm -rf "$tmp_dir"
        exit 1
    fi
    
    print_status "Download completed. File size: $(ls -lh "$archive_path" | awk '{print $5}')"
    
    # Extract the archive
    print_status "Extracting archive..."
    
    if ! tar -xzf "$archive_path" -C "$tmp_dir"; then
        print_error "Failed to extract archive: $archive_path"
        rm -rf "$tmp_dir"
        exit 1
    fi
    
    # Find the extracted binary
    local binary_path="$tmp_dir/$BINARY_NAME"
    if [[ ! -f "$binary_path" ]]; then
        print_error "Binary not found after extraction: $binary_path"
        print_status "Contents of temp directory:"
        ls -la "$tmp_dir"
        rm -rf "$tmp_dir"
        exit 1
    fi
    
    # Make binary executable
    chmod +x "$binary_path"
    
    print_status "Binary extracted successfully to: $binary_path"
    print_status "Binary version:"
    "$binary_path" --version 2>/dev/null || echo "Version check failed, but binary exists"
    
    # Store the paths for later use
    TEMP_DIR="$tmp_dir"
    BINARY_PATH="$binary_path"
    
    print_status "Download and extraction completed successfully!"
}

# Handle local file or download
if [[ -n "${LOCAL_BINARY:-}" ]] && [[ -f "$LOCAL_BINARY" ]]; then
    # Use local binary if specified
    print_status "Using local binary: $LOCAL_BINARY"
    BINARY_PATH="$LOCAL_BINARY"
else
    # Download binary
    get_latest_release_info
    download_and_extract
    CLEANUP_TEMP=true
fi

# Create installation directory if it doesn't exist
print_status "Creating installation directory: $INSTALL_DIR"
mkdir -p "$INSTALL_DIR"

# Install binary
print_status "Installing $BINARY_NAME to $INSTALL_DIR"
cp "$BINARY_PATH" "$INSTALL_DIR/$BINARY_NAME"

# Clean up temporary file if we downloaded it
if [[ "${CLEANUP_TEMP:-}" == "true" ]]; then
    rm -rf "$TEMP_DIR"
fi

# Function to add PATH to a file if not already present
add_path_to_file() {
    local file="$1"
    local path_line="export PATH=\"\$HOME/.local/bin:\$PATH\""
    
    if [[ -f "$file" ]]; then
        if ! grep -q "\.local/bin" "$file"; then
            print_status "Adding PATH to $file"
            echo "" >> "$file"
            echo "# Added by binary installer script" >> "$file"
            echo "$path_line" >> "$file"
            return 0
        else
            print_warning "PATH already configured in $file"
            return 1
        fi
    fi
    return 1
}

# Track if we modified any files
modified_files=()

# Add to shell configuration files
# Check for bash
if [[ -n "$BASH_VERSION" ]] || [[ "$SHELL" == */bash ]]; then
    # Try .bashrc first (most common)
    if add_path_to_file "$HOME/.bashrc"; then
        modified_files+=("$HOME/.bashrc")
    fi
    
    # Also try .bash_profile for login shells
    if add_path_to_file "$HOME/.bash_profile"; then
        modified_files+=("$HOME/.bash_profile")
    fi
fi

# Check for zsh
if [[ -n "$ZSH_VERSION" ]] || [[ "$SHELL" == */zsh ]]; then
    if add_path_to_file "$HOME/.zshrc"; then
        modified_files+=("$HOME/.zshrc")
    fi
fi

# Add to .profile as fallback (POSIX compliant)
if add_path_to_file "$HOME/.profile"; then
    modified_files+=("$HOME/.profile")
fi

# Create a system-wide PATH file for immediate recognition
print_status "Creating PATH configuration in /etc/environment (if writable)"
if sudo -n true 2>/dev/null; then
    # We have sudo access
    if [[ -f /etc/environment ]]; then
        if ! sudo grep -q "$HOME/.local/bin" /etc/environment 2>/dev/null; then
            echo "PATH=\"$HOME/.local/bin:\$PATH\"" | sudo tee -a /etc/environment > /dev/null
            print_status "Added PATH to /etc/environment"
        fi
    fi
else
    # Skip system-wide PATH configuration
    print_warning "Skipping system-wide PATH configuration in /etc/environment"
    print_warning "Consider manually adding '$HOME/.local/bin' to your PATH if needed"
fi

# Update current session's PATH
export PATH="$HOME/.local/bin:$PATH"

# Verify installation
if command -v "$BINARY_NAME" >/dev/null 2>&1; then
    print_status "✓ Installation successful!"
    print_status "✓ $BINARY_NAME is now available in PATH"
    
    # Show version or help if available
    if "$BINARY_NAME" --version >/dev/null 2>&1; then
        print_status "Version: $("$BINARY_NAME" --version)"
    elif "$BINARY_NAME" --help >/dev/null 2>&1; then
        print_status "Run '$BINARY_NAME --help' for usage information"
    fi
else
    print_error "Installation failed - binary not found in PATH"
    exit 1
fi

# Summary
echo ""
print_status "Installation Summary:"
echo "  Binary: $BINARY_NAME"
echo "  Platform: $PLATFORM"
echo "  Location: $INSTALL_DIR/$BINARY_NAME"
echo "  Available in current session: Yes"

if [[ ${#modified_files[@]} -gt 0 ]]; then
    echo "  Modified files:"
    for file in "${modified_files[@]}"; do
        echo "    - $file"
    done
    echo ""
    print_status "The binary will be available in new bash sessions automatically."
    print_status "No need to source any files - just open a new terminal!"
else
    echo ""
    print_warning "No shell configuration files were modified."
    print_warning "You may need to manually add '$INSTALL_DIR' to your PATH."
fi

echo ""
print_status "You can now run '$BINARY_NAME' from any new terminal window."
echo ""