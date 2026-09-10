#!/data/data/com.termux/files/usr/bin/bash
#
# Install Android Harness CLI.
#
#   curl -fsSL https://raw.githubusercontent.com/Sanuu7/AndroidHarnessCLI/main/install.sh | bash
#
# Options (only when running the script directly, not through the pipe):
#   --dir <path>   install somewhere else than $PREFIX/bin or ~/.local/bin
#   --ref <ref>    install from another branch or tag, default main
#
# The binary is static, so nothing else has to be installed first.

set -euo pipefail

REPO=Sanuu7/AndroidHarnessCLI
REF=${HARNESS_REF:-main}
DEST=${HARNESS_BIN_DIR:-}

usage() {
  sed -n '3,10p' "$0" | sed 's/^# \{0,1\}//'
  exit 0
}

while [ $# -gt 0 ]; do
  case "$1" in
    --dir) shift; DEST="${1:?--dir needs a path}" ;;
    --ref|-r) shift; REF="${1:?--ref needs a value}" ;;
    -h|--help) usage ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

for tool in curl uname; do
  command -v "$tool" >/dev/null 2>&1 || { echo "install.sh needs $tool" >&2; exit 1; }
done

case "$(uname -m)" in
  aarch64|arm64) ARCH=aarch64 ;;
  x86_64|amd64) ARCH=x86_64 ;;
  *) echo "no build for $(uname -m) yet. aarch64 and x86_64 are published." >&2; exit 1 ;;
esac

URL="https://raw.githubusercontent.com/$REPO/$REF/dist/harness-$ARCH"

if [ -z "$DEST" ]; then
  # Termux sets PREFIX, and its bin is already on PATH.
  if [ -n "${PREFIX:-}" ] && [ -d "$PREFIX/bin" ]; then
    DEST="$PREFIX/bin"
  else
    DEST="$HOME/.local/bin"
  fi
fi
mkdir -p "$DEST"

TMPDIR_LOCAL="${TMPDIR:-$DEST}"
[ -d "$TMPDIR_LOCAL" ] || TMPDIR_LOCAL=$DEST
TMP=$(mktemp "$TMPDIR_LOCAL/harness.XXXXXX" 2>/dev/null || echo "$DEST/.harness.tmp")
trap 'rm -f "$TMP"' EXIT

echo ">> downloading harness ($ARCH, $REF)"
if ! curl -fsSL "$URL" -o "$TMP"; then
  echo "download failed: $URL" >&2
  echo "the ref '$REF' or the arch '$ARCH' may not have a build." >&2
  exit 1
fi

# A static ELF, nothing else. Catches an HTML error page saved as a binary.
if [ "$(head -c 4 "$TMP" | od -An -tx1 | tr -d ' \n')" != "7f454c46" ]; then
  echo "downloaded file is not an ELF binary, aborting" >&2
  exit 1
fi

chmod 755 "$TMP"
mv -f "$TMP" "$DEST/harness"
trap - EXIT

SIZE=$(du -h "$DEST/harness" | cut -f1)
echo ">> installed $DEST/harness ($SIZE)"

case ":$PATH:" in
  *":$DEST:"*) echo ">> run it with: harness" ;;
  *)
    echo ">> $DEST is not on your PATH. add it:"
    echo "     export PATH=\"$DEST:\$PATH\""
    echo "   and put that line in ~/.bashrc to keep it."
    ;;
esac
