use crate::layout::{DocumentLayout, ScriptType};
use anyhow::{Context, Result};
use rust_xlsxwriter::{Format, FormatScript, Workbook};
use std::path::Path;

/// Exports a `DocumentLayout` to an Excel spreadsheet (.xlsx).
pub fn export_xlsx(doc: &DocumentLayout, output_path: &Path) -> Result<()> {
    let mut workbook = Workbook::new();

    for page in &doc.pages {
        let worksheet = workbook.add_worksheet();
        let sheet_name = format!("Page {}", page.page_index + 1);
        worksheet
            .set_name(&sheet_name)
            .with_context(|| format!("Failed to set worksheet name '{}'", sheet_name))?;

        // Sort lines top-to-bottom (descending baseline_y)
        let mut sorted_lines = page.lines.clone();
        sorted_lines.sort_by(|a, b| {
            b.baseline_y
                .partial_cmp(&a.baseline_y)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for (line_idx, line) in sorted_lines.iter().enumerate() {
            let row = line_idx as u32;
            let mut last_col: Option<u16> = None;

            for run in &line.runs {
                // Determine column index from horizontal offset from left margin
                let mut col = (run.bounds.0 / 60.0).floor().max(0.0) as u16;

                // Ensure successive runs on the same row don't overwrite each other
                if let Some(prev) = last_col {
                    if col <= prev {
                        col = prev + 1;
                    }
                }
                last_col = Some(col);

                let mut format = Format::new()
                    .set_font_name(&run.font_name)
                    .set_font_size(run.font_size as f64);

                if run.is_bold {
                    format = format.set_bold();
                }
                if run.is_italic {
                    format = format.set_italic();
                }

                if run.script == ScriptType::Superscript {
                    format = format.set_font_script(FormatScript::Superscript);
                } else if run.script == ScriptType::Subscript {
                    format = format.set_font_script(FormatScript::Subscript);
                }

                worksheet
                    .write_string_with_format(row, col, &run.text, &format)
                    .with_context(|| {
                        format!(
                            "Failed to write string '{}' at row {}, col {}",
                            run.text, row, col
                        )
                    })?;
            }
        }
    }

    workbook
        .save(output_path)
        .with_context(|| format!("Failed to save XLSX workbook to {}", output_path.display()))?;

    Ok(())
}
