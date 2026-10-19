# pdfcraft-collate

Text collation for the lists PdfCraft sorts and filters: comment authors, bookmarks, form
choices and file names.

- **Layer:** L0, standalone, no dependencies. `#![forbid(unsafe_code)]` (workspace lint) and the
  crate denies `unwrap`/`expect`/`panic!`/`todo!`. std only, no dependencies; the wasm gate runs in CI.
- **Canonical equivalence.** `é` precomposed equals `e` + U+0301, in any case and with marks in
  either order: `text_eq`, `fold_key`, `compare` and `text_key` all agree.
- **Order.** `Collation::Root` puts accented letters with their base letter; `Collation::Vi` is
  the CLDR `vi` order (`a ă â b c d đ e ê …`, tones as a secondary difference). `natural` sorts
  digit runs by value on top of either.
- **Search.** `search_contains` ignores case and encoding, matches whole letters (`e` is not in
  `Nguyễn`) and keeps the viewer's Turkish `i` and Greek sigma folds (`search_fold`).
- **Never changes text.** It builds keys and answers comparisons; callers reorder, never rewrite.
- **Data.** `src/ucd.rs` holds the Unicode 17.0 decomposition and combining-class tables
  (Unicode License v3, header of the file, listed in `NOTICE`). They are checked against ICU4X
  `icu_normalizer`, and the `vi` order against ICU4X `icu_collator`, both dev-dependencies only.
- Limits: a run of more than 32 combining marks is not reordered; a key holds at most 16 384
  primary elements and the final tie-break compares the whole folded text. A digit run in a natural
  key is one element however long, so such a key can exceed that by the digits of one run; still
  linear.
