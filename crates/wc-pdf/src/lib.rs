//! Vector PDF export of schematic pages.
//!
//! Output is plain PDF 1.7 written by hand (no dependencies): lines and curves stay vectors,
//! text uses the standard Helvetica font so tags are searchable, every page gets a bookmark,
//! and cross-references between a coil and its contacts are clickable links.

mod layout;
mod writer;

use std::collections::BTreeMap;

use wc_core::{Project, Symbol};

pub use layout::{FRAME_COLUMNS, column_of, pin_position};

#[derive(Debug, Clone)]
pub struct Options {
    /// Export only these page ids (in project order); empty means all pages.
    pub pages: Vec<String>,
    /// Text for the date field of the title block.
    pub date: String,
    /// Print wire numbers and potential names next to wires.
    pub wire_labels: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            pages: Vec::new(),
            date: String::new(),
            wire_labels: true,
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
        pages: pages.len(),
        warnings: Vec::new(),
    };
    let xref = layout::CrossRefs::build(project, symbols);
    let mut doc = writer::Doc::new(&project.meta.name);
    for (i, page) in pages.iter().enumerate() {
        let drawn = layout::draw_page(
            project,
            page,
            i,
            pages.len(),
            symbols,
            &xref,
            options,
            &mut report.warnings,
        );
        doc.add_page(page.size.mm(), &format!("{} {}", page.number, page.title), drawn);
    }
    // Links resolve against the exported page list, so links to pages left out are dropped.
    let index: BTreeMap<String, usize> = pages.iter().enumerate().map(|(i, p)| (p.id.clone(), i)).collect();
    (doc.finish(&index), report)
}

#[cfg(test)]
mod tests;
