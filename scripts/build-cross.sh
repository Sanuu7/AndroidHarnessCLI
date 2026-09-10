#!/usr/bin/env bash
# Cross-compile a static aarch64 binary on a Linux desktop and hand it to the
# phone. This is the fast path: no compiling on the phone at all.
#
#   rustup target add aarch64-unknown-linux-musl
#   ./scripts/build-cross.sh             # build only
#   ./scripts/build-cross.sh --push      # build, then adb push to Termux
#   ./scripts/build-cross.sh --push -s RZCX915L9KJ
#
# A static musl binary runs on Android's kernel with no Termux packages
# installed, since it carries its own libc and there is no OpenSSL to link.

set -euo pipefail
cd "$(dirname "$0")/.."

TARGET=aarch64-unknown-linux-musl
BIN=target/$TARGET/release/harness
PUSH=0
SERIAL=""

while [ $# -gt 0 ]; do
  case "$1" in
    --push) PUSH=1 ;;
    -s|--serial) shift; SERIAL="$1" ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

if ! rustup target list --installed | grep -qx "$TARGET"; then
  echo ">> adding target $TARGET"
  rustup target add "$TARGET"
fi

echo ">> building for $TARGET"
cargo build --release --target "$TARGET"

SIZE=$(du -h "$BIN" | cut -f1)
echo ">> built $BIN ($SIZE)"

if [ "$PUSH" -eq 0 ]; then
  echo
  echo "push it yourself with:"
  echo "  adb push $BIN /sdcard/Download/harness"
  echo "  # then in Termux:"
  echo "  mkdir -p ~/.local/bin && cp /sdcard/Download/harness ~/.local/bin/ && chmod +x ~/.local/bin/harness"
  exit 0
fi

ADB=(adb)
if [ -n "$SERIAL" ]; then
  ADB+=( -s "$SERIAL" )
fi

# Shared storage, because an app-private directory is not writable from adb
# without a debuggable Termux build.
DEST=/sdcard/Download/harness
echo ">> pushing to $DEST"
"${ADB[@]}" push "$BIN" "$DEST"

echo ">> done. now in Termux:"
echo "   mkdir -p ~/.local/bin"
echo "   cp /sdcard/Download/harness ~/.local/bin/"
echo "   chmod +x ~/.local/bin/harness"
echo "   export PATH=\$HOME/.local/bin:\$PATH   # add to ~/.bashrc to keep it"
echo "   harness"
