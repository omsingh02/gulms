#!/bin/sh
# Install gulms: download the latest release for this machine, verify its checksum,
# and put the binary in ~/.local/bin.
#
#   curl -fsSL https://raw.githubusercontent.com/omsingh02/gulms/main/install.sh | sh
#
# Environment:
#   GULMS_VERSION      install a specific tag, e.g. v0.1.0 (default: the latest release)
#   GULMS_INSTALL_DIR  where to put the binary (default: $HOME/.local/bin)
#   GULMS_BASE_URL     read release files from here instead of GitHub (used by tests)
set -eu

REPO="omsingh02/gulms"
INSTALL_DIR="${GULMS_INSTALL_DIR:-$HOME/.local/bin}"
BASE_URL="${GULMS_BASE_URL:-}"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

fetch() {
  if have curl; then
    curl -fsSL "$1" -o "$2" || die "could not download $1"
  elif have wget; then
    wget -q "$1" -O "$2" || die "could not download $1"
  else
    die "need curl or wget to download files"
  fi
}

detect_target() {
  os=$(uname -s)
  arch=$(uname -m)
  case "$os" in
    Linux) os_part="unknown-linux-gnu" ;;
    Darwin) os_part="apple-darwin" ;;
    *) die "unsupported OS '$os' (on Windows, use install.ps1)" ;;
  esac
  case "$arch" in
    x86_64 | amd64) arch_part="x86_64" ;;
    aarch64 | arm64) arch_part="aarch64" ;;
    *) die "unsupported CPU '$arch'; build from source with: cargo install --git https://github.com/$REPO" ;;
  esac
  printf '%s-%s' "$arch_part" "$os_part"
}

latest_version() {
  have curl || die "set GULMS_VERSION (for example v0.1.0); finding the latest release needs curl"
  final=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") ||
    die "could not reach github.com"
  tag=${final##*/}
  case "$tag" in
    v*) printf '%s' "$tag" ;;
    *) die "no release found at https://github.com/$REPO/releases" ;;
  esac
}

sha256_of() {
  if have sha256sum; then
    sha256sum "$1" | awk '{print $1}'
  elif have shasum; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    die "need sha256sum or shasum to verify the download"
  fi
}

target=$(detect_target)
if [ -n "${GULMS_VERSION:-}" ]; then
  version="$GULMS_VERSION"
else
  version=$(latest_version)
fi
archive="gulms-${version}-${target}.tar.gz"
base="${BASE_URL:-https://github.com/$REPO/releases/download/$version}"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

say "Installing gulms $version ($target)"
fetch "$base/$archive" "$tmp/$archive"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS"

expected=$(awk -v f="$archive" '$2 == f { print $1 }' "$tmp/SHA256SUMS")
[ -n "$expected" ] || die "no checksum listed for $archive"
actual=$(sha256_of "$tmp/$archive")
[ "$expected" = "$actual" ] || die "checksum mismatch for $archive; refusing to install"

tar xzf "$tmp/$archive" -C "$tmp"
mkdir -p "$INSTALL_DIR"
install -m 755 "$tmp/gulms-${version}-${target}/gulms" "$INSTALL_DIR/gulms"

say "Installed to $INSTALL_DIR/gulms"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    say ""
    say "$INSTALL_DIR is not on your PATH. Add it with:"
    say "  export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac
say ""
say "Next: run 'gulms' to sign in and get started."
say "Tab completion: gulms completions bash|zsh|fish > <your completions folder>"
