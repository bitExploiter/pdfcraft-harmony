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
The existing licence allowlist is unchanged. GPL fonts declaring an embedding
exception, including TH Sarabun New, are not accepted by the file importer;
that policy decision is separate from Add Text. External test inputs and PDFs
already embedding those programs can exercise the shared shaping/writing path.

The PDF stores actual glyph positions, cluster ToUnicode mappings and logical
ActualText. Wrapping never separates a shaping cluster, including Thai vowel
and tone marks; it prefers spaces and otherwise breaks at cluster boundaries.
It does not perform dictionary-based word segmentation. The complete font is
shared between mappings and items, and a reference in `/PCAdded /CustomFont`
retains the choice after full or incremental saves. Moving, resizing and
retyping therefore require no original file or installed font. An unavailable
or invalid saved font stays explicit and editing fails visibly, preserving the
document. Unsupported glyphs and late embedding failures are atomic errors.
Automatic Inter fallback retains its automatic status on reopen, so retyping
plain Latin returns to the standard face. Explicit Inter selection stays explicit.
Bold/italic controls are disabled while a complete face supplies the style.
Coverage checks allow Unicode default-ignorable controls (joiners, bidi controls
and variation selectors) without requiring visible cmap glyphs; shaping still
receives them unchanged. Selected-font layout accepts CRLF and lone CR as row
separators, retaining the original typed text in the editable marker.

The font catalogue is cached for the current page and COS document revision;
decoded native Unicode programs are shared through `Arc` across items and refreshes
in a bounded 32 MiB/128-program cache. Legacy preview cmaps synthesized from PDF
mappings are not shared across mappings. Subsets and preview-only approved faces
remain available to Edit Text, but cannot be chosen as full Add Text faces.
One bounded layout per thread reuses shaping for measurement, validation and writing,
with one parsed shaper per layout. Horizontal glyphs use batched `TJ` arrays;
vertical mark offsets start another array without changing their positions.
Loading an Add Text font changes only the draft/style; embedding happens with the
single Add/Update undo step. Discarding a draft leaves document history untouched.

Headless use follows the existing font workflow:

1. Call `text_font_import` with `doc`, `page` and the font `path`, or inspect
   `text_fonts` for an existing resource. File paths retain the usual `--root`
   confinement; repeated imports return the reusable resource.
2. Pass the returned `font_resource` to `page_add_text` or `content_update`.
   It is mutually exclusive with `font`, `bold` and `italic` arguments.
3. `content_list` reports the selected face; subsequent text updates preserve it.
   Style-only updates to a retained selected face refuse unsupported flags;
   explicitly choosing a standard `font` clears the selection and permits its style.
   Importing a program already present as a subset returns a complete resource.

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
Latin. Thai and CFF tests return early without these external inputs (the Thai
edit test can also use a suitable craft-fonts face). CI does not set these
variables: the real-font scenarios and
`cff_encoding_cmap_declares_its_character_collection` are local opt-in checks,
not unconditional CI evidence. The existing engine test
`opentype_cff_document_font_preserves_program_codes_and_rendering_after_reopen`
runs unconditionally with a synthetic CFF face. It exercises the CFF writer and
save/reopen/rendering, but does not assert the CMap metadata invariants.
With TH Sarabun New, the automation
test uses an externally embedded PDF font and checks that file import is refused
by the unchanged policy. With allowed fonts it covers import/reuse, saving, reopening, extraction,
search, rendering, retyping, undo/redo and missing-resource refusal.
