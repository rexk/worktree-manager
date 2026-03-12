#!/bin/sh
set -eu

REPO="rexk/worktree-manager"
BIN="wkm"

usage() {
  cat <<EOF
Install prebuilt wkm binaries from GitHub Releases.

Usage:
  install.sh [OPTIONS]

Options:
  --to <dir>        Install directory [default: ~/.local/bin]
  --tag <tag>       Version tag to install (e.g. v0.1.0) [default: latest]
  --target <triple> Target triple [default: auto-detect]
  --help            Show this help
EOF
}

say() {
  if [ -t 1 ]; then
    printf '\033[1;32m>\033[0m %s\n' "$1"
  else
    printf '> %s\n' "$1"
  fi
}

err() {
  if [ -t 2 ]; then
    printf '\033[1;31merror\033[0m: %s\n' "$1" >&2
  else
    printf 'error: %s\n' "$1" >&2
  fi
  exit 1
}

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    err "need $1 (command not found)"
  fi
}

# --- parse args ---

INSTALL_DIR="$HOME/.local/bin"
TAG=""
TARGET=""

while [ $# -gt 0 ]; do
  case "$1" in
    --to)
      [ $# -ge 2 ] || err "--to requires a directory argument"
      INSTALL_DIR="$2"
      shift 2
      ;;
    --tag)
      [ $# -ge 2 ] || err "--tag requires a version argument"
      TAG="$2"
      shift 2
      ;;
    --target)
      [ $# -ge 2 ] || err "--target requires a triple argument"
      TARGET="$2"
      shift 2
      ;;
    --help)
      usage
      exit 0
      ;;
    *)
      err "unknown option: $1"
      ;;
  esac
done

# --- detect platform ---

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Linux)  os_part="unknown-linux-gnu" ;;
    Darwin) os_part="apple-darwin" ;;
    *)      err "unsupported OS: $os (try downloading from https://github.com/$REPO/releases)" ;;
  esac

  case "$arch" in
    x86_64|amd64)  arch_part="x86_64" ;;
    aarch64|arm64) arch_part="aarch64" ;;
    *)             err "unsupported architecture: $arch" ;;
  esac

  echo "${arch_part}-${os_part}"
}

if [ -z "$TARGET" ]; then
  TARGET="$(detect_target)"
fi

say "detected target: $TARGET"

# --- pick downloader ---

download() {
  url="$1"
  output="$2"

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$output" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$output" "$url"
  else
    err "need curl or wget to download files"
  fi
}

fetch() {
  url="$1"

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO- "$url"
  else
    err "need curl or wget to download files"
  fi
}

# --- resolve version ---

if [ -z "$TAG" ]; then
  say "fetching latest release tag..."
  TAG="$(fetch "https://api.github.com/repos/$REPO/releases/latest" \
    | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
  [ -n "$TAG" ] || err "could not determine latest release tag"
fi

say "installing $BIN $TAG"

# version without leading 'v'
VERSION="${TAG#v}"

# --- download & extract ---

ARCHIVE="${BIN}-${VERSION}-${TARGET}.tar.gz"
URL="https://github.com/$REPO/releases/download/${TAG}/${ARCHIVE}"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

say "downloading $URL"
download "$URL" "$TMPDIR/$ARCHIVE"

need tar
tar xzf "$TMPDIR/$ARCHIVE" -C "$TMPDIR"

# --- install ---

mkdir -p "$INSTALL_DIR"
cp "$TMPDIR/$BIN" "$INSTALL_DIR/$BIN"
chmod +x "$INSTALL_DIR/$BIN"

say "installed $BIN to $INSTALL_DIR/$BIN"

# --- PATH check ---

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    if [ -t 1 ]; then
      printf '\033[1;33mwarning\033[0m: %s is not in your PATH\n' "$INSTALL_DIR"
    else
      printf 'warning: %s is not in your PATH\n' "$INSTALL_DIR"
    fi
    ;;
esac
