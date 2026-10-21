#!/bin/sh
# Rebuilds the corpus: each PDF from its source in src/ with WordCraft or DeckCraft, and beside it
# the source's own text (<name>.txt), which the converted document is compared with.
# Needs wordcraft-cli and deckcraft-cli on PATH. Run from this folder.
set -eu
w() { wordcraft-cli "$@" >/dev/null; }
d() { deckcraft-cli "$@" >/dev/null; }
for n in letter turkish table lists outline long; do
  w convert "src/$n.md" "$n.pdf"
  w convert "src/$n.md" "$n.txt"
done
w convert src/styled.html styled.pdf
w convert src/styled.html styled.txt
w run --file src/letter.md --cmd 'layout.columns={"count":2,"apply":"document"}' --save columns.docx
w convert columns.docx columns.pdf && w convert columns.docx columns.txt && rm columns.docx
w run --file src/table.md --cmd 'layout.orientation={"value":"landscape"}' --save landscape.docx
w convert landscape.docx landscape.pdf && w convert landscape.docx landscape.txt && rm landscape.docx
w run --file src/long.md --cmd 'select.all={}' --cmd 'para.justify={}' --save justified.docx
w convert justified.docx justified.pdf && w convert justified.docx justified.txt && rm justified.docx
for n in deck-plan deck-turkish; do
  d convert "src/$n.txt" "$n.pdf"
  d convert "src/$n.txt" "$n.md" && mv "$n.md" "$n.txt"
done
d run --sample --save deck-sample.pdf
d run --sample --save deck-sample.md && mv deck-sample.md deck-sample.txt
