# Text reading order and normal selection

The renderer's `PageText` supplies the reading order shared by normal selection, highlighting,
copying, search, plain-text export and headless extraction. Fix layout there; filtering the clipboard or
clipping a mouse drag to a rectangle would leave those consumers inconsistent and prevent
intentional selection across columns.

## Execution path

- `vendor/hayro-interpret/src/interpret/text.rs` interprets text positioning and font operations.
- `crates/render/src/text.rs::TextDevice` receives decoded glyphs and transforms their advances
  and em boxes into page view coordinates. It handles marked-content `ActualText` replacements.
- `extract_page_filtered` separates writing directions, normalizes each flow into its baseline
  frame, calls `arrange_flow`, and maps the resulting indices back to the original glyph quads.
- `arrange` partitions whitespace regions before `arrange_region` reconstructs runs, removes
  redraws, places overlapping glyphs, infers word gaps and reverses visual RTL runs.
- `crates/render/src/raster.rs` supplies `PageText` through `PageRenderer` and `RenderPool`.
- `crates/ui-egui/src/canvas.rs` caches that layer. A normal drag hit-tests endpoints with
  `PageText::nearest` and stores `Span::Run { anchor, head }`. The inclusive endpoint range
  drives both `line_rects` for highlights and `text_of` for copied text. Word/line clicks use the
  same layout. `crates/ui-egui/src/lib.rs::copy_selected_text` enforces document copy permission.
- `crates/automation/src/lib.rs::text_extract` and `apps/pdfcraft-cli/src/main.rs` consume the
  same renderer text. Rectangular selection remains a separate, explicit operation.

Editing and rich Office export also use `crates/edit/src/text.rs` and `text_regions.rs` text
models. Those structures do not drive the viewer's normal mouse selection and are unchanged
by this fix; improving their semantic reading order is a separate task.

## Confirmed defect

The former reconstructor joined consecutive same-baseline glyphs across gaps smaller than
three glyph heights, then merged separate same-baseline segments across gaps smaller than
1.2 heights. Neither stage knew about persistent column gutters. Magazine gutters smaller
than those tolerances became word spaces. Stacked blocks then inherited cross-column lines;
their bounding boxes no longer represented individual columns. Normal selection correctly
selected a contiguous range of an incorrectly reconstructed order, with matching incorrect
highlight rectangles and copied text.

Reducing only the second tolerance leaves the first path open for row-major paint order.
Reducing both globally also weakens reconstruction of tracked words and separately drawn
fragments. Content stream order alone is insufficient: real producers paint by column, by row,
by font, or in other orders.

## Whitespace regions

`crates/render/src/text/regions.rs` performs an original implementation of geometric XY
subdivision. It projects nonblank glyph boxes onto both axes. A vertical cut must be wider than
ordinary letter tracking plus word spacing (and half the local median em height), with overlapping multiline text
on both sides. A horizontal cut exposes spanning headings, section changes and footers. The
strongest gaps compete in local word/line-spacing units, allowing a large section break to
take priority over a gutter. Equal gaps prefer balanced subdivisions to avoid unnecessarily
deep trees. Horizontal subdivisions are retained only when they expose a supported column
split in their subtree; otherwise the original region and paint order are restored and line
grouping remains with the existing reconstructor. All tolerances scale with the typography.
Tracking is measured in visual bands using the same lower-third statistic as word grouping,
so widely spaced lettering is not interpreted as one column per letter.

Leaf regions retain original glyph indices in paint order. The existing line reconstructor
runs independently inside each leaf; it cannot join text across an established gutter.
Concatenating leaves and offsetting their line identifiers gives every consumer one consistent
reading order. There is no new public mode, dependency, page-size assumption or magazine rule.
Existing direction normalization, original quads and ligature boundaries are preserved.
Detected curved runs retain their continuity-based placement. Straight flows classified as
turned by the existing extractor are segmented in their baseline frame when direction is
uniform (covered at 30 degrees). Non-quarter-turn frames combining different angles retain
existing placement rather than inventing columns from approximate boxes. This preserves
decorative map labels. The extractor's existing orientation-classification tolerance remains.

Subdivision is iterative with a depth limit of 64 and falls back to the existing reconstructor
at that limit or on nonfinite geometry. Axis indices are sorted once, then filtered at each
cut; prefix and suffix bounds make testing all cut candidates linear. Tracking samples sort
within visual bands only when a potential gutter exists. Pruned regions merge
their original indices in linear time. Glyphs are referenced, not cloned. The new pass uses
O(n) live auxiliary storage and bounded O(64 n log n) work. Existing per-leaf line
reconstruction retains its previous complexity and benefits from smaller inputs.

## Verification

```text
cargo test -p pdfcraft-render -j 2
cargo test -p pdfcraft-ui-egui -j 2 --test ui_group text_select
cargo clippy -p pdfcraft-render -j 2 --all-targets -- -D warnings
cargo check -p pdfcraft-render -j 2 --target wasm32-unknown-unknown
```

`crates/render/tests/column_reading.rs` generates original PDFs in memory, including narrow
gutters, full-width headings, section layout changes, short table cells, text beside an
illustration, scale/translation invariance and reordered glyphs. It checks logical ranges and
highlight bounds, not merely the presence of words. The UI regression drags without modifiers
in both directions and checks the emitted clipboard content.

The private magazine regression is intentionally ignored by default. Set
`PDFCRAFT_COLUMN_PDF` to the local supplied PDF, then run:

```text
cargo test -p pdfcraft-render -j 2 --test column_reading magazine_ -- --ignored
cargo test -p pdfcraft-ui-egui -j 2 --test ui_group magazine_page_twelve_plain_mouse_selection_and_copy -- --ignored
```

Do not commit the magazine, comparison screenshots, or fonts extracted from them. The regression
uses page 12 (zero-based index 11), selects a known left-column paragraph, and verifies that every
highlight stays to the left of the middle column. The UI check sends plain pointer events and
checks clipboard output; the other private render check protects page 49's decorative labels.
The narrow-column and page-12 regressions were observed failing before the fix; typography and
map-label checks also caught regressions during development.

## Practical limits

Geometric subdivision is a fallback, not a proof of author intent. Untagged tables and prose
columns can have identical geometry. Normal table order remains column-oriented as before;
explicit rectangular extraction retains row/tab output. No semantic table-cell model is added.
This change does not yet use a tagged PDF's structure tree as the authority for reading order.
Overlapping columns, gutters obstructed on every baseline, very tight gutters indistinguishable
from word spacing, complex nonrectangular flows and RTL/mixed reading-region order need further
corpus work. Existing cross-page selection and dehyphenation limitations are unchanged.

Passing the regression suite and the supplied magazine is not a claim of universal Acrobat
parity. Further work should add structure-tree ordering with validated geometric fallback and
broader independently annotated reading-order corpora.

## Design references

- [PDF Association: logical content order](https://pdfa.org/techniques-for-accessible-pdf/):
  tagged content can express authorial order independently of paint order.
- [pdfminer.six layout analysis](https://pdfminersix.readthedocs.io/en/latest/topic/converting_pdf_to_text.html):
  character/line proximity and hierarchical box grouping are separate reconstruction stages.
- [Meunier, Optimized XY-cut for determining a page reading order, ICDAR 2005](https://doi.org/10.1109/ICDAR.2005.182):
  whitespace subdivision and block ordering provide context for this geometric design.

No third-party layout implementation was copied. Copyleft engines are external inspection
tools only; they are not linked or added as product dependencies by this change.
