#!/bin/sh
# Installs barcode-read @VERSION@ into ~/.local/bin after checking the archive's SHA-256.
# curl -fsSL https://github.com/nowmyagentcan/barcode-read/releases/latest/download/install.sh | sh
set -eu
version=@VERSION@
base=${BARCODE_READ_BASE:-https://github.com/nowmyagentcan/barcode-read/releases/download/$version}
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target=linux-x64 ;;
  Linux-aarch64 | Linux-arm64) target=linux-arm64 ;;
  Darwin-x86_64) target=macos-x64 ;;
  Darwin-arm64) target=macos-arm64 ;;
  *) echo "barcode-read: no build for $(uname -s) $(uname -m)." >&2; exit 1 ;;
esac
# The Linux builds use the system's glibc and libstdc++, as found on Ubuntu 22.04 and later.
case $target-$(ldd --version 2>&1 | head -n 1) in
  macos-* | *" 2.3"[5-9]* | *" 2."[4-9][0-9]*) ;;
  *) echo "barcode-read needs glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 or later). Alpine and other musl systems aren't supported yet." >&2; exit 1 ;;
esac
archive=barcode-read-$version-$target.tar.gz
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL -o "$tmp/$archive" "$base/$archive"
curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS"
expected=$(awk -v f="$archive" '$2 == f { print $1 }' "$tmp/SHA256SUMS")
if command -v sha256sum >/dev/null; then actual=$(sha256sum "$tmp/$archive"); else actual=$(shasum -a 256 "$tmp/$archive"); fi
if [ -z "$expected" ] || [ "${actual%% *}" != "$expected" ]; then
  echo "barcode-read: the SHA-256 of $archive doesn't match SHA256SUMS; nothing was installed." >&2
  exit 1
fi
tar -xzf "$tmp/$archive" -C "$tmp" barcode-read
# An empty file "runs" as an empty script, so the answer is checked too.
case $("$tmp/barcode-read" --version 2>/dev/null || :) in
  "barcode-read "*) ;;
  *) echo "barcode-read: $archive has no barcode-read that runs here; nothing was installed." >&2; exit 1 ;;
esac
dir=$HOME/.local/bin
mkdir -p "$dir"
mv "$tmp/barcode-read" "$dir/barcode-read"
"$dir/barcode-read" --version
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "Add $dir to your PATH: export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac
