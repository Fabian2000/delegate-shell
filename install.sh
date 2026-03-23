#!/bin/sh
set -e

REPO="Fabian2000/delegate-shell"
INSTALL_DIR="/usr/local/bin"

# Detect OS and architecture
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case "$OS" in
    linux)  OS_NAME="linux" ;;
    darwin) OS_NAME="macos" ;;
    *)      echo "Unsupported OS: $OS"; exit 1 ;;
esac

case "$ARCH" in
    x86_64|amd64)   ARCH_NAME="x86_64" ;;
    aarch64|arm64)   ARCH_NAME="arm64" ;;
    *)               echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

ASSET="dgsh-${OS_NAME}-${ARCH_NAME}.tar.gz"

# Get latest release tag
echo "Fetching latest release..."
TAG=$(curl -sL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | head -1 | cut -d'"' -f4)

if [ -z "$TAG" ]; then
    echo "Failed to fetch latest release. Check https://github.com/${REPO}/releases"
    exit 1
fi

echo "Installing dgsh ${TAG} for ${OS_NAME}-${ARCH_NAME}..."

# Download and extract
URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"
TMPDIR=$(mktemp -d)
curl -sL "$URL" -o "${TMPDIR}/${ASSET}"

if [ ! -s "${TMPDIR}/${ASSET}" ]; then
    echo "Download failed. URL: ${URL}"
    rm -rf "$TMPDIR"
    exit 1
fi

tar xzf "${TMPDIR}/${ASSET}" -C "$TMPDIR"

# Install
if [ -w "$INSTALL_DIR" ]; then
    mv "${TMPDIR}/dgsh" "${INSTALL_DIR}/dgsh"
else
    echo "Installing to ${INSTALL_DIR} (requires sudo)..."
    sudo mv "${TMPDIR}/dgsh" "${INSTALL_DIR}/dgsh"
fi

chmod +x "${INSTALL_DIR}/dgsh"
rm -rf "$TMPDIR"

echo "dgsh ${TAG} installed to ${INSTALL_DIR}/dgsh"
echo "Run 'dgsh' to start the REPL or 'dgsh script.dgsh' to run a script."
