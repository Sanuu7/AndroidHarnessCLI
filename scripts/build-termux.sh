#!/data/data/com.termux/files/usr/bin/bash
# Build harness on the phone itself.
#
#   pkg install rust
#   ./scripts/build-termux.sh
#
# First build takes a few minutes on a phone (ratatui pulls in a fair amount
# of proc-macro machinery). Incremental builds after that are quick.

set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. run: pkg install rust" >&2
  exit 1
fi

echo ">> building release"
cargo build --release

BIN=target/release/harness
SIZE=$(du -h "$BIN" | cut -f1)
echo ">> done: $BIN ($SIZE)"
echo
echo "run it with:"
echo "  $PWD/$BIN"
echo
echo "to put it on PATH:"
echo "  cp $PWD/$BIN \$PREFIX/bin/harness"
