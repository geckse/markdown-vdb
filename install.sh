#!/bin/sh
set -eu

REPO="geckse/markdown-vdb"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"

# Detect OS
OS=$(uname -s)
case "$OS" in
  Darwin) os="apple-darwin" ;;
  Linux)  os="unknown-linux-gnu" ;;
  *)
    echo "Error: Unsupported OS: $OS"
    echo "Please download manually from https://github.com/${REPO}/releases"
    exit 1
    ;;
esac

# Detect architecture
ARCH=$(uname -m)
case "$ARCH" in
  x86_64|amd64)   arch="x86_64" ;;
  arm64|aarch64)   arch="aarch64" ;;
  *)
    echo "Error: Unsupported architecture: $ARCH"
    echo "Please download manually from https://github.com/${REPO}/releases"
    exit 1
    ;;
esac

TARGET="${arch}-${os}"

# Fetch latest release tag
echo "Fetching latest release..."
VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
  | grep '"tag_name"' \
  | sed 's/.*"tag_name": *"//;s/".*//')

if [ -z "$VERSION" ]; then
  echo "Error: Could not determine latest version"
  exit 1
fi

ARCHIVE="mdvdb-${VERSION}-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE}"

echo "Downloading mdvdb ${VERSION} for ${TARGET}..."

# Download to temp directory with cleanup trap
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

if ! curl -fsSL "$URL" -o "${TMPDIR}/${ARCHIVE}"; then
  echo "Error: Failed to download ${URL}"
  echo "Please check https://github.com/${REPO}/releases for available binaries"
  exit 1
fi

tar -xzf "${TMPDIR}/${ARCHIVE}" -C "$TMPDIR"

# Install binary
mkdir -p "$INSTALL_DIR"
mv "${TMPDIR}/mdvdb" "${INSTALL_DIR}/mdvdb"
chmod +x "${INSTALL_DIR}/mdvdb"

echo ""
echo "mdvdb ${VERSION} installed to ${INSTALL_DIR}/mdvdb"
echo ""

# Verify installation
if "${INSTALL_DIR}/mdvdb" --version 2>/dev/null; then
  echo ""
  echo "Run 'mdvdb --help' to get started."
else
  echo "Binary installed but could not verify. You may need to add ${INSTALL_DIR} to your PATH."
fi
