# Moving a PDF tab to a new window

On Windows, drag a document tab away from the tab strip and release to move it to
another native PdfCraft window. Drag along the strip to reorder tabs as before;
its edges still scroll a long tab list. Escape cancels a drag. There is a 24-point
dead zone outside the strip so small vertical pointer movements do not detach tabs.
Dragging horizontally past the window edge also tears off a tab.

The tab context menu and command palette offer **Move Tab to New Window**. The
opt-in UI control channel exposes the same action as `ui.command` with
`{"id":"file.detach_tab"}`. Browser and other desktop hosts leave it disabled.

The transfer moves the existing engine document and view in memory. It preserves
unsaved PDF bytes, Undo/Redo, the original save path, encryption state, pending
in-view typing, zoom/page position, and crash-recovery ownership. No temporary PDF
is written and no implicit save happens. File-watcher stamps move too, so a change
on disk still triggers the existing overwrite protection.

Each window uses the normal Save/Discard/Cancel guard. Closing the original window
keeps detached windows alive. A later external file-open request can show the
original window again. Moving a tab while a modal, comparison, or mutating background
operation is active is disabled until that interaction finishes.

This adds tear-off into a **new** window. Dragging tabs into an existing window,
restoring the arrangement of several windows after restart, and native tear-off
on macOS/Linux or the web are not included.
