# Text reading order and normal selection

The renderer's `PageText` supplies the reading order shared by normal selection, highlighting,
copying, search, plain-text export and headless extraction. Fix layout there; filtering the clipboard or
clipping a mouse drag to a rectangle would leave those consumers inconsistent and prevent
intentional selection across columns.

## Execution path

- `vendor/hayro-interpret/src/interpret/text.rs` interprets text positioning and font operations.
  The optional `Device::begin_text_run` notification marks each complete `Tj`, `TJ`, `'` or
  `"` operation; `begin_text_string` marks its individual string operands. Both default
  to no-ops for raster devices.
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
on both sides. Each side also needs adjacent, substantially filled prose lines that reach the
candidate edge. Repeated whitespace beside isolated cells or list markers is insufficient.
A supported continuous gutter always takes precedence over horizontal
whitespace: even a large aligned paragraph break must not interleave neighbouring columns.
When spanning content obstructs the gutter, a horizontal cut exposes headings, section
changes and footers before their local column layouts. Empty space alone cannot establish
a semantic section break. Equal gaps prefer balanced subdivisions to avoid unnecessarily
deep trees. The subdivision tree is retained through reconstruction of reading order.
Adjacent horizontal bands with compatible empty gutter corridors share complete columns,
even when a heading or footer initially blocked those corridors at the page level.
Intersecting corridors identify continuity without a paragraph-gap cutoff; ambiguous
one-to-many matches are left separate. A heading spanning only some columns stays inside
their group. Spanning text leaves and proven table rows remain barriers, so actual section
headings still separate layouts. Horizontal subdivisions are retained when they expose supported columns or
row relationships: paired labels and values, or sparse cells separated by row whitespace.
Other horizontal splits restore the original region and paint order, leaving line
grouping to the existing reconstructor. All tolerances scale with the typography.
Tracking is measured in visual bands using the same lower-third statistic as word grouping,
so widely spaced lettering is not interpreted as one column per letter.

Word-space evidence includes the actual advance after explicit spaces, including `Tw`.
Such advances are measured within one string operand: a trailing space must not borrow
the position of an adjacent column's first glyph.
A single word-space advance covering an entire candidate corridor also vetoes that cut.
This local evidence protects a larger heading whose space is wider than the body gutter;
the page's median space alone cannot describe mixed font sizes. Repeated alignment spaces
do not combine into such a bridge.
`text/spacing.rs` estimates implicit spaces within individual text-show operations when no
Unicode spaces are available, protecting justified `TJ` text. Show provenance is evidence
about spacing, not authority for reading order: one `TJ` can still paint several columns.
When separately positioned word shows provide no spacing hints, visual rows with several
non-letter gaps supply a fallback estimate. Uniform character advances bound it by a
character cell; proportional advances use the local em, including lines containing only
narrow lowercase letters. Advances are normalized to their own glyph height so changes of
point size do not masquerade as proportional spacing. This does not impose a three-line
minimum on short or staggered columns.
CJK character gaps do not contribute to that inter-word estimate; unspaced CJK columns
remain covered independently of the Latin prose fixtures.
Conventional list markers (including multi-digit, alphabetic and bracketed numbering) and
colon-terminated field labels are excluded from prose evidence. Established single-row
leaves allow distant label/value fragments to join; wrapped table cells retain their own
lines within each row. Rectangular extraction still splits distant cell pieces for tab output.
It finds empty projection gaps in visual order before grouping logical glyphs. A bidi number
or Latin run can jump across intervening ink in logical order; that jump is not an empty cell
gap. Cell pieces from one logical line keep their logical order, including neutral numeric
cells; pieces from different lines retain the existing spatial row ordering. This preserves
mixed RTL rows without suppressing real gaps between RTL table cells.

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
their original indices in linear time. Column-band normalization also bounds its recursive
descent at 64 and matches ordered gutter corridors in linear time. Glyphs are referenced,
not cloned. Live auxiliary storage is O(n); subdivision and normalization have bounded
depth. Existing per-leaf line
reconstruction retains its previous complexity and benefits from smaller inputs.
Rectangular copying sorts the selected glyphs of each logical line for cell-gap analysis,
using O(k log k) time and O(k) auxiliary storage for k selected glyphs. Normal copying does
not run this additional projection pass.

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
in both directions and checks the emitted clipboard content, including aligned paragraph
breaks. The paragraph regression checks two and three columns over four paragraph spacings,
with distinct text in every column and paragraph, and verifies cross-paragraph search bounds.
The aligned-paragraph regression was observed failing with the previous horizontal-gap
preference, as was the expanded plain-mouse/clipboard regression.
Additional regressions cover headings at 14/20/30 points over a ten-point body, headings
and footers enclosing two/three columns with paragraph gaps up to 180 points, and a heading
over only two of three columns. The larger-font and headed-paragraph cases were observed
failing before their fixes; the partial-heading case protects existing behavior.

`crates/render/tests/row_structure.rs` covers the contributor's justified `TJ`/`Tw` example,
implicit spaces without space glyphs, hanging bullets (also painted before their bodies),
per-word `Tj` without spacing hints in Courier/Helvetica and three paint orders,
numeric and alphabetic labels, dense form rows and single-line/wrapped tables. The UI
regression `normal_row_drags_copy_spaces_labels_and_cells_at_every_zoom` uses physical
word endpoints and independent source expectations, checks word boundaries, and copies
with plain drags in both directions at 50%, 100%, 200% and 400%.
The mixed Hebrew/Arabic, number, punctuation and Latin unit cases check rectangular copying
against independent logical strings, alongside the existing real-gap RTL table cases.

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

Geometric subdivision is a fallback, not a proof of author intent. Sparse tables, lists and
forms have regression coverage and retain row order. Dense wrapped tables and prose
columns can still have identical geometry; no semantic table-cell model is added.
This change does not yet use a tagged PDF's structure tree as the authority for reading order.
Overlapping columns, gutters obstructed on every baseline, very tight gutters indistinguishable
from word spacing, complex nonrectangular flows and RTL/mixed reading-region order need further
corpus work. Existing cross-page selection and dehyphenation limitations are unchanged.

Passing the regression suite and the supplied magazine is not a claim of universal Acrobat
parity. Further work should add structure-tree ordering with validated geometric fallback and
broader independently annotated reading-order corpora.

The contributor's external corpus is the PDF Association's
[accessible PDF techniques](https://github.com/pdf-association/techniques-for-accessible-pdf/tree/3e31095d8aeea941eccbbb766a23369374c2e9a6)
(CC BY 4.0). Keep that checkout outside this repository: it contains document/font assets
that are not product inputs. Differential extraction on its 82 PDFs is useful compatibility
evidence, but matching a baseline does not establish tag-tree conformance or reproduce the
contributor's independently annotated drag suite.

## Design references

- [PDF Association: logical content order](https://pdfa.org/techniques-for-accessible-pdf/):
  tagged content can express authorial order independently of paint order.
- [pdfminer.six layout analysis](https://pdfminersix.readthedocs.io/en/latest/topic/converting_pdf_to_text.html):
  character/line proximity and hierarchical box grouping are separate reconstruction stages.
- [Meunier, Optimized XY-cut for determining a page reading order, ICDAR 2005](https://doi.org/10.1109/ICDAR.2005.182):
  whitespace subdivision and block ordering provide context for this geometric design.

No third-party layout implementation was copied. Copyleft engines are external inspection
tools only; they are not linked or added as product dependencies by this change.
