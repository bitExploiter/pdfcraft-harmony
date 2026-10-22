# macOS reader windows and thumbnail navigation

On macOS, each document opens in an independent native window. Opening a document closes the Tools pane and opens Pages. This reading layout takes priority over the PDF's requested navigation pane; document page, zoom and page-layout settings continue to apply. Closing the last document hides its window and keeps the application running. Explicit Quit retains the save, discard and cancel prompts for unsaved documents.

Page thumbnails use a fixed 96-point image width. Widening the Inspector adds columns instead of enlarging the thumbnails, and the Inspector is no longer capped at 520 points. Click a thumbnail to navigate, use the existing selection modifiers to select pages, or drag pages to reorder them. Tabbed frontends keep their existing opening-pane rules.

Opaque images can use smooth downsampling during normal page rendering with `PDFCRAFT_SMOOTH_IMAGES=1`. This display option defaults to off and leaves PDF bytes unchanged. Preview rendering, image magnification, masks and Type 3 glyph images keep their existing sampling behavior.
