#!/usr/bin/env bash
# HDR-Analyze Suite Installer for Unix-like systems (Linux/macOS)
# Usage: curl -fsSL https://github.com/tinof/hdr-analyze/releases/latest/download/install.sh | bash

set -euo pipefail

REPO="tinof/hdr-analyze"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

info() { echo -e "${BLUE}[INFO]${NC} $1"; }
success() { echo -e "${GREEN}[OK]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1" >&2; exit 1; }

# Detect platform
detect_platform() {
    local os arch target

    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux*)  os="unknown-linux-gnu" ;;
        Darwin*) os="apple-darwin" ;;
        *)       error "Unsupported OS: $os" ;;
    esac

    case "$arch" in
        x86_64|amd64) arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *)            error "Unsupported architecture: $arch" ;;
    esac

    # Linux ARM64 not currently supported
    if [[ "$os" == "unknown-linux-gnu" && "$arch" == "aarch64" ]]; then
        error "Linux ARM64 is not currently supported. Please build from source."
    fi

    target="${arch}-${os}"
    echo "$target"
}

# Get latest release version
get_latest_version() {
    local version
    version=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')
    if [[ -z "$version" ]]; then
        error "Failed to fetch latest version"
    fi
    echo "$version"
}

# Download and extract
download_and_install() {
    local version="$1"
    local target="$2"
    local archive_name="hdr-analyze-${version}-${target}.tar.gz"
    local download_url="https://github.com/${REPO}/releases/download/${version}/${archive_name}"
    local temp_dir

    temp_dir=$(mktemp -d)
    trap "rm -rf '$temp_dir'" EXIT

    info "Downloading ${archive_name}..."
    if ! curl -fsSL "$download_url" -o "${temp_dir}/${archive_name}"; then
        error "Failed to download from: $download_url"
    fi

    info "Extracting archive..."
    tar -xzf "${temp_dir}/${archive_name}" -C "$temp_dir"

    # Create install directory if needed
    mkdir -p "$INSTALL_DIR"

    # Find and install binaries
    local bin_dir="${temp_dir}/hdr-analyze-${version}-${target}/bin"
    if [[ ! -d "$bin_dir" ]]; then
        error "Binary directory not found in archive"
    fi

    info "Installing to ${INSTALL_DIR}..."
    for binary in hdr_analyzer_mvp mkvdolby verifier; do
        if [[ -f "${bin_dir}/${binary}" ]]; then
            cp "${bin_dir}/${binary}" "$INSTALL_DIR/"
            chmod +x "${INSTALL_DIR}/${binary}"
            success "Installed: ${binary}"
        fi
    done
}

# Check if install dir is in PATH
check_path() {
    if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
        warn "$INSTALL_DIR is not in your PATH"
        echo ""
        echo "Add it to your shell profile:"
        echo ""
        echo "  # For bash (~/.bashrc):"
        echo "  export PATH=\"\$PATH:$INSTALL_DIR\""
        echo ""
        echo "  # For zsh (~/.zshrc):"
        echo "  export PATH=\"\$PATH:$INSTALL_DIR\""
        echo ""
    fi
}

# Main
main() {
    echo ""
    echo "  HDR-Analyze Suite Installer"
    echo "  ==========================="
    echo ""

    # Check for curl
    if ! command -v curl &> /dev/null; then
        error "curl is required but not installed"
    fi

    local target version
    target=$(detect_platform)
    info "Detected platform: $target"

    version=$(get_latest_version)
    info "Latest version: $version"

    download_and_install "$version" "$target"

    echo ""
    success "Installation complete!"
    echo ""

    check_path

    echo "Verify installation:"
    echo "  hdr_analyzer_mvp --help"
    echo "  mkvdolby --help"
    echo "  verifier --help"
    echo ""
}

main "$@"
