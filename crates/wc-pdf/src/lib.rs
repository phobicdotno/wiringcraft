//! Vector PDF export of schematic pages.
//!
//! Output is plain PDF 1.7 written by hand (no dependencies): lines and curves stay vectors,
//! text uses the standard Helvetica font so tags are searchable, every page gets a bookmark,
//! and cross-references between a coil and its contacts are clickable links. The same pages
//! also export as SVG and DXF (`vector`).

mod layout;
mod picture;
mod topology;
mod vector;
mod writer;

use std::collections::BTreeMap;

use wc_core::{Project, Symbol};

pub use layout::{
    CrossRefs, FRAME_COLUMNS, FrameMark, Ink, XREF_HEIGHT, XrefMark, column_of, cover_logo_box, cover_logo_default,
    fit_picture, frame_marks, junctions, logo_boxes, pin_position, title_logo_box, title_logo_boxes, xref_marks,
};
pub use picture::{Pictures, read_picture, read_pictures};
pub use vector::{PageDrawing, Picture, dxf, page_drawings, page_lines, svg};
pub use writer::{Layer, Prim, Stroke, text_width_mm};

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
    /// Picture files by path (see [`read_pictures`]): underlays, drawn under their pages on
    /// a layer of their own, and the project logo.
    pub underlays: BTreeMap<String, Vec<u8>>,
    /// Why pictures that exist could not be read, by path (see [`Pictures::unreadable`]).
    pub unreadable: BTreeMap<String, String>,
    /// Panel layouts printed after the schematic pages, one sheet each.
    pub panels: Vec<wc_core::panel::Panel>,
    /// Fieldbus networks drawn as topology pages at the end, one sheet each.
    pub networks: Vec<wc_core::network::Network>,
    /// Footprint drawings by part id ([`footprints`]): a device whose part has one is drawn
    /// with it on panel layouts instead of a plain box.
    pub footprints: BTreeMap<String, wc_core::Footprint>,
    /// PLC cards drawn after the panel layouts, one card per sheet (more for a long card),
    /// with a terminal per channel and the point drawn there (#437).
    pub plc_cards: Vec<wc_report::PlcCardSheet>,
}

/// The footprint drawings of `parts` by part id, for [`Options::footprints`].
pub fn footprints<'a>(parts: impl IntoIterator<Item = &'a wc_core::Part>) -> BTreeMap<String, wc_core::Footprint> {
    parts
        .into_iter()
        .filter_map(|p| p.footprint.clone().map(|f| (p.id.clone(), f)))
        .collect()
}

impl Default for Options {
    fn default() -> Self {
        Self {
            pages: Vec::new(),
            date: String::new(),
            wire_labels: true,
            reports: Vec::new(),
            underlays: BTreeMap::new(),
            unreadable: BTreeMap::new(),
            panels: Vec::new(),
            networks: Vec::new(),
            footprints: BTreeMap::new(),
            plc_cards: Vec::new(),
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
    // The logo is read once and stored in the file once, however many pages show it.
    let logo = project.meta.logo.as_ref().and_then(|l| {
        let found = match options.underlays.get(&l.file) {
            Some(bytes) => picture::decode(bytes).map_err(|why| format!("logo {}: {why}", l.file)),
            None => Err(match options.unreadable.get(&l.file) {
                Some(why) => format!("logo picture {} cannot be read: {why}", l.file),
                None => format!("logo picture {} is missing", l.file),
            }),
        };
        found.map_err(|w| report.warnings.push(w)).ok().map(std::sync::Arc::new)
    });
    // Every sheet shows the logo in its title block; the cover page also shows it larger.
    let place_logo = |drawn: &mut writer::Drawn, page: &wc_core::Page| {
        let Some(img) = &logo else { return };
        for b in layout::logo_boxes(project, page) {
            let (min, max) = layout::fit_picture(b, (img.width, img.height));
            drawn.pictures.push(writer::Placed {
                image: img.clone(),
                min: (min.x, min.y),
                max: (max.x, max.y),
            });
        }
    };
    let xref = layout::CrossRefs::build(project, symbols);
    // Report pages use the size of the first schematic page (a larger one when that is too
    // short for a table) and are counted in the sheet total. Panel layouts keep that page's size.
    let first_size = pages.first().map(|p| p.size).unwrap_or_default();
    // Room for a grouped table's heading with its first row, and its line of header values.
    let lines = options
        .reports
        .iter()
        .map(|t| usize::from(t.group.is_some()) + usize::from(!t.header.is_empty()) + 1)
        .max()
        .unwrap_or(1);
    let size = layout::report_size(first_size, project.meta.template.band_height(), lines);
    let per_page = layout::rows_per_page(size, project.meta.template.band_height());
    // A sheet too short for a table's header values and a row (none is large enough) leaves
    // the header values out, so no page holds more lines than it has room for.
    let reports: Vec<std::borrow::Cow<'_, wc_report::Table>> = options
        .reports
        .iter()
        .map(|t| {
            // A front table (the table of contents) lists only the pages this PDF holds: a page
            // left out with `pages` would otherwise show as a row without a link (#692).
            let t = if t.front && !options.pages.is_empty() {
                std::borrow::Cow::Owned(only_exported_rows(t, &options.pages))
            } else {
                std::borrow::Cow::Borrowed(t)
            };
            if t.header.is_empty() || layout::prints_header(&t, per_page) {
                t
            } else {
                let mut t = t.into_owned();
                t.header.clear();
                report.warnings.push(format!(
                    "{}: the sheet has no room for the header values above the table; they are left out",
                    t.title
                ));
                std::borrow::Cow::Owned(t)
            }
        })
        .collect();
    // The lines of each report table's pages.
    let paged: Vec<Vec<Vec<layout::TableLine>>> = reports.iter().map(|t| layout::table_pages(t, per_page)).collect();
    // Each report page: (table, its page, part, parts). Front tables (the table of contents)
    // come before the schematic pages, the rest after them.
    let split = |front: bool| -> Vec<(usize, usize, usize, usize)> {
        options
            .reports
            .iter()
            .enumerate()
            .filter(|(_, t)| t.front == front)
            .flat_map(|(ti, _)| {
                let n = paged.get(ti).map_or(1, Vec::len);
                (0..n).map(move |k| (ti, k, k + 1, n))
            })
            .collect()
    };
    let (front_pages, back_pages) = (split(true), split(false));
    // Each drawn card sheet: (card, first row, part, parts).
    let card_size = layout::card_size(first_size, project.meta.template.band_height());
    let card_rows = layout::card_rows_per_page(card_size, project.meta.template.band_height());
    let card_pages: Vec<(usize, usize, usize, usize)> = options
        .plc_cards
        .iter()
        .enumerate()
        .flat_map(|(ci, card)| {
            let n = layout::card_rows_of(card).len().div_ceil(card_rows).max(1);
            (0..n).map(move |k| (ci, k * card_rows, k + 1, n))
        })
        .collect();
    let total = front_pages.len()
        + pages.len()
        + options.panels.len()
        + card_pages.len()
        + back_pages.len()
        + options.networks.len();
    report.pages = total;
    let mut doc = writer::Doc::new(&project.meta.name);
    let mut sheet = 0;
    let mut report_no = 0;
    let mut add_reports = |doc: &mut writer::Doc, list: Vec<(usize, usize, usize, usize)>, sheet: &mut usize| {
        for (ti, k, part, parts) in list {
            let Some(table) = reports.get(ti) else { continue };
            let lines = paged.get(ti).and_then(|p| p.get(k)).map_or(&[][..], Vec::as_slice);
            report_no += 1;
            let mut page = wc_core::Page::new(
                format!("report-{ti}-{part}"),
                format!("R{report_no}"),
                table.title.clone(),
            );
            page.size = size;
            let mut drawn = layout::draw_table_page(
                project,
                &page,
                *sheet,
                total,
                table,
                lines,
                (part, parts),
                &options.date,
            );
            place_logo(&mut drawn, &page);
            let title = if parts > 1 {
                format!("{} {} ({part}/{parts})", page.number, table.title)
            } else {
                format!("{} {}", page.number, table.title)
            };
            doc.add_page(size.mm(), &page.number, &title, drawn);
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
                None => report.warnings.push(match options.unreadable.get(&u.file) {
                    Some(why) => format!(
                        "page {}: underlay picture {} cannot be read: {why}",
                        page.number, u.file
                    ),
                    None => format!("page {}: underlay picture {} is missing", page.number, u.file),
                }),
            }
        }
        place_logo(&mut drawn, page);
        doc.add_page(
            page.size.mm(),
            &page.number,
            &format!("{} {}", page.number, page.title),
            drawn,
        );
        sheet += 1;
    }
    // Panel layouts follow the schematic pages, one sheet each.
    for (k, panel) in options.panels.iter().enumerate() {
        let mut page = wc_core::Page::new(format!("panel-{}", panel.id), format!("P{}", k + 1), panel.name.clone());
        page.size = first_size;
        let mut drawn =
            layout::draw_panel_page(project, &page, sheet, total, panel, &options.footprints, &options.date);
        place_logo(&mut drawn, &page);
        doc.add_page(
            first_size.mm(),
            &page.number,
            &format!("{} {}", page.number, panel.name),
            drawn,
        );
        sheet += 1;
    }
    // Drawn PLC cards follow the panel layouts.
    for (k, &(ci, first, part, parts)) in card_pages.iter().enumerate() {
        let Some(card) = options.plc_cards.get(ci) else {
            continue;
        };
        let mut page = wc_core::Page::new(
            format!("plc-card-{ci}-{part}"),
            format!("C{}", k + 1),
            format!("PLC card {}", card.card),
        );
        page.size = card_size;
        let mut drawn = layout::draw_card_page(project, &page, sheet, total, card, first, (part, parts), &options.date);
        place_logo(&mut drawn, &page);
        doc.add_page(
            card_size.mm(),
            &page.number,
            &format!("{} {}", page.number, page.title),
            drawn,
        );
        sheet += 1;
    }
    add_reports(&mut doc, back_pages, &mut sheet);
    // Network topologies close the document, after the network list.
    // A topology box links to a page this PDF holds.
    let exported: std::collections::BTreeSet<&str> = pages.iter().map(|p| p.id.as_str()).collect();
    for (k, net) in options.networks.iter().enumerate() {
        // A sheet the whole topology fits on: the first page's size, or a larger one.
        let (size, warn) = topology::sheet(first_size, project.meta.template.band_height(), net);
        report.warnings.extend(warn);
        let mut page = wc_core::Page::new(
            format!("network-{}", net.name),
            format!("N{}", k + 1),
            format!("Network {}", net.name),
        );
        page.size = size;
        let mut drawn = topology::draw_network_page(
            project,
            &page,
            sheet,
            total,
            net,
            &options.date,
            &exported,
            &mut report.warnings,
        );
        place_logo(&mut drawn, &page);
        doc.add_page(
            size.mm(),
            &page.number,
            &format!("{} {}", page.number, page.title),
            drawn,
        );
        sheet += 1;
    }
    // Links resolve against the exported page list, so links to pages left out are dropped.
    let index: BTreeMap<String, usize> = pages
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.clone(), offset + i))
        .collect();
    (doc.finish(&index), report)
}

/// `table` without the rows that describe a page not in `pages`. Rows that describe no page
/// stay.
fn only_exported_rows(table: &wc_report::Table, pages: &[String]) -> wc_report::Table {
    let page_of = |n: usize| table.row_pages.get(n).map_or("", String::as_str);
    let kept: Vec<usize> = (0..table.rows.len())
        .filter(|&n| page_of(n).is_empty() || pages.iter().any(|p| p == page_of(n)))
        .collect();
    let mut row_pages: Vec<String> = kept.iter().map(|&n| page_of(n).to_owned()).collect();
    while row_pages.last().is_some_and(String::is_empty) {
        row_pages.pop();
    }
    wc_report::Table {
        rows: kept.iter().filter_map(|&n| table.rows.get(n).cloned()).collect(),
        row_pages,
        ..table.clone()
    }
}

#[cfg(test)]
mod tests;
