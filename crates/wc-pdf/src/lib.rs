//! Vector PDF export: schematic pages, report tables, panel layouts, PLC card sheets and
//! network topology sheets.
//!
//! Output is plain PDF 1.7 written by hand (no dependencies): lines and curves stay vectors,
//! text uses the standard Helvetica font so tags are searchable, every page gets a bookmark,
//! and cross-references between a coil and its contacts are clickable links. The schematic
//! pages also export as SVG and DXF (`vector`).

mod layout;
mod picture;
mod topology;
mod vector;
mod writer;

use std::collections::BTreeMap;

use wc_core::{Project, Symbol};

pub use layout::{
    CrossRefs, FRAME_COLUMNS, FrameMark, Ink, XREF_HEIGHT, XrefMark, column_of, cover_logo_box, cover_logo_default,
    fit_picture, frame_marks, jumper_points, junctions, logo_boxes, pin_position, tag_lines, title_logo_box,
    title_logo_boxes, xref_marks,
};
pub use picture::{Pictures, read_picture, read_pictures};
pub use vector::{
    PageDrawing, Picture, dxf, dxf_warnings, page_drawings, page_file_stem, page_lines, panel_file_stem, svg,
};
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

/// The warning [`export`] gives when there is no sheet to print ([`Report::pages`] is 0): the
/// bytes are then a PDF without pages, which most readers refuse to open, so callers show this
/// instead of saving them.
pub const NOTHING_TO_EXPORT: &str = "nothing to export: the project has no pages";

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
    let card_size = layout::card_size(first_size, project.meta.template.band_height());
    let card_rows = layout::card_rows_per_page(card_size, project.meta.template.band_height());
    // The contents also lists the sheets that follow the schematic pages, each row linked to
    // its sheet (#693).
    let (reports, extra_ids) = list_extra_sheets(reports, options, per_page, card_rows, |t| {
        t.at_page.as_deref().is_none_or(|at| pages.iter().any(|p| p.id == at))
    });
    // The lines of each report table's pages.
    let paged: Vec<Vec<Vec<layout::TableLine>>> = reports.iter().map(|t| layout::table_pages(t, per_page)).collect();
    // Each report page: (table, its page, part, parts). Front tables (the table of contents)
    // come before the schematic pages, the rest after them.
    let split = |front: bool| -> Vec<(usize, usize, usize, usize)> {
        options
            .reports
            .iter()
            .enumerate()
            .filter(|(_, t)| t.front == front && t.at_page.is_none())
            .flat_map(|(ti, _)| {
                let n = paged.get(ti).map_or(1, Vec::len);
                (0..n).map(move |k| (ti, k, k + 1, n))
            })
            .collect()
    };
    let (front_pages, back_pages) = (split(true), split(false));
    // The report pages' tables (`Table::at_page`), each printed in its page's place: for each
    // exported page, the sheets (table, its page, part, parts) that stand for it. A table for a
    // page this PDF leaves out is not printed.
    let mut placed: BTreeMap<&str, Vec<(usize, usize, usize, usize)>> = BTreeMap::new();
    for (ti, t) in options.reports.iter().enumerate() {
        let Some(at) = t.at_page.as_deref() else { continue };
        if !pages.iter().any(|p| p.id == at) {
            continue;
        }
        let n = paged.get(ti).map_or(1, Vec::len);
        placed.entry(at).or_default().extend((0..n).map(|k| (ti, k, k + 1, n)));
    }
    let page_sheets: usize = pages
        .iter()
        .map(|p| placed.get(p.id.as_str()).map_or(1, |s| s.len().max(1)))
        .sum();
    // Each drawn card sheet: (card, first row, part, parts).
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
        + page_sheets
        + options.panels.len()
        + card_pages.len()
        + back_pages.len()
        + options.networks.len();
    report.pages = total;
    // A PDF without pages does not open in most readers (#833); callers check `pages` and
    // show this instead of saving the file.
    if total == 0 {
        report.warnings.push(NOTHING_TO_EXPORT.into());
    }
    let mut doc = writer::Doc::new(&project.meta.name);
    // The reader's document properties show who drew the project and what it is (#1095).
    let field = |k: &str| project.meta.title_block.get(k).map_or("", String::as_str);
    doc.set_info("Author", field("drawn_by"));
    doc.set_info("Subject", field("description"));
    let mut sheet = 0;
    let mut report_no = 0;
    // `on` is the report page the sheets stand for: they take its number, title and fields.
    let mut add_reports = |doc: &mut writer::Doc,
                           list: Vec<(usize, usize, usize, usize)>,
                           on: Option<&wc_core::Page>,
                           sheet: &mut usize,
                           warnings: &mut Vec<String>| {
        for (ti, k, part, parts) in list {
            let Some(table) = reports.get(ti) else { continue };
            let lines = paged.get(ti).and_then(|p| p.get(k)).map_or(&[][..], Vec::as_slice);
            let mut page = match on {
                Some(on) => {
                    let mut page = on.clone();
                    if page.title.trim().is_empty() {
                        page.title = table.title.clone();
                    }
                    show_latest_revision(project, &mut page);
                    page
                }
                None => {
                    report_no += 1;
                    sheet_page(
                        project,
                        format!("report-{ti}-{part}"),
                        format!("R{report_no}"),
                        table.title.clone(),
                    )
                }
            };
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
                format!("{} {} ({part}/{parts})", page.number, page.title)
            } else {
                format!("{} {}", page.number, page.title)
            };
            warnings.extend(unprintable(&page.number, &drawn));
            doc.add_page(size.mm(), &page.number, &title, drawn);
            *sheet += 1;
        }
    };
    add_reports(&mut doc, front_pages, None, &mut sheet, &mut report.warnings);
    // Where each exported page starts in the document, for links.
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for page in &pages {
        index.insert(page.id.clone(), sheet);
        if let Some(list) = placed.get(page.id.as_str()) {
            if !page.symbols.is_empty() || !page.wires.is_empty() || !page.graphics.is_empty() {
                report.warnings.push(format!(
                    "page {}: a report page; what is drawn on it is not printed",
                    page.number
                ));
            }
            add_reports(&mut doc, list.clone(), Some(page), &mut sheet, &mut report.warnings);
            continue;
        }
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
        report.warnings.extend(unprintable(&page.number, &drawn));
        doc.add_page(
            page.size.mm(),
            &page.number,
            &format!("{} {}", page.number, page.title),
            drawn,
        );
        sheet += 1;
    }
    // The sheets after the pages start here, in the order the contents lists them.
    let after_pages = sheet;
    // Panel layouts follow the schematic pages, one sheet each.
    for (k, panel) in options.panels.iter().enumerate() {
        let mut page = sheet_page(
            project,
            format!("panel-{}", panel.id),
            format!("P{}", k + 1),
            panel.name.clone(),
        );
        page.size = first_size;
        let mut drawn =
            layout::draw_panel_page(project, &page, sheet, total, panel, &options.footprints, &options.date);
        place_logo(&mut drawn, &page);
        report.warnings.extend(unprintable(&page.number, &drawn));
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
        let mut page = sheet_page(
            project,
            format!("plc-card-{ci}-{part}"),
            format!("C{}", k + 1),
            format!("PLC card {}", card.card),
        );
        page.size = card_size;
        let mut drawn = layout::draw_card_page(project, &page, sheet, total, card, first, (part, parts), &options.date);
        place_logo(&mut drawn, &page);
        report.warnings.extend(unprintable(&page.number, &drawn));
        doc.add_page(
            card_size.mm(),
            &page.number,
            &format!("{} {}", page.number, page.title),
            drawn,
        );
        sheet += 1;
    }
    add_reports(&mut doc, back_pages, None, &mut sheet, &mut report.warnings);
    // Network topologies close the document, after the network list.
    // A topology box links to a page this PDF holds.
    let exported: std::collections::BTreeSet<&str> = pages.iter().map(|p| p.id.as_str()).collect();
    for (k, net) in options.networks.iter().enumerate() {
        // A sheet the whole topology fits on: the first page's size, or a larger one.
        let (size, warn) = topology::sheet(first_size, project.meta.template.band_height(), net);
        report.warnings.extend(warn);
        let mut page = sheet_page(
            project,
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
        report.warnings.extend(unprintable(&page.number, &drawn));
        doc.add_page(
            size.mm(),
            &page.number,
            &format!("{} {}", page.number, page.title),
            drawn,
        );
        sheet += 1;
    }
    // Links resolve against the exported page list, so links to pages left out are dropped.
    // The sheets after the schematic pages follow them in the order the contents lists them.
    index.extend(extra_ids.into_iter().enumerate().map(|(i, id)| (id, after_pages + i)));
    (doc.finish(&index), report)
}

/// The warning for a sheet whose text has characters the PDF's Helvetica cannot print, naming
/// each once: they come out as `?`, so a "10 kΩ" note reads "10 k?". An arrow, which prints
/// as `>`, is not one of them. `None` when every character prints as itself.
fn unprintable(number: &str, drawn: &writer::Drawn) -> Option<String> {
    let mut missing: Vec<char> = Vec::new();
    for prim in &drawn.prims {
        let writer::Prim::Text { text, .. } = prim else {
            continue;
        };
        for c in text.chars() {
            if c != '\u{2192}' && !wc_core::font::pdf_prints(c) && !missing.contains(&c) {
                missing.push(c);
            }
        }
    }
    if missing.is_empty() {
        return None;
    }
    let list: Vec<String> = missing.iter().map(|c| c.escape_debug().to_string()).collect();
    Some(format!(
        "page {number}: the PDF cannot print {} in its text; they show as ?",
        list.join(" ")
    ))
}

/// Link target of the `n`th sheet after the schematic pages; no page id starts with a
/// control character.
fn extra_sheet_id(n: usize) -> String {
    format!("\u{1}sheet-{n}")
}

/// `reports` with a row per sheet that follows the schematic pages (panel layouts, PLC cards,
/// the report pages after them and network topologies, in that order) added to each front
/// table (table of contents) that `printed` says the PDF prints, and the link targets of those
/// sheets in that order. A front table that grows onto another page moves the report numbers,
/// which count from the first front page.
fn list_extra_sheets<'a>(
    mut reports: Vec<std::borrow::Cow<'a, wc_report::Table>>,
    options: &Options,
    per_page: usize,
    card_rows: usize,
    printed: impl Fn(&wc_report::Table) -> bool,
) -> (Vec<std::borrow::Cow<'a, wc_report::Table>>, Vec<String>) {
    // A contents on a report page the PDF leaves out is not printed, so it lists nothing.
    let contents: Vec<usize> = (0..reports.len())
        .filter(|&i| reports.get(i).is_some_and(|t| t.front && printed(t)))
        .collect();
    if contents.is_empty() {
        return (reports, Vec::new());
    }
    let parted = |title: &str, part: usize, parts: usize| {
        if parts > 1 {
            format!("{title} ({part}/{parts})")
        } else {
            title.to_owned()
        }
    };
    // (number, title); a report page's number is filled in once the front pages are counted.
    let mut rows: Vec<(Option<String>, String)> = Vec::new();
    for (k, panel) in options.panels.iter().enumerate() {
        rows.push((Some(format!("P{}", k + 1)), panel.name.clone()));
    }
    let mut card_no = 0;
    for card in &options.plc_cards {
        let n = layout::card_rows_of(card).len().div_ceil(card_rows).max(1);
        for part in 1..=n {
            card_no += 1;
            rows.push((
                Some(format!("C{card_no}")),
                parted(&format!("PLC card {}", card.card), part, n),
            ));
        }
    }
    for t in reports.iter().filter(|t| !t.front && t.at_page.is_none()) {
        let n = layout::table_pages(t, per_page).len();
        for part in 1..=n {
            rows.push((None, parted(&t.title, part, n)));
        }
    }
    for (k, net) in options.networks.iter().enumerate() {
        rows.push((Some(format!("N{}", k + 1)), format!("Network {}", net.name)));
    }
    if rows.is_empty() {
        return (reports, Vec::new());
    }
    let ids: Vec<String> = (0..rows.len()).map(extra_sheet_id).collect();
    // The built-in columns may be reordered, renamed or left out by the project's layout for
    // the contents: the values go where the "Page" and "Title" columns are, if shown.
    for &i in &contents {
        let Some(t) = reports.get_mut(i) else { continue };
        let t = t.to_mut();
        let page_col = t.column_of("Page");
        let title_col = t.column_of("Title");
        // Every earlier row describes a page or none; the new ones each describe a sheet.
        t.row_pages.resize(t.rows.len(), String::new());
        for (k, (number, title)) in rows.iter().enumerate() {
            let mut row = vec![String::new(); t.columns.len()];
            // A report page's number is not known yet: a stand-in, different from every
            // other row's, pages the table as the real number will (a contents grouped by
            // page prints a heading per number).
            if let Some(cell) = page_col.and_then(|c| row.get_mut(c)) {
                *cell = number.clone().unwrap_or_else(|| format!("\u{1}{k}"));
            }
            if let Some(cell) = title_col.and_then(|c| row.get_mut(c)) {
                cell.clone_from(title);
            }
            t.rows.push(row);
        }
        t.row_pages.extend(ids.iter().cloned());
    }
    let front: usize = reports
        .iter()
        .filter(|t| t.front && t.at_page.is_none())
        .map(|t| layout::table_pages(t, per_page).len())
        .sum();
    for &i in &contents {
        let Some(t) = reports.get_mut(i) else { continue };
        let t = t.to_mut();
        let Some(c) = t.column_of("Page") else { continue };
        let mut report_no = front;
        let start = t.rows.len() - rows.len();
        for ((number, _), row) in rows.iter().zip(t.rows.iter_mut().skip(start)) {
            if number.is_none() {
                report_no += 1;
                if let Some(cell) = row.get_mut(c) {
                    *cell = format!("R{report_no}");
                }
            }
        }
    }
    (reports, ids)
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

/// A sheet the PDF makes itself (a report, panel layout, PLC card or network page): a page
/// with nothing of its own, except that a revision field of [`wc_core::project::AUTO_REVISION`]
/// shows the project's latest revision ([`show_latest_revision`]).
fn sheet_page(
    project: &Project,
    id: impl Into<String>,
    number: impl Into<String>,
    title: impl Into<String>,
) -> wc_core::Page {
    let mut page = wc_core::Page::new(id, number, title);
    show_latest_revision(project, &mut page);
    page
}

/// A sheet made from the project as it is now (a generated sheet, or a report page's) shows the
/// latest revision where its revision field is `auto`, not the one its clouds would give.
fn show_latest_revision(project: &Project, page: &mut wc_core::Page) {
    let auto = wc_core::project::is_auto_revision(page.title_field_set(&project.meta, "revision").0);
    let latest = project
        .meta
        .revisions
        .iter()
        .rev()
        .find(|r| !wc_core::project::is_auto_revision(&r.letter));
    if auto && let Some(latest) = latest {
        page.title_block.insert("revision".into(), latest.letter.clone());
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod helper_tests {
    use std::borrow::Cow;

    use super::*;

    fn text(s: &str) -> writer::Prim {
        writer::Prim::Text {
            at: (0.0, 0.0),
            anchor: (0.0, 0.0),
            align: 0,
            height: 2.5,
            angle: 0.0,
            text: s.to_owned(),
            ink: writer::Stroke {
                rgb: (0.0, 0.0, 0.0),
                width: 0.25,
            },
            layer: None,
        }
    }

    fn drawn(texts: &[&str]) -> writer::Drawn {
        writer::Drawn {
            prims: texts.iter().map(|t| text(t)).collect(),
            ..writer::Drawn::default()
        }
    }

    fn table(title: &str, columns: &[&str], rows: usize, front: bool) -> wc_report::Table {
        wc_report::Table {
            title: title.to_owned(),
            columns: columns.iter().map(|c| (*c).to_owned()).collect(),
            rows: (0..rows).map(|n| vec![format!("{n}"); columns.len()]).collect(),
            front,
            ..wc_report::Table::default()
        }
    }

    fn panel(name: &str) -> wc_core::panel::Panel {
        wc_core::panel::Panel {
            id: name.to_lowercase(),
            name: name.to_owned(),
            width: 600.0,
            height: 800.0,
            items: Vec::new(),
        }
    }

    #[test]
    fn unprintable_is_none_for_latin_text_and_arrows() {
        assert_eq!(
            unprintable("1", &drawn(&["K1 \u{2192} 3.4", "Müller 10 °C", "€"])),
            None
        );
        assert_eq!(unprintable("1", &writer::Drawn::default()), None);
    }

    #[test]
    fn unprintable_names_each_character_once_in_first_seen_order() {
        let warning = unprintable("7", &drawn(&["10 k\u{3a9}", "\u{4e2d}", "1 \u{3a9}", "a\tb"]));
        assert_eq!(
            warning.as_deref(),
            Some("page 7: the PDF cannot print \u{3a9} \u{4e2d} \\t in its text; they show as ?")
        );
    }

    #[test]
    fn extra_sheet_ids_are_distinct_and_cannot_be_page_ids() {
        assert_eq!(extra_sheet_id(0), "\u{1}sheet-0");
        assert_ne!(extra_sheet_id(1), extra_sheet_id(10));
        assert!(extra_sheet_id(3).starts_with('\u{1}'));
    }

    #[test]
    fn extra_sheets_need_a_front_table_and_a_sheet_to_list() {
        let reports = vec![Cow::Owned(table("Devices", &["Page", "Title"], 2, false))];
        let options = Options {
            panels: vec![panel("Main")],
            ..Options::default()
        };
        let (out, ids) = list_extra_sheets(reports.clone(), &options, 40, 20, |_| true);
        assert!(ids.is_empty());
        assert_eq!(out, reports);

        let reports = vec![Cow::Owned(table("Contents", &["Page", "Title"], 2, true))];
        let (out, ids) = list_extra_sheets(reports.clone(), &Options::default(), 40, 20, |_| true);
        assert!(ids.is_empty());
        assert_eq!(out, reports);
    }

    #[test]
    fn extra_sheets_are_numbered_in_order_after_the_front_pages() {
        let mut contents = table("Contents", &["Page", "Title"], 2, true);
        contents.row_pages = vec!["p1".to_owned()];
        let reports = vec![
            Cow::Owned(contents),
            // Three report pages at two rows a page.
            Cow::Owned(table("Wires", &["Wire"], 5, false)),
        ];
        let options = Options {
            panels: vec![panel("Main"), panel("Door")],
            networks: vec![wc_core::network::Network {
                name: "PN1".to_owned(),
                ..wc_core::network::Network::default()
            }],
            ..Options::default()
        };
        let (out, ids) = list_extra_sheets(reports, &options, 2, 20, |_| true);
        assert_eq!(ids, (0..6).map(extra_sheet_id).collect::<Vec<_>>());
        let Some(front) = out.first() else { unreachable!() };
        let added: Vec<(&str, &str)> = front
            .rows
            .iter()
            .skip(2)
            .map(|r| (r[0].as_str(), r[1].as_str()))
            .collect();
        // The contents itself now runs onto four pages, so the first report page is R5.
        assert_eq!(
            added,
            [
                ("P1", "Main"),
                ("P2", "Door"),
                ("R5", "Wires (1/3)"),
                ("R6", "Wires (2/3)"),
                ("R7", "Wires (3/3)"),
                ("N1", "Network PN1"),
            ]
        );
        // The row without a page keeps none; the new rows link to their sheets.
        let mut row_pages = vec!["p1".to_owned(), String::new()];
        row_pages.extend(ids);
        assert_eq!(front.row_pages, row_pages);
        // The other report is left as it was.
        assert_eq!(out.get(1).map(|t| t.rows.len()), Some(5));
    }

    #[test]
    fn extra_sheets_follow_renamed_and_hidden_columns() {
        let mut contents = table("Contents", &["Name", "Sheet"], 0, true);
        contents.sources = vec!["Title".to_owned(), "Page".to_owned()];
        let options = Options {
            panels: vec![panel("Main")],
            ..Options::default()
        };
        let (out, _) = list_extra_sheets(vec![Cow::Owned(contents)], &options, 40, 20, |_| true);
        let rows = out.first().map(|t| t.rows.clone());
        assert_eq!(rows, Some(vec![vec!["Main".to_owned(), "P1".to_owned()]]));

        // Without a page column only the title is filled in.
        let contents = table("Contents", &["Title", "Notes"], 0, true);
        let (out, ids) = list_extra_sheets(vec![Cow::Owned(contents)], &options, 40, 20, |_| true);
        let rows = out.first().map(|t| t.rows.clone());
        assert_eq!(rows, Some(vec![vec!["Main".to_owned(), String::new()]]));
        assert_eq!(ids, [extra_sheet_id(0)]);
    }

    #[test]
    fn only_exported_rows_keeps_rows_of_exported_pages_and_rows_of_none() {
        let mut t = table("Devices", &["Tag"], 4, false);
        t.row_pages = vec!["p1".to_owned(), "p2".to_owned(), String::new(), "p3".to_owned()];
        let kept = only_exported_rows(&t, &["p1".to_owned(), "p2".to_owned()]);
        assert_eq!(
            kept.rows,
            vec![vec!["0".to_owned()], vec!["1".to_owned()], vec!["2".to_owned()]]
        );
        // The trailing row without a page needs no entry.
        assert_eq!(kept.row_pages, ["p1", "p2"]);
        assert_eq!(kept.title, "Devices");
    }

    #[test]
    fn only_exported_rows_keeps_rows_past_a_short_row_pages() {
        let mut t = table("Devices", &["Tag"], 3, false);
        t.row_pages = vec!["p9".to_owned()];
        let kept = only_exported_rows(&t, &["p1".to_owned()]);
        assert_eq!(kept.rows, vec![vec!["1".to_owned()], vec!["2".to_owned()]]);
        assert!(kept.row_pages.is_empty());
    }
}
