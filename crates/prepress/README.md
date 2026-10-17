# pdfcraft-prepress

Print production (L4): the marks and checks a commercial printer needs on a page before it goes
to plate. Today that is **printer marks**; the rest of Acrobat's Print Production panel (output
preview, separations, ink manager, convert colours, fix hairlines) is still planned
(`docs/gaps.md` gap 18).

## Printer marks

`add_printer_marks` places crop marks, bleed marks, registration targets, a colour bar and a
page-information line around a chosen page box, as **printer's mark annotations**
(ISO 32000-2 §12.5.6.20, the subtype the spec defines for exactly these graphics). Each mark is
one annotation with its own appearance stream, tagged with the spec's `/MN` mark name
(`/CropMark`, `/BleedMark`, `/RegistrationTarget`, `/ColorBar`, `/PageInformation`).

Annotations rather than page content, because:

- marks stay removable (`remove_printer_marks`), so adding them is not a destructive edit;
- the page's content stream and resources are untouched, which keeps save fidelity;
- the rest of the codebase already handles the subtype correctly: `a11y` keeps printer marks out
  of the structure tree, `redact` keeps them, `render`'s inspector skips them.

Only marks PdfCraft wrote carry `/PCPrinterMark`, and removal touches nothing else, so marks a
different producer put on the page survive.

### Geometry

Boxes resolve as ISO 32000-2 §14.11.2 requires: the crop box defaults to the media box, and the
bleed, trim and art boxes default to the crop box, each clipped to the media box. Crop marks sit
at the corners of the chosen reference box (the trim box by default) and bleed marks at the
corners of the bleed box, both in a band `offset` away from the edge. Registration targets are
centred on each side, and the colour bar and page-information line go beyond that band, so no two
mark kinds overlap whatever the style.

Marks reach outside the reference box by design, so they usually need room in the media box.
`grow_media` enlarges it to fit; without it the marks that fall outside are reported as clipped
rather than silently lost.

### Colour

Crop, bleed and registration marks and the page-information text are drawn in a `/Separation`
with the colorant name `All` (§8.6.6.4), which paints on every plate, so the marks appear on all
separations. The colour bar's patches are `DeviceCMYK`.

### Known limits

- Marks are placed in unrotated user space, so on a page with `/Rotate` they stay locked to the
  trim geometry and the page-information text turns with the page rather than reading upright.
- Page-information text is drawn in Helvetica (WinAnsiEncoding); text outside it is refused
  rather than written as `?`.
