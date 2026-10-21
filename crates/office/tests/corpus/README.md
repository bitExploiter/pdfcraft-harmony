# Conversion fidelity corpus

Synthetic PDFs that `tests/fidelity.rs` converts to Word and PowerPoint and compares with their
sources. Every text in `src/` was written for this corpus by PdfCraft contributors; the PDFs were
made from those sources by WordCraft and DeckCraft (`make.sh`, with `wordcraft-cli` and
`deckcraft-cli` 0.8.0), and `deck-sample.pdf` is DeckCraft's own sample presentation. No
third-party or user document is included. Each PDF is listed in `ATTRIBUTION.toml`.

| File | What it exercises |
|---|---|
| `letter`, `outline`, `long` | headings and paragraphs, one to four pages |
| `turkish` | Turkish letters (ğ, ş, ı, İ) through the fonts' encodings |
| `table`, `landscape` | a table, on a portrait and a landscape page |
| `lists` | bulleted and numbered lists |
| `styled` | alignment (centred, right), italic, serif, sans and monospace families |
| `columns` | two text columns |
| `justified` | justified paragraphs over four pages |
| `deck-plan`, `deck-turkish`, `deck-sample` | slides: a text layer over a picture of each slide |

Beside each `<name>.pdf` is `<name>.txt`, the source's own plain text (WordCraft's text export, or
DeckCraft's outline). `floor.toml` holds, for each converted file, the lowest accepted share of the
source's words found in order and of its paragraphs found whole; the floors only go up.

    cargo test -p pdfcraft-office --test fidelity -- --nocapture       # the table
    FIDELITY_SHOW=lists.docx cargo test -p pdfcraft-office --test fidelity -- --nocapture
    FIDELITY_WRITE=1 cargo test -p pdfcraft-office --test fidelity     # new floors, after a gain

Rebuilding the PDFs (`./make.sh`) changes their bytes: update their `sha256` in `ATTRIBUTION.toml`.
