//! Vector PDF export of schematic pages.
//!
//! Output is plain PDF 1.7 written by hand (no dependencies): lines and curves stay vectors,
//! text uses the standard Helvetica font so tags are searchable, every page gets a bookmark,
//! and cross-references between a coil and its contacts are clickable links. The same pages
//! also export as SVG and DXF (`vector`).

mod layout;
mod picture;
mod vector;
mod writer;

use std::collections::BTreeMap;

use wc_core::{Project, Symbol};

pub use layout::{
    CrossRefs, FRAME_COLUMNS, FrameMark, Ink, XREF_HEIGHT, XrefMark, column_of, frame_marks, junctions, pin_position,
    xref_marks,
};
pub use picture::read_underlays;
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
    /// Underlay picture files by path (see [`read_underlays`]), drawn under their pages on
    /// a layer of their own.
    pub underlays: BTreeMap<String, Vec<u8>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            pages: Vec::new(),
            date: String::new(),
            wire_labels: true,
            reports: Vec::new(),
            underlays: BTreeMap::new(),
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
    // Each report page: (table, first row, part, parts). Front tables (the table of contents)
    // come before the schematic pages, the rest after them.
    let split = |front: bool| -> Vec<(usize, usize, usize, usize)> {
        options
            .reports
            .iter()
            .enumerate()
            .filter(|(_, t)| t.front == front)
            .flat_map(|(ti, t)| {
                let n = t.rows.len().div_ceil(per_page).max(1);
                (0..n).map(move |k| (ti, k * per_page, k + 1, n))
            })
            .collect()
    };
    let (front_pages, back_pages) = (split(true), split(false));
    let total = front_pages.len() + pages.len() + back_pages.len();
    report.pages = total;
    let mut doc = writer::Doc::new(&project.meta.name);
    let mut sheet = 0;
    let mut report_no = 0;
    let mut add_reports = |doc: &mut writer::Doc, list: Vec<(usize, usize, usize, usize)>, sheet: &mut usize| {
        for (ti, first, part, parts) in list {
            let Some(table) = options.reports.get(ti) else { continue };
            report_no += 1;
            let mut page = wc_core::Page::new(
                format!("report-{ti}-{part}"),
                format!("R{report_no}"),
                table.title.clone(),
            );
            page.size = size;
            let drawn = layout::draw_table_page(
                project,
                &page,
                *sheet,
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
            *sheet += 1;
        }
    };
    add_reports(&mut doc, front_pages, &mut sheet);
    let offset = sheet;
    for page in &pages {
        let mut drawn = layout::draw_page(
            project,
            page,
            sheet,
            total,
            symbols,
            &xref,
            options,
            &mut report.warnings,
        );
        if let Some(u) = &page.underlay {
            match options.underlays.get(&u.file) {
                Some(bytes) => match picture::page_image(u, bytes) {
                    Ok(img) => drawn.underlay = Some(img),
                    Err(why) => report
                        .warnings
                        .push(format!("page {}: underlay {}: {why}", page.number, u.file)),
                },
                None => report
                    .warnings
                    .push(format!("page {}: underlay picture {} is missing", page.number, u.file)),
            }
        }
        doc.add_page(page.size.mm(), &format!("{} {}", page.number, page.title), drawn);
        sheet += 1;
    }
    add_reports(&mut doc, back_pages, &mut sheet);
    // Links resolve against the exported page list, so links to pages left out are dropped.
    let index: BTreeMap<String, usize> = pages
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.clone(), offset + i))
        .collect();
    (doc.finish(&index), report)
}

#[cfg(test)]
mod tests;
