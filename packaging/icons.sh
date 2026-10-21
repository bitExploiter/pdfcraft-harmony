#!/usr/bin/env bash
# Regenerate the Windows PDF file icon from assets/app-icon/pdfcraft-document.svg (master vector).
# The app icon itself is Design Kreativ's (assets/brand/, rendered from kreativpdf-1024.png).
#
# Needs: resvg (brew install resvg / cargo install resvg) and python3 (stdlib only, for the .ico).
# The output is committed, so building and packaging never need these tools. After running it,
# update the sha256 values in ATTRIBUTION.toml, then `cargo xtask assets --write`.
#
#   packaging/icons.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/assets/app-icon"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

command -v resvg >/dev/null || { echo "error: resvg not found (brew install resvg)" >&2; exit 1; }

render() { resvg -w "$2" -h "$2" "$1" "$3" </dev/null; }


# Windows .ico: PNG-compressed entries, 16-256 px, embedded as resource 2 in pdfcraft.exe.
for name in pdfcraft-document; do
  ICO_PNGS=()
  for s in 16 20 24 32 40 48 64 128 256; do
    render "$DIR/$name.svg" "$s" "$TMP/$name-$s.png"
    ICO_PNGS+=("$TMP/$name-$s.png")
  done
  python3 - "$DIR/$name.ico" "${ICO_PNGS[@]}" <<'PY'
import struct, sys
out, pngs = sys.argv[1], sys.argv[2:]
blobs = [open(p, "rb").read() for p in pngs]
head = struct.pack("<HHH", 0, 1, len(blobs))
entries, data, offset = b"", b"", 6 + 16 * len(blobs)
for b in blobs:
    w, h = struct.unpack(">II", b[16:24])  # IHDR
    entries += struct.pack("<BBBBHHII", w % 256, h % 256, 0, 0, 1, 32, len(b), offset)
    data += b
    offset += len(b)
open(out, "wb").write(head + entries + data)
PY
done

echo "icons written to $DIR"
