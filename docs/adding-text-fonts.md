# Fonts in Add Text

Add Text and Edit Text share `pdfcraft-fonts::source_shaping` (harfrust OpenType
shaping, Unicode script runs and UAX #9 bidi) and `embedding` (Type 0 fonts,
Unicode CIDs, glyph mappings, widths and the complete embedded program).

In the Format text panel, choose a document font or **Load font…** to select a
static TTF/OTF file. Desktop builds also offer installed document fonts through
the existing bounded, on-request discovery. The chosen file supplies its own
weight and style: load its bold/italic face to use that face. Variable fonts and
collections still require a standalone static instance, as in Edit Text.

Without a selected file, existing Western text retains its standard PDF face.
Text outside that encoding uses bundled Inter when Inter covers the complete
text, including Cyrillic and Greek. Existing Arabic, Hebrew and Thai build-input
fallbacks remain available. Other scripts can use an explicitly selected face;
there is no language-specific font import path.

The font program is document input, not an interface fallback. Import retains
the existing validation of format, size, Unicode mapping and embedding rights.
The name-table licence check also recognizes the GPL font-embedding exception
declared by TH Sarabun New. This does not bundle that font or add GPL fonts to
the application's asset manifest.

The PDF stores actual glyph positions, cluster ToUnicode mappings and logical
ActualText. Wrapping never separates a shaping cluster, including Thai vowel
and tone marks; it prefers spaces and otherwise breaks at cluster boundaries.
It does not perform dictionary-based word segmentation. The complete font is
shared between mappings and items, and a reference in `/PCAdded /CustomFont`
retains the choice after full or incremental saves. Moving, resizing and
retyping therefore require no original file or installed font. An unavailable
or invalid saved font stays explicit and editing fails visibly, preserving the
document. Unsupported glyphs and late embedding failures are atomic errors.

Headless use follows the existing font workflow:

1. Call `text_font_import` with `doc`, `page` and the font `path`, or inspect
   `text_fonts` for an existing resource. File paths retain the usual `--root`
   confinement; repeated imports return the reusable resource.
2. Pass the returned `font_resource` to `page_add_text` or `content_update`.
   It is mutually exclusive with the standard `font` argument.
3. `content_list` reports the selected face; subsequent text updates preserve it.

Regression tests use the already bundled Inter. Additional real-font checks
accept external inputs, never committed font fixtures:

```text
PDFCRAFT_TEST_THAI_FONT=/path/to/THSarabunNew.ttf
PDFCRAFT_TEST_OTF_FONT=/path/to/LibertinusSerif-Regular.otf
cargo test -p pdfcraft-fonts -p pdfcraft-edit -j 2 --lib
cargo test -p pdfcraft-automation -j 2 --test automation added_unicode_document_fonts_through_tools
cargo test -p pdfcraft-ui-egui -j 2 --lib content_ui::tests
cargo test -p pdfcraft-ui-egui -j 2 --test content -- --test-threads=1
```

The Thai edit test also runs with a suitable optional craft-fonts Thai face.
The OTF input in these tests must cover Cyrillic, Greek, combining acute and
Latin. The automation test covers import/reuse, saving, reopening, extraction,
search, rendering, retyping, undo/redo and missing-resource refusal.
