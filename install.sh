#!/usr/bin/env bash

# wassette installer script
# Downloads and installs the appropriate wassette binary for your system

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
REPO="microsoft/wassette"
BINARY_NAME="wassette"
INSTALL_DIR=""

# Helper functions
log_info() {
    printf "${BLUE}ℹ️  %s${NC}\n" "$1"
}

log_success() {
    printf "${GREEN}✅ %s${NC}\n" "$1"
}

log_warning() {
    printf "${YELLOW}⚠️  %s${NC}\n" "$1"
}

log_error() {
    printf "${RED}❌ %s${NC}\n" "$1"
}

# Check if a command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Detect operating system
detect_os() {
    local os
    case "$(uname -s)" in
    Linux*) os="linux" ;;
    Darwin*) os="darwin" ;;
    CYGWIN* | MINGW32* | MSYS* | MINGW*) os="windows" ;;
    *)
        log_error "Unsupported operating system: $(uname -s)"
        exit 1
        ;;
    esac
    echo "$os"
}

# Detect architecture
detect_arch() {
    local arch
    case "$(uname -m)" in
    x86_64 | amd64) arch="amd64" ;;
    arm64 | aarch64) arch="arm64" ;;
    *)
        log_error "Unsupported architecture: $(uname -m)"
        exit 1
        ;;
    esac
    echo "$arch"
}

# Find the best installation directory
find_install_dir() {
    local install_dir="${BIN_DIR:-$HOME/.local/bin}"

    # Create the directory if it doesn't exist
    mkdir -p "$install_dir"

    # Check if it's writable
    if [ ! -w "$install_dir" ]; then
        log_error "$install_dir is not a writable directory"
        exit 1
    fi

    echo "$install_dir"
}

# Get the latest release version
get_latest_version() {
    local version
    version=$(curl -s "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')
    if [ -z "$version" ]; then
        log_error "Failed to get latest release version from GitHub API"
        return 1
    fi
    echo "$version"
}

# Download and extract binary
download_and_install() {
    local os="$1"
    local arch="$2"
    local version="$3"
    local install_dir="$4"

    local archive_name="wassette-${os}-${arch}"
    local extension="tar.gz"

    # Handle Windows naming convention
    if [ "$os" = "windows" ]; then
        archive_name="${archive_name}.exe"
        extension="zip"
    fi

    local download_url="https://github.com/$REPO/releases/download/$version/${archive_name}.${extension}"
    local temp_dir=$(mktemp -d)
    local archive_file="$temp_dir/${archive_name}.${extension}"

    log_info "Downloading $BINARY_NAME $version for $os/$arch..."

    if ! curl -L -o "$archive_file" "$download_url"; then
        log_error "Failed to download $download_url"
        exit 1
    fi

    if [ ! -f "$archive_file" ]; then
        log_error "Failed to download $download_url"
        exit 1
    fi

    log_info "Extracting archive..."

    if [ "$extension" = "tar.gz" ]; then
        tar -xzf "$archive_file" -C "$temp_dir"
    else
        # Handle zip files (Windows)
        if command_exists unzip; then
            unzip -q "$archive_file" -d "$temp_dir"
        else
            log_error "unzip command not found, cannot extract Windows binary"
            exit 1
        fi
    fi

    local binary_path="$temp_dir/$BINARY_NAME"

    # Handle Windows binary extension
    if [ "$os" = "windows" ]; then
        binary_path="${binary_path}.exe"
    fi

    if [ ! -f "$binary_path" ]; then
        log_error "Binary not found in archive"
        exit 1
    fi

    log_info "Installing to $install_dir..."
    mkdir -p "$install_dir"
    cp "$binary_path" "$install_dir/"
    chmod +x "$install_dir/$BINARY_NAME"

    # Clean up
    rm -rf "$temp_dir"

    log_success "$BINARY_NAME installed successfully!"
}

# Main installation process
main() {
    # Handle command line arguments
    case "${1:-}" in
    -h | --help)
        echo "wassette installer"
        echo ""
        echo "Usage: $0 [options]"
        echo ""
        echo "Options:"
        echo "  -h, --help    Show this help message"
        echo ""
        echo "This script will:"
        echo "  1. Detect your OS and architecture"
        echo "  2. Download the latest wassette binary"
        echo "  3. Install it to your PATH"
        echo ""
        echo "Environment variables:"
        echo "  BIN_DIR       Custom installation directory (default: \$HOME/.local/bin)"
        echo ""
        echo "Examples:"
        echo "  curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | bash"
        echo "  BIN_DIR=/usr/local/bin ./install.sh"
        exit 0
        ;;
    esac

    log_info "Starting wassette installation..."

    local os=$(detect_os)
    local arch=$(detect_arch)
    log_info "Detected platform: $os/$arch"

    local version=$(get_latest_version)
    if [ -z "$version" ]; then
        log_error "Failed to get latest release version"
        exit 1
    fi
    log_info "Latest version: $version"

    INSTALL_DIR=$(find_install_dir)
    log_info "Installation directory: $INSTALL_DIR"

    download_and_install "$os" "$arch" "$version" "$INSTALL_DIR"

    # Check if install directory is in PATH
    if ! echo "$PATH" | grep -q "$INSTALL_DIR"; then
        log_warning "Installation directory $INSTALL_DIR is not in your PATH"
        log_info "Add this to your shell profile (.bashrc, .zshrc, etc.):"
        echo "    export PATH=\"$INSTALL_DIR:\$PATH\""
        log_info "Then restart your terminal or run: source ~/.bashrc (or your shell's config file)"
    fi

    # Verify installation
    if [ -x "$INSTALL_DIR/$BINARY_NAME" ]; then
        log_success "Installation complete!"

        # Check if the correct binary is being found in PATH
        local found_binary=$(command -v "$BINARY_NAME" 2>/dev/null || echo "")
        if [ "$found_binary" = "$INSTALL_DIR/$BINARY_NAME" ]; then
            log_success "$BINARY_NAME is ready to use!"
            log_info "Try running: $BINARY_NAME --help"
        elif [ -n "$found_binary" ]; then
            log_warning "Different '$BINARY_NAME' command found at $found_binary"
            log_info "Try running: $INSTALL_DIR/$BINARY_NAME --help"
            log_info "Or run 'hash -r' and try again"
            log_info "Or add $INSTALL_DIR to the beginning of your PATH"
        else
            log_warning "You may need to restart your terminal or update your PATH"
            log_info "Try running: $INSTALL_DIR/$BINARY_NAME --help"
        fi

        echo ""
        log_info "Next steps:"
        log_info "1. Run '$BINARY_NAME --help' to see available commands"
        log_info "2. Create a policy.yaml file for your tools"
        log_info "3. Start the server with: $BINARY_NAME serve --http --policy-file policy.yaml"
        log_info ""
        log_info "For more information, visit: https://github.com/$REPO"
    else
        log_error "Installation verification failed"
        exit 1
    fi
}

main "$@"
