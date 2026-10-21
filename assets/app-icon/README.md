# KreativPdf icons

The app icon is Design Kreativ's, kept with the rest of the brand in [`assets/brand/`](../brand/)
(a Design Kreativ trademark, not open source: see `assets/brand/LICENSE-brand.txt`):

| File | What it is |
|---|---|
| `assets/brand/kreativpdf-1024.png` | 1024 px; the runtime Dock icon on macOS, and the source of the rest |
| `assets/brand/kreativpdf-256.png` | the runtime window icon on Windows and Linux |
| `assets/brand/kreativpdf.ico` | Windows icon, embedded in `pdfcraft.exe` by `apps/pdfcraft/build.rs` |
| `assets/brand/kreativpdf.icns` | macOS app bundle icon (16–1024 px) |
| `assets/brand/hicolor/<n>x<n>/apps/ai.storyteller.pdfcraft.png` | Linux hicolor theme, 16–512 px |

This directory holds the Windows PDF *file* icon:

| File | What it is |
|---|---|
| `pdfcraft-document.svg` | Windows PDF file icon master: a Lucide document outline, green badge and outlined PDF lettering; no font dependency |
| `pdfcraft-document.ico` | Windows PDF file icon (16–256 px), embedded as resource 2 in `pdfcraft.exe` (`packaging/icons.sh`) |

Where they show: `apps/pdfcraft/src/main.rs` sets the window icon (Dock, taskbar, Alt-Tab, launcher) and the
Wayland app id `ai.storyteller.pdfcraft`; `packaging/linux/ai.storyteller.pdfcraft.desktop` names the
hicolor icon.

Windows PDF files use resource 2 (`"pdfcraft.exe",-2`) when KreativPdf is chosen as the default
reader; the app keeps resource 1. The document outline is adapted from Lucide `file-text`
1.49.0 (ISC); its badge and outlined lettering are original (`MIT OR Apache-2.0`), covered by
[LICENSE-document.txt](LICENSE-document.txt). Both icons are embedded, requiring no runtime
sidecar. This provides a file type icon; first-page thumbnails still need a thumbnail provider.

## Regenerate

```sh
packaging/icons.sh        # needs resvg and python3; iconutil (macOS) for the .icns
cargo xtask assets        # then update the sha256 values in ATTRIBUTION.toml and run with --write
```
