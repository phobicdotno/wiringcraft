//! Vector PDF export of schematic pages.
//!
//! Output is plain PDF 1.7 written by hand (no dependencies): lines and curves stay vectors,
//! text uses the standard Helvetica font so tags are searchable, every page gets a bookmark,
//! and cross-references between a coil and its contacts are clickable links. The same pages
//! also export as SVG and DXF (`vector`).

mod layout;
mod vector;
mod writer;

use std::collections::BTreeMap;

use wc_core::{Project, Symbol};

pub use layout::{
    CrossRefs, FRAME_COLUMNS, FrameMark, Ink, XREF_HEIGHT, XrefMark, column_of, frame_marks, junctions, pin_position,
    xref_marks,
};
pub use vector::{PageDrawing, dxf, page_drawings, svg};
pub use writer::{Layer, Prim, Stroke};

#[derive(Debug, Clone)]
pub struct Options {
    /// Export only these page ids (in project order); empty means all pages.
    pub pages: Vec<String>,
    /// Text for the date field of the title block.
    pub date: String,
    /// Print wire numbers and potential names next to wires.
    pub wire_labels: bool,
    /// Report tables appended after the schematic pages, each starting on a new page.
    pub reports: Vec<wc_report::Table>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            pages: Vec::new(),
            date: String::new(),
            wire_labels: true,
            reports: Vec::new(),
        }
    }
}

/// Problems that did not stop the export but are worth showing (missing symbols, open pins).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub pages: usize,
    pub warnings: Vec<String>,
}

/// Render the project to PDF bytes. `symbols` are the project's vendored symbols by id.
pub fn export(project: &Project, symbols: &BTreeMap<String, Symbol>, options: &Options) -> (Vec<u8>, Report) {
    let pages: Vec<&wc_core::Page> = if options.pages.is_empty() {
        project.pages.iter().collect()
    } else {
        project.pages.iter().filter(|p| options.pages.contains(&p.id)).collect()
    };
    let mut report = Report {
        pages: 0,
        warnings: Vec::new(),
    };
    let xref = layout::CrossRefs::build(project, symbols);
    // Report pages use the size of the first schematic page and are counted in the sheet total.
    let size = pages.first().map(|p| p.size).unwrap_or_default();
    let per_page = layout::rows_per_page(size);
    let report_pages: Vec<(usize, usize, usize, usize)> = options
        .reports
        .iter()
        .enumerate()
        .flat_map(|(ti, t)| {
            let n = t.rows.len().div_ceil(per_page).max(1);
            (0..n).map(move |k| (ti, k * per_page, k + 1, n))
        })
        .collect();
    let total = pages.len() + report_pages.len();
    report.pages = total;
    let mut doc = writer::Doc::new(&project.meta.name);
    for (i, page) in pages.iter().enumerate() {
        let drawn = layout::draw_page(project, page, i, total, symbols, &xref, options, &mut report.warnings);
        doc.add_page(page.size.mm(), &format!("{} {}", page.number, page.title), drawn);
    }
    for (k, (ti, first, part, parts)) in report_pages.into_iter().enumerate() {
        let Some(table) = options.reports.get(ti) else { continue };
        let mut page = wc_core::Page::new(
            format!("report-{ti}-{part}"),
            format!("R{}", k + 1),
            table.title.clone(),
        );
        page.size = size;
        let drawn = layout::draw_table_page(
            project,
            &page,
            pages.len() + k,
            total,
            table,
            first,
            (part, parts),
            &options.date,
        );
        let title = if parts > 1 {
            format!("{} {} ({part}/{parts})", page.number, table.title)
        } else {
            format!("{} {}", page.number, table.title)
        };
        doc.add_page(size.mm(), &title, drawn);
    }
    // Links resolve against the exported page list, so links to pages left out are dropped.
    let index: BTreeMap<String, usize> = pages.iter().enumerate().map(|(i, p)| (p.id.clone(), i)).collect();
    (doc.finish(&index), report)
}

#[cfg(test)]
mod tests;
