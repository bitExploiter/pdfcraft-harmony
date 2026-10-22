//! Visual order for single-line interface text that may hold right-to-left script (file names,
//! document titles).
//!
//! egui shapes each font-face run with the script's own direction, so the letters of an Arabic
//! word join and run right to left, but it places the runs themselves left to right in logical
//! order: `واحد اثنين.pdf` would show its two words swapped. [`visual`] puts the runs in display
//! order and leaves the letters of each word alone for the shaper.
//!
//! [`display_rtl`] does the same for the labels of a right-to-left interface language (the Arabic
//! catalog), whose base direction is right to left.

use std::borrow::Cow;
use std::ops::Range;

use unicode_bidi::{BidiInfo, Level, ParagraphInfo};

/// `text` rearranged for an egui call that lays out one line. Text without right-to-left
/// characters comes back borrowed and unchanged.
///
/// The base direction is left to right, as the interface is: a file name keeps its extension
/// at the end, the way the window title shows it. Use it for painting only; accessibility
/// labels and anything stored keep the logical text.
pub fn visual(text: &str) -> Cow<'_, str> {
    if !text.chars().any(is_rtl_script) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    push_visual(&mut out, text, Level::ltr());
    Cow::Owned(out)
}

/// `text` in display order for the paragraph direction `base`, appended to `out`.
fn push_visual(out: &mut String, text: &str, base: Level) {
    let info = BidiInfo::new(text, Some(base));
    for para in &info.paragraphs {
        push_line(out, text, &info, para, para.range.clone());
    }
}

/// The `line` of paragraph `para` (a non-empty byte range of `text`) in display order, as egui
/// needs it, appended to `out`.
fn push_line(out: &mut String, text: &str, info: &BidiInfo<'_>, para: &ParagraphInfo, line: Range<usize>) {
    let (levels, runs) = info.visual_runs(para, line);
    // First the true display order, every character left to right as it appears on screen.
    let mut shown = String::with_capacity(text.len());
    for run in runs {
        let rtl = levels.get(run.start).is_some_and(Level::is_rtl);
        // Runs lie on character boundaries; a missing slice is skipped, never a panic.
        let Some(piece) = text.get(run) else { continue };
        let chars = piece.chars().filter(|c| !is_bidi_control(*c));
        if rtl {
            shown.extend(chars.rev().map(mirrored));
        } else {
            shown.extend(chars);
        }
    }
    push_for_shaper(out, &shown);
}

/// `shown` (in screen order) as egui must get it: egui hands each stretch of right-to-left
/// script to the shaper as one run, which lays it out right to left (and joins its letters), so
/// each such stretch goes back to logical order. What lies between them (spaces, punctuation,
/// Latin) is drawn by the Latin face left to right and stays as it is. The stretches include
/// Persian and Arabic-Indic digits: Arabic script to the shaper, though they read left to right.
fn push_for_shaper(out: &mut String, shown: &str) {
    let mut stretch: Vec<char> = Vec::new();
    for c in shown.chars() {
        if is_rtl_script(c) || is_joiner(c) {
            stretch.push(c);
        } else {
            out.extend(stretch.drain(..).rev());
            out.push(c);
        }
    }
    out.extend(stretch.drain(..).rev());
}

/// One line of wrapped text (see [`wrap`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The line in display order, for one egui call that lays out one line.
    pub text: String,
    /// The paragraph reads right to left: the line belongs against the right edge.
    pub rtl: bool,
}

/// One wrapped line slice with its logical byte range and display text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineSlice {
    /// The safe byte range in the logical text corresponding to this line.
    pub logical_range: Range<usize>,
    /// The line in display order.
    pub display_text: String,
    /// Whether this line is right-to-left.
    pub rtl: bool,
}

/// Whether `text` holds any right-to-left script, so [`wrap`] or [`visual_auto`] is needed.
pub fn has_rtl(text: &str) -> bool {
    text.chars().any(is_rtl_script)
}

/// Multi-line text with right-to-left script, broken into lines that `fits` accepts
/// and each put in display order, returning the logical byte range and display text for each line.
pub fn wrap_slices(text: &str, mut fits: impl FnMut(&str) -> bool) -> Vec<LineSlice> {
    let info = BidiInfo::new(text, None);
    let mut slices = Vec::new();
    for para in &info.paragraphs {
        let whole = text.get(para.range.clone()).unwrap_or_default();
        // unicode-bidi ends a paragraph at the `\r` of `\r\n`; the `\n` is no new paragraph.
        if whole == "\n" && text.get(..para.range.start).is_some_and(|before| before.ends_with('\r')) {
            continue;
        }
        let content = para.range.start..para.range.start.saturating_add(whole.trim_end_matches(is_paragraph_separator).len());
        let rtl = para.level.is_rtl();
        for line in break_lines(text, content, &mut fits) {
            let mut out = String::new();
            // unicode-bidi indexes the first level of a line, so an empty one would panic.
            if !line.is_empty() {
                push_line(&mut out, text, &info, para, line.clone());
            }
            slices.push(LineSlice {
                logical_range: line,
                display_text: out,
                rtl,
            });
        }
    }
    slices
}

/// Multi-line text with right-to-left script (a comment in Persian, Arabic, Hebrew), broken
/// into lines that `fits` accepts (it measures a line in logical order) and each put in display
/// order by the Unicode bidirectional algorithm (UAX #9).
///
/// Every paragraph takes its direction from its first strong character, as `dir="auto"` does,
/// so a Persian comment reads right to left with its Latin words and numbers left to right
/// inside it. Lines break between words, in reading order, and a word wider than a line
/// between characters. Use it for painting; copying and accessibility keep the logical text.
pub fn wrap(text: &str, fits: impl FnMut(&str) -> bool) -> Vec<Line> {
    wrap_slices(text, fits)
        .into_iter()
        .map(|s| Line {
            text: s.display_text,
            rtl: s.rtl,
        })
        .collect()
}

/// One line of `text` in display order, its direction from its first strong character (an
/// author's name). Text without right-to-left characters comes back borrowed and unchanged.
pub fn visual_auto(text: &str) -> Cow<'_, str> {
    if !has_rtl(text) {
        return Cow::Borrowed(text);
    }
    let lines: Vec<String> = wrap(text, |_| true).into_iter().map(|l| l.text).collect();
    Cow::Owned(lines.join("\n"))
}

/// Multi-line or single-line `text` in display order, broken into lines that `fits` accepts
/// and each put in display order.
pub fn visual_auto_wrap<'a>(text: &'a str, fits: impl FnMut(&str) -> bool) -> Cow<'a, str> {
    if !has_rtl(text) {
        return Cow::Borrowed(text);
    }
    let lines: Vec<String> = wrap(text, fits).into_iter().map(|l| l.text).collect();
    Cow::Owned(lines.join("\n"))
}

/// The byte ranges of the lines `range` of `text` breaks into: as many words as `fits` accepts
/// per line, a word too wide for any line split between characters. Always at least one line.
fn break_lines(text: &str, range: Range<usize>, fits: &mut impl FnMut(&str) -> bool) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut current: Option<Range<usize>> = None;
    for word in words(text, range.clone()) {
        if let Some(line) = current.as_mut() {
            if fits(text.get(line.start..word.end).unwrap_or_default()) {
                line.end = word.end;
                continue;
            }
            lines.push(line.clone());
        }
        // The word starts a line.
        let mut start = word.start;
        loop {
            let end = fitting_prefix(text, start..word.end, fits);
            if end >= word.end {
                current = Some(start..word.end);
                break;
            }
            lines.push(start..end);
            start = end;
        }
    }
    lines.push(current.unwrap_or(range.start..range.start));
    lines
}

/// The end of the longest start of `range` that `fits`, at least one character.
fn fitting_prefix(text: &str, range: Range<usize>, fits: &mut impl FnMut(&str) -> bool) -> usize {
    let piece = text.get(range.clone()).unwrap_or_default();
    let mut end = range.end;
    for (i, (at, c)) in piece.char_indices().enumerate() {
        let next = range.start.saturating_add(at).saturating_add(c.len_utf8());
        if i > 0 && !fits(text.get(range.start..next).unwrap_or_default()) {
            end = range.start.saturating_add(at);
            break;
        }
    }
    end
}

/// The byte ranges of the whitespace-separated words of `range` in `text`.
fn words(text: &str, range: Range<usize>) -> Vec<Range<usize>> {
    let mut words = Vec::new();
    let mut start = None;
    for (at, c) in text.get(range.clone()).unwrap_or_default().char_indices() {
        let at = range.start.saturating_add(at);
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                words.push(s..at);
                start = None;
            }
            (false, None) => start = Some(at),
            _ => {}
        }
    }
    if let Some(s) = start {
        words.push(s..range.end);
    }
    words
}

/// Characters that end a paragraph (bidi class B).
fn is_paragraph_separator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{1C}'..='\u{1E}' | '\u{85}' | '\u{2029}')
}

/// A label of a right-to-left interface language, in logical order, rearranged for egui: each
/// line reads right to left, with Latin words and numbers inside it left to right.
///
/// - `{name}` placeholders stay whole and unmirrored, so [`crate::i18n::fmt`] still fills them.
///   What it inserts is drawn left to right at that place.
/// - Spaces and separator dots at either end stay at that end: the code appends to a label on
///   its right (`"Issued by: "` + a name), as the interface is still laid out left to right.
/// - A short caption's closing colon stays on the right too, next to the widget it captions.
///
/// Text without right-to-left characters comes back unchanged. Like [`visual`], this is for
/// painting one line; a label egui wraps breaks at the wrong end.
pub fn display_rtl(text: &str) -> String {
    if !text.chars().any(is_rtl_script) {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        push_rtl_line(&mut out, line);
    }
    out
}

/// The most words a caption may have for its closing colon to stay on the right.
const CAPTION_WORDS: usize = 3;
/// First of the private-use characters that stand for placeholders while a line is reordered.
/// They are strong left-to-right, like the names they replace.
const PLACEHOLDER_BASE: u32 = 0xE000;
/// Placeholders protected per line; a line with more keeps the rest as plain text.
const MAX_PLACEHOLDERS: usize = 64;

fn push_rtl_line(out: &mut String, line: &str) {
    let edge = |c: char| c.is_whitespace() || c == '·';
    let core = line.trim_start_matches(edge);
    let lead = line.get(..line.len().saturating_sub(core.len())).unwrap_or_default();
    let trimmed = core.trim_end_matches(edge);
    let tail = core.get(trimmed.len()..).unwrap_or_default();
    let mut core = trimmed;
    let mut colon = "";
    if let Some(caption) = core.strip_suffix(':')
        && caption.split_whitespace().count() <= CAPTION_WORDS
    {
        core = caption;
        colon = ":";
    }
    // A line that already uses the stand-in characters is reordered with its placeholders as text.
    let reserved = core.chars().any(|c| (PLACEHOLDER_BASE..PLACEHOLDER_BASE.saturating_add(MAX_PLACEHOLDERS as u32)).contains(&u32::from(c)));
    let (masked, names) = if reserved { (core.to_owned(), Vec::new()) } else { mask_placeholders(core) };
    let mut visual = String::with_capacity(masked.len());
    push_visual(&mut visual, &masked, Level::rtl());
    out.push_str(lead);
    for c in visual.chars() {
        let index = u32::from(c).checked_sub(PLACEHOLDER_BASE).and_then(|i| usize::try_from(i).ok());
        match index.and_then(|i| names.get(i)) {
            Some(name) => out.push_str(name),
            None => out.push(c),
        }
    }
    out.push_str(colon);
    out.push_str(tail);
}

/// `text` with each `{name}` placeholder and each keyboard shortcut (`⇧⌘+`) replaced by one
/// stand-in character, and what they replaced, in order. A shortcut's symbols have no direction
/// of their own, so left alone they would be drawn in reverse.
fn mask_placeholders(text: &str) -> (String, Vec<&str>) {
    let mut masked = String::with_capacity(text.len());
    let mut names: Vec<&str> = Vec::new();
    let mut rest = text;
    while names.len() < MAX_PLACEHOLDERS
        && let Some((start, end)) = next_atom(rest)
    {
        let (Some(before), Some(atom), Some(after)) = (rest.get(..start), rest.get(start..end), rest.get(end..)) else { break };
        // `names.len()` is below MAX_PLACEHOLDERS, so the sum is a private-use scalar.
        let Some(stand_in) = u32::try_from(names.len()).ok().and_then(|i| char::from_u32(PLACEHOLDER_BASE.saturating_add(i))) else { break };
        masked.push_str(before);
        masked.push(stand_in);
        names.push(atom);
        rest = after;
    }
    masked.push_str(rest);
    (masked, names)
}

/// The byte range of the first placeholder or keyboard shortcut in `text`.
fn next_atom(text: &str) -> Option<(usize, usize)> {
    let placeholder = text.find('{').and_then(|open| {
        let len = text.get(open..)?.find('}')?;
        Some((open, open.saturating_add(len).saturating_add(1)))
    });
    let shortcut = text.find(is_modifier_key).map(|at| {
        // The shortcut is the stretch around the symbol up to a space, a bracket or script text.
        let outside = |c: char| c.is_whitespace() || is_rtl_script(c) || matches!(c, '(' | ')' | '[' | ']' | '{' | '}');
        let start = text
            .get(..at)
            .and_then(|before| before.rfind(outside).and_then(|i| before.get(i..)?.chars().next().map(|c| i.saturating_add(c.len_utf8()))))
            .unwrap_or(0);
        let end = text.get(at..).and_then(|after| after.find(outside)).map_or(text.len(), |i| at.saturating_add(i));
        (start, end)
    });
    match (placeholder, shortcut) {
        (Some(p), Some(s)) => Some(if s.0 < p.0 { s } else { p }),
        (p, s) => p.or(s),
    }
}

/// The modifier-key symbols of keyboard shortcuts: ⌘ ⇧ ⌥ ⌃.
fn is_modifier_key(c: char) -> bool {
    matches!(c, '\u{2318}' | '\u{21E7}' | '\u{2325}' | '\u{2303}')
}

/// Characters of the right-to-left scripts' blocks: letters, marks and the scripts' own
/// punctuation, all drawn by the script's face.
fn is_rtl_script(c: char) -> bool {
    matches!(u32::from(c), 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFC | 0x1_0800..=0x1_0FFF | 0x1_E800..=0x1_EFFF)
}

/// The zero-width (non-)joiners inside a word (Persian `می‌روم`): egui shapes them with the
/// letter before them.
fn is_joiner(c: char) -> bool {
    matches!(c, '\u{200C}' | '\u{200D}')
}

/// Direction marks, embeddings, overrides and isolates: they steer the reordering and are not drawn.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

fn mirrored(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        other => other,
    }
}
/// A layouter for [`egui::TextEdit`] that lays out right-to-left text in reading order
/// and maps character boundaries so the cursor and typing track properly from right to left.
pub fn text_edit_layouter() -> impl FnMut(&egui::Ui, &dyn egui::TextBuffer, f32) -> std::sync::Arc<egui::Galley> {
    |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| text_edit_layout(ui, text.as_str(), wrap_width)
}

/// Lays out text for a text edit field, applying bidirectional reordering when the text
/// or the interface contains right-to-left script, and adjusting glyph bounds so the
/// cursor and selection track characters in typing order.
pub fn text_edit_layout(ui: &egui::Ui, content: &str, wrap_width: f32) -> std::sync::Arc<egui::Galley> {
    let is_rtl_ui = crate::i18n::current().rtl();
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let text_color = ui
        .visuals()
        .override_text_color
        .unwrap_or_else(|| ui.visuals().widgets.inactive.text_color());

    if !has_rtl(content) && !is_rtl_ui {
        let mut job = egui::text::LayoutJob::simple(content.to_owned(), font_id, text_color, wrap_width);
        job.wrap.max_width = wrap_width;
        return ui.fonts_mut(|f| f.layout_job(job));
    }

    if content.is_empty() {
        let mut job = egui::text::LayoutJob::simple(String::new(), font_id, text_color, wrap_width);
        if is_rtl_ui {
            job.halign = egui::Align::RIGHT;
        }
        job.wrap.max_width = wrap_width;
        return ui.fonts_mut(|f| f.layout_job(job));
    }

    let is_rtl = has_rtl(content) || is_rtl_ui;

    // Pre-layout measurement and wrapping: break text into fitting visual line slices.
    let (slices_per_line, visual_text) = if wrap_width.is_finite() && wrap_width > 0.0 {
        ui.fonts_mut(|f| {
            let mut measure = |s: &str| f.layout_no_wrap(s.to_owned(), font_id.clone(), text_color).size().x <= wrap_width;
            let mut all_slices = Vec::new();
            let mut display_lines = Vec::new();
            for logical_line in content.split('\n') {
                let slices = wrap_slices(logical_line, &mut measure);
                if slices.is_empty() {
                    display_lines.push(String::new());
                } else {
                    for s in &slices {
                        display_lines.push(s.display_text.clone());
                    }
                }
                all_slices.push(slices);
            }
            (all_slices, display_lines.join("\n"))
        })
    } else {
        let mut all_slices = Vec::new();
        let mut display_lines = Vec::new();
        for logical_line in content.split('\n') {
            let slices = wrap_slices(logical_line, |_| true);
            if slices.is_empty() {
                display_lines.push(String::new());
            } else {
                for s in &slices {
                    display_lines.push(s.display_text.clone());
                }
            }
            all_slices.push(slices);
        }
        (all_slices, display_lines.join("\n"))
    };

    let mut job = egui::text::LayoutJob::simple(visual_text, font_id, text_color, f32::INFINITY);
    if is_rtl {
        job.halign = egui::Align::RIGHT;
    }
    let mut galley = ui.fonts_mut(|f| f.layout_job(job));

    let g = std::sync::Arc::make_mut(&mut galley);
    let fallback_sample = g.rows.iter().find_map(|r| r.row.glyphs.first().copied());
    let mut global_row_idx = 0;
    let logical_lines: Vec<&str> = content.split('\n').collect();
    let num_logical_lines = logical_lines.len();

    for (line_idx, (logical_line, slices)) in logical_lines.iter().zip(&slices_per_line).enumerate() {
        if slices.is_empty() {
            if global_row_idx < g.rows.len() {
                global_row_idx += 1;
            }
            continue;
        }
        let row_char_counts: Vec<usize> = slices
            .iter()
            .enumerate()
            .map(|(slice_idx, slice)| {
                let lead_ws_count = if slice_idx == 0 {
                    logical_line.get(..slice.logical_range.start).map_or(0, |s| s.chars().count())
                } else {
                    0
                };
                let slice_char_count = logical_line.get(slice.logical_range.clone()).map_or(0, |s| s.chars().count());
                let next_start = if slice_idx + 1 < slices.len() {
                    slices[slice_idx + 1].logical_range.start
                } else {
                    logical_line.len()
                };
                let wrap_ws_count = logical_line.get(slice.logical_range.end..next_start).map_or(0, |s| s.chars().count());
                lead_ws_count + slice_char_count + wrap_ws_count
            })
            .collect();

        if row_char_counts.iter().sum::<usize>() != logical_line.chars().count() {
            global_row_idx = (global_row_idx + slices.len()).min(g.rows.len());
            continue;
        }

        let is_last_logical_line = line_idx + 1 == num_logical_lines;

        for (slice_idx, slice) in slices.iter().enumerate() {
            if global_row_idx >= g.rows.len() {
                break;
            }
            let placed_row = &mut g.rows[global_row_idx];
            global_row_idx += 1;

            let is_last_slice_in_para = slice_idx + 1 == slices.len();
            placed_row.ends_with_newline = is_last_slice_in_para && !is_last_logical_line;

            let row = std::sync::Arc::make_mut(&mut placed_row.row);
            let orig_glyphs = row.glyphs.clone();
            let vis_glyphs: Vec<_> = orig_glyphs.iter().filter(|g| g.advance_width > 0.0).copied().collect();

            let lead_ws_count = if slice_idx == 0 {
                logical_line.get(..slice.logical_range.start).map_or(0, |s| s.chars().count())
            } else {
                0
            };

            // Safe slice of the logical line on character boundaries.
            let logical_slice = logical_line.get(slice.logical_range.clone()).unwrap_or_default();
            let slice_char_count = logical_slice.chars().count();
            let total_row_chars = row_char_counts[slice_idx];

            let base_level = if slice.rtl { Level::rtl() } else { Level::ltr() };
            let bidi = unicode_bidi::BidiInfo::new(logical_slice, Some(base_level));
            let levels = if let Some(para) = bidi.paragraphs.first() {
                bidi.reordered_levels_per_char(para, para.range.clone())
            } else {
                vec![base_level; slice_char_count]
            };
            let visual_map = unicode_bidi::BidiInfo::reorder_visual(&levels);
            let mut log_to_vis = vec![0; slice_char_count];
            for (v, &log) in visual_map.iter().enumerate() {
                if log < slice_char_count {
                    log_to_vis[log] = v;
                }
            }

            let row_width = row.size.x;
            let mut cursor_offsets = vec![0.0f32; total_row_chars.saturating_add(1)];
            for col in 0..slice_char_count {
                let is_char_rtl = levels.get(col).is_some_and(Level::is_rtl);
                let v = log_to_vis.get(col).copied().unwrap_or(0);
                let (x_left, x_right) = if v < vis_glyphs.len() {
                    let g = vis_glyphs[v];
                    (g.pos.x.clamp(0.0, row_width), (g.pos.x + g.advance_width).clamp(0.0, row_width))
                } else {
                    (0.0, row_width)
                };
                let row_col = lead_ws_count + col;
                if is_char_rtl {
                    cursor_offsets[row_col] = x_right;
                    cursor_offsets[row_col + 1] = x_left;
                } else {
                    cursor_offsets[row_col] = x_left;
                    cursor_offsets[row_col + 1] = x_right;
                }
            }

            // Anchor leading whitespace character offsets at the row start.
            let row_start_offset = if slice_char_count > 0 {
                cursor_offsets[lead_ws_count]
            } else if slice.rtl {
                row_width
            } else {
                0.0
            };
            for w in 0..lead_ws_count {
                cursor_offsets[w] = row_start_offset;
            }
            if slice_char_count == 0 {
                cursor_offsets[lead_ws_count] = row_start_offset;
            }

            // Pin any trailing or collapsed whitespace at the wrap boundary to the row end
            // so whitespace never distorts the cursor position at row ends.
            let text_end_col = lead_ws_count + slice_char_count;
            let row_end_offset = cursor_offsets[text_end_col];
            for w in text_end_col..total_row_chars {
                cursor_offsets[w + 1] = row_end_offset;
            }

            if total_row_chars == 0 {
                row.glyphs.clear();
                row.size.x = 0.0;
                continue;
            }

            let Some(sample) = orig_glyphs.first().copied().or(fallback_sample) else { continue };
            row.glyphs.clear();
            for &x in &cursor_offsets[..total_row_chars] {
                let mut glyph = sample;
                glyph.pos.x = x;
                row.glyphs.push(glyph);
            }
            row.size.x = cursor_offsets[total_row_chars];
        }
    }

    galley
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_without_rtl_is_borrowed_unchanged() {
        for s in ["", "report.pdf", "日本語の文字.pdf", "a (b) [c]", "caf\u{00E9} \u{200E}x"] {
            assert!(matches!(visual(s), Cow::Borrowed(v) if v == s), "{s:?}");
        }
    }

    #[test]
    fn arabic_words_swap_and_the_extension_stays_last() {
        // The reported name: "one two.pdf" in Arabic.
        assert_eq!(visual("واحد اثنين.pdf"), "اثنين واحد.pdf");
        assert_eq!(visual("ملف.pdf"), "ملف.pdf");
        assert_eq!(visual("تقرير نهائي جدا.pdf"), "جدا نهائي تقرير.pdf");
    }

    #[test]
    fn latin_and_numbers_keep_their_own_order() {
        assert_eq!(visual("report واحد اثنين final.pdf"), "report اثنين واحد final.pdf");
        // A number inside right-to-left text reads left to right, to the left of what precedes it.
        assert_eq!(visual("ملف 2024.pdf"), "2024 ملف.pdf");
        assert_eq!(visual(r"C:\Users\me\واحد اثنين\a.pdf"), r"C:\Users\me\اثنين واحد\a.pdf");
    }

    #[test]
    fn hebrew_and_brackets() {
        assert_eq!(visual("שלום עולם.pdf"), "עולם שלום.pdf");
        // Brackets between right-to-left words flip with the direction.
        assert_eq!(visual("واحد (اثنين) ثلاثة.pdf"), "ثلاثة (اثنين) واحد.pdf");
    }

    #[test]
    fn direction_controls_are_not_drawn() {
        assert_eq!(visual("\u{202B}واحد اثنين\u{202C}.pdf"), "اثنين واحد.pdf");
        assert_eq!(visual("\u{200F}واحد\u{061C}"), "واحد");
    }

    #[test]
    fn rtl_labels_read_right_to_left() {
        // No right-to-left characters: unchanged.
        for s in ["", "Save as…", "{n} pages", "日本語"] {
            assert_eq!(display_rtl(s), s);
        }
        assert_eq!(display_rtl("حفظ"), "حفظ");
        // "Save as…": the second word and the ellipsis end up on the left.
        assert_eq!(display_rtl("حفظ باسم…"), "…باسم حفظ");
        // Latin words and numbers keep their own order inside the line.
        assert_eq!(display_rtl("إنشاء PDF من ملف"), "ملف من PDF إنشاء");
        assert_eq!(display_rtl("تصدير إلى Microsoft Word الآن"), "الآن Microsoft Word إلى تصدير");
        assert_eq!(display_rtl("صالح لمدة 5 سنوات"), "سنوات 5 لمدة صالح");
        // Keyboard shortcuts are not reversed.
        assert_eq!(display_rtl("طباعة (⌘P)"), "(⌘P) طباعة");
        assert_eq!(display_rtl("تدوير العرض (⇧⌘+)"), "(⇧⌘+) العرض تدوير");
        assert_eq!(display_rtl("التمرير مع ⌘ أيضًا"), "أيضًا ⌘ مع التمرير");
        assert_eq!(display_rtl("نص (عادي)"), "(عادي) نص");
    }

    #[test]
    fn rtl_labels_keep_placeholders_whole() {
        // Two placeholders around a colon stay together, left to right: a file name and its error.
        assert_eq!(display_rtl("تعذّر فتح {name}: {e}"), "{name}: {e} فتح تعذّر");
        assert_eq!(display_rtl("فشل {label}، السبب {e}"), "{e} السبب ،{label} فشل");
        assert_eq!(display_rtl("{n} صفحات"), "صفحات {n}");
        assert_eq!(display_rtl("الصفحة {p} من {n}"), "{n} من {p} الصفحة");
        // Adjacent placeholders keep their order, as numbers around a slash would.
        assert_eq!(display_rtl("جارٍ البحث… {s}/{p}"), "{s}/{p} …البحث جارٍ");
        // Braces that are not a placeholder are ordinary punctuation.
        assert_eq!(display_rtl("حرف } واحد"), "واحد { حرف");
        let many = format!("ا {}", "{a} ".repeat(200));
        assert_eq!(display_rtl(&many).matches("{a}").count(), 200);
    }

    #[test]
    fn rtl_labels_keep_their_edges() {
        // Edge spaces and separator dots stay where the code concatenates.
        assert_eq!(display_rtl(" (صفحة {p})"), " ({p} صفحة)");
        assert_eq!(display_rtl(" · محمي بكلمة مرور"), " · مرور بكلمة محمي");
        assert_eq!(display_rtl("سلسلة المفاتيح  ·  "), "المفاتيح سلسلة  ·  ");
        // A caption's colon stays next to its widget; a sentence's colon ends the sentence.
        assert_eq!(display_rtl("الاسم:"), "الاسم:");
        assert_eq!(display_rtl("صادرة عن: "), "عن صادرة: ");
        assert_eq!(display_rtl("اختر المعرّف الرقمي الذي تريد استخدامه:"), ":استخدامه تريد الذي الرقمي المعرّف اختر");
        // Each line is its own paragraph.
        assert_eq!(display_rtl("تعذّر عرض الصفحة.\n{e}"), ".الصفحة عرض تعذّر\n{e}");
    }

    #[test]
    fn rtl_labels_never_panic_on_odd_input() {
        let long = "ا {n} ".repeat(20_000);
        for s in [
            "ا{",
            "ا}",
            "ا{}",
            "ا{{{",
            "}ا{",
            "ا\u{E000}{n}",
            "⌘",
            "ا ⌘",
            "⌘ا⌘{",
            "ا (⇧⌘",
            "\u{064B}:",
            ":",
            "ا:",
            " : ا : ",
            "ا\n\n\nب",
            "\u{202E}ا{n}",
            long.as_str(),
        ] {
            let v = display_rtl(s);
            assert!(v.chars().count() <= s.chars().count(), "{:?}", s.chars().take(12).collect::<String>());
        }
    }

    /// `wrap` with a width of `n` characters.
    fn wrap_at(text: &str, n: usize) -> Vec<Line> {
        wrap(text, |s| s.chars().count() <= n)
    }

    fn rtl(text: &str) -> Line {
        Line { text: text.to_owned(), rtl: true }
    }

    fn ltr(text: &str) -> Line {
        Line { text: text.to_owned(), rtl: false }
    }

    #[test]
    fn persian_paragraph_reads_right_to_left() {
        // "I go to school": the words come out in reverse for egui's left-to-right placement.
        assert_eq!(wrap_at("من به مدرسه می‌روم", 80), [rtl("می‌روم مدرسه به من")]);
        // The zero-width non-joiner keeps a word whole, so the shaper gets it as one run.
        assert_eq!(wrap_at("کتاب‌ها", 80), [rtl("کتاب‌ها")]);
    }

    #[test]
    fn arabic_lines_break_in_reading_order() {
        // The first words go on the first line, each line reordered on its own.
        assert_eq!(wrap_at("واحد اثنين ثلاثة أربعة", 11), [rtl("اثنين واحد"), rtl("أربعة ثلاثة")]);
        assert_eq!(wrap_at("واحد اثنين ثلاثة أربعة", 80), [rtl("أربعة ثلاثة اثنين واحد")]);
    }

    #[test]
    fn base_direction_comes_from_the_first_strong_character() {
        // Latin first: a left-to-right paragraph with an Arabic phrase inside it.
        assert_eq!(wrap_at("See واحد اثنين here", 80), [ltr("See اثنين واحد here")]);
        // Persian first: right to left, the Latin words and the number left to right inside it.
        assert_eq!(wrap_at("فایل PDF نسخه 2 است", 80), [rtl("است 2 نسخه PDF فایل")]);
        assert_eq!(wrap_at("سلام Hello World", 80), [rtl("Hello World سلام")]);
        // Leading neutrals and numbers don't decide it.
        assert_eq!(wrap_at("12. واحد اثنين", 80), [rtl("اثنين واحد .12")]);
        // Each paragraph finds its own direction.
        assert_eq!(wrap_at("واحد اثنين\nOK then\n\nשלום עולם", 80), [rtl("اثنين واحد"), ltr("OK then"), ltr(""), rtl("עולם שלום")]);
    }

    #[test]
    fn rtl_punctuation_and_brackets() {
        // The sentence's full stop ends up on the left, brackets flip.
        assert_eq!(wrap_at("سلام دنیا.", 80), [rtl(".دنیا سلام")]);
        assert_eq!(wrap_at("واحد (اثنين) ثلاثة", 80), [rtl("ثلاثة (اثنين) واحد")]);
        assert_eq!(wrap_at("آیا آماده است؟", 80), [rtl("است؟ آماده آیا")]);
    }

    #[test]
    fn persian_digits_go_to_the_shaper_reversed() {
        // Persian and Arabic-Indic digits are Arabic script, which egui's shaper lays out right to
        // left; they read left to right, so they are handed over reversed.
        assert_eq!(wrap_at("سال ۱۴۰۲ شمسی", 80), [rtl("شمسی ۲۰۴۱ سال")]);
        assert_eq!(wrap_at("صفحه ١٢", 80), [rtl("٢١ صفحه")]);
        // Glued to a word, number and word swap places within one shaped run.
        assert_eq!(wrap_at("ب۱۲", 80), [rtl("ب۲۱")]);
    }

    #[test]
    fn wrapping_handles_odd_widths_and_input() {
        assert_eq!(wrap_at("", 10), Vec::<Line>::new());
        // A word wider than the line is broken between characters.
        assert_eq!(wrap_at("abcdefghij واحد", 4), [ltr("abcd"), ltr("efgh"), ltr("ij"), ltr("واحد")]);
        // Extra spaces between words don't make empty lines; \r\n is one break.
        assert_eq!(wrap_at("واحد    اثنين\r\nب", 5), [rtl("واحد"), rtl("اثنين"), rtl("ب")]);
        // Nothing fits: one character per line, never an endless loop.
        assert_eq!(wrap("واحد", |_| false).len(), 4);
        let long = "ا ب ۱ (ج) ".repeat(5_000);
        for s in ["\u{202E}ا\u{202C}b", "\u{064B}", "\n\n", "\u{2029}ا", long.as_str()] {
            let lines = wrap_at(s, 7);
            let shown: usize = lines.iter().map(|l| l.text.chars().count()).sum();
            assert!(shown <= s.chars().count(), "{:?}", s.chars().take(12).collect::<String>());
        }
    }

    /// The characters egui puts on screen for each line of `text`, left to right, zero-width
    /// ones left out, with the interface fonts.
    fn on_screen(text: &str) -> Vec<String> {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let mut shown = Vec::new();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for line in wrap(text, |_| true) {
                let galley = ui.painter().layout_no_wrap(line.text, egui::FontId::proportional(13.0), egui::Color32::WHITE);
                let glyphs = galley.rows.iter().flat_map(|row| row.glyphs.iter());
                shown.push(glyphs.filter(|g| g.advance_width > 0.0).map(|g| g.chr).collect());
            }
        });
        output.drop_without_applying_deltas();
        shown
    }

    fn rev(s: &str) -> String {
        s.chars().rev().collect()
    }

    #[test]
    fn egui_draws_lines_in_reading_order() {
        // Only with an Arabic-script face (the craft-fonts build input, as in CI); without one
        // egui has no shaper run to check.
        if pdfcraft_fonts::ui_arabic_fonts().is_empty() {
            return;
        }
        assert_eq!(on_screen("من به مدرسه می\u{200C}روم"), [rev("من به مدرسه میروم")]);
        assert_eq!(on_screen("فایل PDF نسخه 2 است"), [format!("{} 2 {} PDF {}", rev("است"), rev("نسخه"), rev("فایل"))]);
        assert_eq!(on_screen("سال ۱۴۰۲ شمسی"), [format!("{} ۱۴۰۲ {}", rev("شمسی"), rev("سال"))]);
        assert_eq!(on_screen("ب۱۲"), ["۱۲ب"]);
        assert_eq!(on_screen("See واحد اثنين here"), [format!("See {} {} here", rev("اثنين"), rev("واحد"))]);
        assert_eq!(on_screen("واحد (اثنين)."), [format!(".({}) {}", rev("اثنين"), rev("واحد"))]);
    }

    #[test]
    fn single_line_with_its_own_direction() {
        assert!(matches!(visual_auto("Ada Lovelace"), Cow::Borrowed("Ada Lovelace")));
        assert_eq!(visual_auto("علی رضایی"), "رضایی علی");
        assert_eq!(visual_auto("علی 2"), "2 علی");
        assert_eq!(visual_auto("Team علی رضایی"), "Team رضایی علی");
    }

    #[test]
    fn odd_input_never_panics() {
        let long = "ا ب ".repeat(20_000);
        let odd = [
            "\u{064B}",
            "\u{064B}\u{064C} \u{0301}",
            "\u{202E}\u{202E}\u{202E}ا",
            "\u{2067}ا\u{2066}b",
            "ا\nب\r\nج\u{2029}د",
            "\u{FEFF}ا\u{0000}ب",
            "ا\u{10FFFF}\u{E000}ب",
            "((((ا]]]]",
            long.as_str(),
        ];
        for s in odd {
            let v = visual(s);
            assert!(v.chars().count() <= s.chars().count(), "{:?}", s.chars().take(12).collect::<String>());
        }
    }
    #[test]
    fn test_bidi_text_edit_layout() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let text = "واحد اثنين";
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let galley = text_edit_layout(ui, text, 200.0);
            let logical_char_count = text.chars().count();
            assert_eq!(galley.rows.len(), 1);
            let c0 = galley.pos_from_cursor(egui::text::CCursor::new(0));
            let c_end = galley.pos_from_cursor(egui::text::CCursor::new(logical_char_count));
            // In RTL, typing starts at the right (c0) and progresses to the left (c_end).
            assert!(c0.min.x > c_end.min.x, "c0 ({}) must be to the right of c_end ({})", c0.min.x, c_end.min.x);
            // Every character boundary should have a valid cursor rect.
            for ci in 0..=logical_char_count {
                let c = galley.pos_from_cursor(egui::text::CCursor::new(ci));
                assert!(c.min.x <= c0.min.x + 0.1 && c.min.x >= c_end.min.x - 0.1, "ci={ci} cursor at {}", c.min.x);
            }
        });
        output.drop_without_applying_deltas();
    }

    #[test]
    fn test_bidi_text_edit_empty_and_mixed() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            // Empty text layout
            let empty = text_edit_layout(ui, "", 200.0);
            assert!(empty.rows.is_empty() || empty.rect.width() == 0.0);

            // Mixed text layout
            let mixed = "مرحبا (PDF) 123";
            let galley = text_edit_layout(ui, mixed, 300.0);
            let count = mixed.chars().count();
            let c0 = galley.pos_from_cursor(egui::text::CCursor::new(0));
            let c_end = galley.pos_from_cursor(egui::text::CCursor::new(count));
            assert!(c0.min.x > c_end.min.x);
        });
        output.drop_without_applying_deltas();
    }

    #[test]
    fn blank_lines_preserve_row_count_and_wrap_slices_empty() {
        assert!(wrap_slices("", |_| true).is_empty());

        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let galley = text_edit_layout(ui, "أ\n\nب", 200.0);
            assert_eq!(galley.rows.len(), 3);
        });
        output.drop_without_applying_deltas();
    }
}
