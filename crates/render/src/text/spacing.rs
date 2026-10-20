//! PDF show-operation provenance separates ordinary TJ word spacing from unrelated text
//! placed on the same baseline. Estimates are in em units and never change glyph geometry.

use super::{Drawn, TextGlyph, WORD_GAP};

/// One spacing hint per framed glyph; zero denotes an unavailable estimate. Runs are
/// measured independently so separate columns drawn on one baseline do not set word space.
pub(super) fn estimates(source: &[Drawn], ids: &[usize], framed: &[TextGlyph]) -> Vec<f32> {
    let mut hints = vec![0.0; framed.len()];
    let run = |at: usize| ids.get(at).and_then(|&id| source.get(id)).map(|g| g.3).unwrap_or_default();
    let mut start = 0;
    while start < framed.len() {
        let key = run(start).show;
        let mut end = start.saturating_add(1);
        while end < framed.len() && run(end).show == key {
            end = end.saturating_add(1);
        }
        if key != 0
            && let Some(glyphs) = framed.get(start..end)
        {
            // Explicit Unicode spaces are stronger evidence, measured with their real Tw
            // advances by regions. A TJ can also position two complete columns; treating
            // that operation's sole large advance as word spacing would erase the gutter.
            if glyphs.iter().any(|g| g.text.chars().any(char::is_whitespace)) {
                for (offset, g) in glyphs.iter().enumerate() {
                    let Some(at) = start.checked_add(offset) else { continue };
                    let origin = run(at);
                    let h = g.rect[3] - g.rect[1];
                    if origin.string == 0 || !h.is_finite() || h <= 0.0 || g.text.is_empty() || !g.text.chars().all(char::is_whitespace) {
                        continue;
                    }
                    let nominal = g.rect[2] - g.rect[0];
                    let width = at
                        .checked_add(1)
                        .filter(|&next| next < end && run(next).string == origin.string)
                        .and_then(|next| framed.get(next))
                        .filter(|next| (next.rect[1] - g.rect[1]).abs() < h * 0.5 && next.rect[0] >= g.rect[0])
                        .map_or(nominal, |next| (next.rect[0] - g.rect[0]).max(nominal));
                    if width.is_finite()
                        && width > 0.0
                        && let Some(slot) = hints.get_mut(at)
                    {
                        *slot = width / h;
                    }
                }
                start = end;
                continue;
            }
            let mut gaps: Vec<_> = glyphs
                .windows(2)
                .filter_map(|pair| {
                    let (a, b) = (pair.first()?, pair.get(1)?);
                    let h = (a.rect[3] - a.rect[1]).min(b.rect[3] - b.rect[1]);
                    if !h.is_finite() || h <= 0.0 {
                        return None;
                    }
                    let offset = (a.rect[1] * 0.5 + a.rect[3] * 0.5 - b.rect[1] * 0.5 - b.rect[3] * 0.5).abs();
                    let gap = (b.rect[0] - a.rect[2]) / h;
                    (offset < h * 0.5 && gap.is_finite() && gap >= 0.0).then_some(gap)
                })
                .collect();
            if !gaps.is_empty() {
                let k = gaps.len() / 3;
                let tracking = *gaps.select_nth_unstable_by(k, f32::total_cmp).1;
                gaps.retain(|gap| *gap > tracking + WORD_GAP);
                if !gaps.is_empty() {
                    let k = gaps.len() / 2;
                    let word = *gaps.select_nth_unstable_by(k, f32::total_cmp).1;
                    if let Some(slots) = hints.get_mut(start..end) {
                        slots.fill(word);
                    }
                }
            }
        }
        start = end;
    }
    hints
}
