//! Reports generated from a project: parts list, device list, wire list, net list and terminal
//! plan. Each report is a [`Table`] that can be written as CSV, Markdown or an Excel workbook, or
//! laid out on PDF report pages.
//!
//! Reports are always derived from the schematic; they are never stored, so they can never be
//! out of date.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use wc_core::connectivity::{self, PinRef};
use wc_core::report_layout::ReportLayout;
use wc_core::symbol::SymbolRole;
use wc_core::{DeviceTag, Endpoint, Part, Project, Symbol};

pub mod aml;
pub mod check;
pub mod configurator;
pub mod marking;
pub mod network;
pub mod plc;
pub mod step;
#[cfg(test)]
mod tests;
pub mod xlsx;

pub use aml::{AmlCounts, automationml};
pub use check::{Finding, Severity, check, check_table, check_with, cloud_index, cloud_item};
pub use configurator::{CONFIGURATOR_COLUMNS, Configurator, Target, configurator_list};
pub use marking::{
    LEVEL_SEPARATOR, Language, LineField, MARKER_PROPERTY, MarkerKind, MarkingOptions, formula_warnings, marking_list,
    marking_list_with, parse_kinds, parse_lines, terminal_strips, to_smartscript_csv,
};
pub use network::{network_list, network_warnings};
pub use plc::{
    CardChannel, CardPoint, PlcCardSheet, PlcVariables, plc_card_overview, plc_card_sheets, plc_rack_warnings,
    plc_variables,
};

/// A finished report: a title, column headings and rows of text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Table {
    pub title: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// For each row, the id of the page it describes (empty when it describes none); a PDF
    /// export makes such rows clickable. Shorter than `rows` when the last rows have none.
    pub row_pages: Vec<String>,
    /// Printed before the schematic pages in a PDF, like the table of contents of a drawing set.
    pub front: bool,
    /// The column the rows are grouped by ([`ReportLayout::group`]): rows of one value come
    /// together, and a PDF prints the value once as a heading above them.
    pub group: Option<usize>,
    /// Title-block values a PDF prints above the table ([`ReportLayout::header`]).
    pub header: Vec<String>,
    /// The built-in column each column shows, by its built-in heading, when a layout
    /// ([`Table::with_layout`]) reordered or renamed them; empty when the columns are the
    /// built-in ones.
    pub sources: Vec<String>,
}

impl Table {
    fn new(title: &str, columns: &[&str]) -> Self {
        Self {
            title: title.into(),
            columns: columns.iter().map(|c| (*c).to_owned()).collect(),
            ..Self::default()
        }
    }

    /// The position of the built-in column `name` among the shown columns, if shown.
    pub fn column_of(&self, name: &str) -> Option<usize> {
        if self.sources.is_empty() {
            self.columns.iter().position(|c| c == name)
        } else {
            self.sources.iter().position(|c| c == name)
        }
    }

    /// RFC 4180 CSV with a header row; fields are quoted when they need it, and cells a
    /// spreadsheet would run as a formula are written as text (see [`formula_safe`]).
    pub fn to_csv(&self) -> String {
        self.to_csv_with(',')
    }

    /// CSV separated by `sep` (`,` or `;`), with a header row and CRLF line ends; fields are
    /// quoted when they contain the separator, a quote or a line end. Every cell, the header
    /// too, goes through [`formula_safe`].
    pub fn to_csv_with(&self, sep: char) -> String {
        self.csv(sep, true)
    }

    /// [`Table::to_csv_with`] with every cell exactly as it is, for a file read by a program
    /// that takes the text literally (a label printer), never for one meant for a spreadsheet.
    pub fn to_csv_raw_with(&self, sep: char) -> String {
        self.csv(sep, false)
    }

    fn csv(&self, sep: char, safe: bool) -> String {
        let field = |s: &str| {
            let s = if safe { formula_safe(s) } else { Cow::Borrowed(s) };
            if s.contains([sep, '"', '\n', '\r']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.into_owned()
            }
        };
        let line = |cells: &[String]| {
            cells
                .iter()
                .map(|c| field(c))
                .collect::<Vec<_>>()
                .join(&sep.to_string())
        };
        let mut out = line(&self.columns);
        out.push_str("\r\n");
        for r in &self.rows {
            out.push_str(&line(r));
            out.push_str("\r\n");
        }
        out
    }

    /// This table in `layout`: its columns picked, ordered and renamed, and its rows sorted.
    /// Columns and sort entries naming no column of the table are left out
    /// ([`layout_warnings`] lists them). Each row keeps its page link.
    pub fn with_layout(self, layout: &ReportLayout) -> Table {
        let index = |name: &str| self.columns.iter().position(|c| c == name);
        let sort: Vec<(usize, bool)> = layout
            .sort
            .iter()
            .filter_map(|entry| {
                let (c, descending) = ReportLayout::sort_key(entry);
                index(c).map(|i| (i, descending))
            })
            .collect();
        let mut picked: Vec<(usize, String)> = if layout.columns.is_empty() {
            self.columns.iter().cloned().enumerate().collect()
        } else {
            layout
                .columns
                .iter()
                .filter_map(|c| index(&c.from).map(|i| (i, c.heading.clone().unwrap_or_else(|| c.from.clone()))))
                .collect()
        };
        // The group column, shown first when the layout leaves it out (every row keeps its value).
        let group = layout.group.as_deref().and_then(|g| index(g.trim()));
        let shown_group = group.map(|g| match picked.iter().position(|(i, _)| *i == g) {
            Some(at) => at,
            None => {
                picked.insert(0, (g, self.columns[g].clone()));
                0
            }
        });
        let source = |i: usize| {
            self.sources
                .get(i)
                .or_else(|| self.columns.get(i))
                .cloned()
                .unwrap_or_default()
        };
        let sources: Vec<String> = picked.iter().map(|&(i, _)| source(i)).collect();
        let Table {
            title,
            rows,
            row_pages,
            front,
            ..
        } = self;
        let mut rows: Vec<(Vec<String>, String)> = rows
            .into_iter()
            .enumerate()
            .map(|(n, r)| (r, row_pages.get(n).cloned().unwrap_or_default()))
            .collect();
        if !sort.is_empty() {
            let cell = |r: &[String], i: usize| r.get(i).map(|v| natural_key(v)).unwrap_or_default();
            // A stable sort: rows equal in every sort column keep the built-in order.
            rows.sort_by(|(a, _), (b, _)| {
                sort.iter()
                    .map(|&(i, descending)| {
                        let o = cell(a, i).cmp(&cell(b, i));
                        if descending { o.reverse() } else { o }
                    })
                    .find(|o| o.is_ne())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        if let Some(g) = group {
            // Each group where its first row is; within it, the sorted order.
            let mut first_seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
            for (r, _) in &rows {
                let next = first_seen.len();
                first_seen.entry(r.get(g).cloned().unwrap_or_default()).or_insert(next);
            }
            let rank = |r: &[String]| {
                let v = r.get(g).map(String::as_str).unwrap_or_default();
                first_seen.get(v).copied().unwrap_or(usize::MAX)
            };
            rows.sort_by_key(|(r, _)| rank(r));
        }
        let mut row_pages: Vec<String> = rows.iter().map(|(_, p)| p.clone()).collect();
        while row_pages.last().is_some_and(String::is_empty) {
            row_pages.pop();
        }
        Table {
            title: layout.title.clone().unwrap_or(title),
            columns: picked.iter().map(|(_, h)| h.clone()).collect(),
            rows: rows
                .into_iter()
                .map(|(r, _)| {
                    picked
                        .iter()
                        .map(|(i, _)| r.get(*i).cloned().unwrap_or_default())
                        .collect()
                })
                .collect(),
            row_pages,
            front,
            group: shown_group,
            header: layout.header.clone(),
            sources,
        }
    }

    /// Adds `row`, which describes the page with id `page_id`: in a PDF the row jumps to it.
    fn push_linked(&mut self, row: Vec<String>, page_id: &str) {
        self.row_pages.resize(self.rows.len(), String::new());
        self.row_pages.push(page_id.to_owned());
        self.rows.push(row);
    }

    /// Rows whose first column is empty take their first `columns` cells from the row above:
    /// a continuation row gets the group it belongs to written out.
    fn fill_down(&mut self, columns: usize) {
        let mut above: Vec<String> = Vec::new();
        for row in &mut self.rows {
            if row.first().is_some_and(String::is_empty) && !above.is_empty() {
                for (cell, value) in row.iter_mut().zip(&above) {
                    cell.clone_from(value);
                }
            } else {
                above = row.iter().take(columns).cloned().collect();
            }
        }
    }

    /// A Markdown table under a level-2 heading.
    pub fn to_markdown(&self) -> String {
        // One line of text: line ends (LF, CR or CRLF) become spaces, and `<` and `>` are
        // written as entities so a value is never read as HTML.
        let line = |s: &str| {
            s.replace("\r\n", " ")
                .replace(['\r', '\n'], " ")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        };
        // In a cell, Markdown punctuation gets a backslash so the table shows the text as typed
        // (see `markdown_cell`); line ends and `<`, `>` are handled as for the title.
        let cell = |s: &str| line(&markdown_cell(s));
        let mut out = format!(
            "## {}\n\n| {} |\n|{}\n",
            line(&self.title),
            self.columns.iter().map(|c| cell(c)).collect::<Vec<_>>().join(" | "),
            "---|".repeat(self.columns.len())
        );
        for r in &self.rows {
            out.push_str(&format!(
                "| {} |\n",
                r.iter().map(|c| cell(c)).collect::<Vec<_>>().join(" | ")
            ));
        }
        out
    }
}

/// One cell's text for a Markdown table, before line ends and `<`, `>` are handled.
///
/// Markdown punctuation gets a backslash so the table shows the text as typed: `*spare*` is
/// not italic, `motor_1_ready` keeps its underscores, `` `X1` `` is not code. A backslash is
/// doubled, so a cell ending in `\` does not escape the column bar after it.
///
/// A word that a Markdown viewer turns into a link by itself (one starting with `http://`,
/// `https://` or `www.`, or an e-mail address) keeps its characters, because a backslash there
/// would end up in the link. Only `\` and `|` are escaped in it, so the table stays whole.
fn markdown_cell(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let word = &rest[..end];
        // Characters before the link that Markdown still lets a link start after.
        let lead = word.len() - word.trim_start_matches(['*', '_', '~', '(']).len();
        let (before, link) = word.split_at(lead);
        markdown_text(before, &mut out);
        if is_autolink(link) {
            // Markdown leaves trailing punctuation and a trailing entity such as `&copy;` out
            // of the link: that part is plain text.
            let mut body = link;
            loop {
                body = body.trim_end_matches(['?', '!', '.', ',', ':', '*', '_', '~']);
                let Some(head) = body.strip_suffix(';') else {
                    break;
                };
                let name = head.trim_end_matches(|c: char| c.is_ascii_alphanumeric());
                match name.strip_suffix('&') {
                    Some(kept) if name.len() < head.len() => body = kept,
                    _ => break,
                }
            }
            for c in body.chars() {
                if matches!(c, '\\' | '|') {
                    out.push('\\');
                }
                out.push(c);
            }
            markdown_text(&link[body.len()..], &mut out);
        } else {
            markdown_text(link, &mut out);
        }
        let space = rest[end..].len() - rest[end..].trim_start().len();
        out.push_str(&rest[end..end + space]);
        rest = &rest[end + space..];
    }
    out
}

/// Escapes the Markdown punctuation in plain cell text (see `markdown_cell`).
fn markdown_text(s: &str, out: &mut String) {
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' | '|' | '`' | '*' | '_' | '[' | ']' | '~' | '&' => {
                out.push('\\');
                out.push(c);
            }
            '!' if chars.peek() == Some(&'[') => out.push_str("\\!"),
            '#' if out.is_empty() => out.push_str("\\#"),
            _ => out.push(c),
        }
    }
}

/// Whether a Markdown viewer turns this word into a link by itself: a web address starting
/// with `http://`, `https://` or `www.`, or an e-mail address such as `a_b@example.com`.
fn is_autolink(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    let web = ["http://", "https://", "www."]
        .iter()
        .find(|p| lower.starts_with(**p))
        .map(|p| (&word[p.len()..], *p == "www."));
    if let Some((after, www)) = web {
        // The host must be a domain Markdown accepts: parts of letters, digits, `-` and `_`
        // between dots, with no `_` in the last two parts.
        let host_len = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
            .unwrap_or(after.len());
        let host = if www { &word[..4 + host_len] } else { &after[..host_len] };
        let host = host.trim_end_matches('.');
        let parts: Vec<&str> = host.split('.').collect();
        return parts.iter().all(|p| !p.is_empty())
            && (!www || parts.len() > 1)
            && parts.iter().rev().take(2).all(|p| !p.contains('_'));
    }
    let Some((name, domain)) = word.split_once('@') else {
        return false;
    };
    let domain = domain.trim_end_matches(['.', ',', ':', '!', '?']);
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_'))
        && domain.contains('.')
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// A cell as a spreadsheet should show it: text that Excel or LibreOffice would run as a formula
/// when the CSV is opened (CWE-1236) gets a leading `'`, which makes it plain text there.
///
/// Guarded: cells starting with `=`, `+`, `@`, a tab or a carriage return, and cells starting
/// with `-` unless they are a plain number (`-5`, `-0.75`, a value a column means as a number).
/// Device tags with only a product aspect (`-K1`, `-X12`) are guarded too: Excel reads `-K1` as
/// the formula `=-K1` and shows the negated cell K1 instead of the tag. WiringCraft's own CSV
/// import (`wc_import::csv::records`) takes the `'` off again, so such a file reads back
/// with the same tags. A cell that itself starts with `'`s before one of those characters
/// (`'-K1` as typed) gets one more `'`, so it reads back with its own.
pub fn formula_safe(cell: &str) -> Cow<'_, str> {
    let guard = match cell.chars().next() {
        Some('=' | '+' | '@' | '\t' | '\r') => true,
        Some('-') => !plain_number(&cell[1..]),
        Some('\'') => cell
            .trim_start_matches('\'')
            .starts_with(['=', '+', '-', '@', '\t', '\r']),
        _ => false,
    };
    if guard {
        Cow::Owned(format!("'{cell}"))
    } else {
        Cow::Borrowed(cell)
    }
}

/// Digits with at most one decimal point or comma, such as `5`, `0.75` or `2,5`.
fn plain_number(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',')
        && s.chars().filter(|c| matches!(c, '.' | ',')).count() <= 1
}

/// Which report to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Parts,
    Devices,
    Wires,
    Nets,
    Terminals,
    Cables,
    Plc,
    PlcCards,
    Contents,
    Locations,
    Networks,
}

impl Kind {
    pub const ALL: [Kind; 11] = [
        Kind::Contents,
        Kind::Parts,
        Kind::Devices,
        Kind::Wires,
        Kind::Nets,
        Kind::Terminals,
        Kind::Cables,
        Kind::Plc,
        Kind::PlcCards,
        Kind::Locations,
        Kind::Networks,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Parts => "parts",
            Kind::Devices => "devices",
            Kind::Wires => "wires",
            Kind::Nets => "nets",
            Kind::Terminals => "terminals",
            Kind::Cables => "cables",
            Kind::Plc => "plc",
            Kind::PlcCards => "plc-cards",
            Kind::Contents => "contents",
            Kind::Locations => "locations",
            Kind::Networks => "networks",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == s)
    }
}

/// Everything a report may need besides the project.
pub struct Inputs<'a> {
    pub project: &'a Project,
    /// The project's vendored symbols by id.
    pub symbols: &'a BTreeMap<String, Symbol>,
    /// The project's vendored parts by id.
    pub parts: &'a BTreeMap<String, Part>,
}

/// The report of `kind`, in the project's own layout for it if it has one
/// ([`wc_core::report_layout`]), else in the built-in layout.
pub fn build(kind: Kind, i: &Inputs<'_>) -> Table {
    build_with(kind, i, layout_for(i.project, kind))
}

/// The report of `kind` in `layout`, or in the built-in layout when there is none: a layout
/// being designed shows as it will print before it is kept in the project.
pub fn build_with(kind: Kind, i: &Inputs<'_>, layout: Option<&ReportLayout>) -> Table {
    let mut table = build_builtin(kind, i);
    match layout {
        Some(layout) => {
            // The cable list names a cable only on its first core's row. A sort or a grouping
            // can move the other rows away from it, so each row then names its cable itself.
            if kind == Kind::Cables && (!layout.sort.is_empty() || layout.group.is_some()) {
                table.fill_down(3);
            }
            table.with_layout(layout)
        }
        None => table,
    }
}

/// The project's layout for `kind`: the first one that names it.
pub fn layout_for(project: &Project, kind: Kind) -> Option<&ReportLayout> {
    project.meta.reports.iter().find(|l| l.kind == kind.name())
}

/// What is wrong with the project's report layouts: an unknown report kind, a second layout
/// for one kind (only the first is used), or a column or sort entry the report does not have
/// (left out). Empty when they are all fine.
pub fn layout_warnings(i: &Inputs<'_>) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for layout in &i.project.meta.reports {
        let Some(kind) = Kind::parse(&layout.kind) else {
            let names: Vec<&str> = Kind::ALL.iter().map(|k| k.name()).collect();
            out.push(format!(
                "report layout for \"{}\": no such report (one of {})",
                layout.kind,
                names.join(", ")
            ));
            continue;
        };
        if !seen.insert(kind) {
            out.push(format!(
                "report layout for \"{}\": a second one, left unused",
                layout.kind
            ));
            continue;
        }
        out.extend(layout_problems(kind, layout, i));
    }
    out
}

/// What in `layout`, a layout for the report of `kind`, names nothing: a column, sort or
/// group column the report does not have, or a header key no title block knows. The report
/// layout dialog shows these for the layout being edited.
pub fn layout_problems(kind: Kind, layout: &ReportLayout, i: &Inputs<'_>) -> Vec<String> {
    layout_problems_for(&build_builtin(kind, i).columns, layout, i.project)
}

/// [`layout_problems`] for a report whose built-in columns are `columns`, for a caller that
/// has them already (the report layout dialog), so the report is not built again for it.
pub fn layout_problems_for(columns: &[String], layout: &ReportLayout, project: &Project) -> Vec<String> {
    let mut out = Vec::new();
    let known = |c: &str| columns.iter().any(|b| b == c);
    for c in &layout.columns {
        if !known(&c.from) {
            out.push(format!(
                "report layout for \"{}\": no column \"{}\" (the columns are {})",
                layout.kind,
                c.from,
                columns.join(", ")
            ));
        }
    }
    for entry in &layout.sort {
        let (c, _) = ReportLayout::sort_key(entry);
        if !known(c) {
            out.push(format!(
                "report layout for \"{}\": cannot sort by \"{c}\", no such column",
                layout.kind
            ));
        }
    }
    if let Some(g) = layout.group.as_deref().map(str::trim)
        && !known(g)
    {
        out.push(format!(
            "report layout for \"{}\": cannot group by \"{g}\", no such column (the columns are {})",
            layout.kind,
            columns.join(", ")
        ));
    }
    let keys = header_keys(project);
    for key in &layout.header {
        if !keys.contains(key) {
            out.push(format!(
                "report layout for \"{}\": header \"{key}\" is not a title-block field (one of {})",
                layout.kind,
                keys.join(", ")
            ));
        }
    }
    out
}

/// The keys a report layout's `header` can name: the project name, the page keys a title
/// block uses, the usual title-block fields and any other the project or one of its pages has.
fn header_keys(project: &Project) -> Vec<String> {
    use wc_core::template::{PAGE_NUMBER, PAGE_TITLE, PROJECT_NAME, SHEET};
    let mut keys: Vec<String> = [PROJECT_NAME, PAGE_TITLE, PAGE_NUMBER, SHEET]
        .into_iter()
        .chain(wc_core::project::TITLE_FIELDS.iter().map(|(k, _)| *k))
        .map(str::to_owned)
        .collect();
    let others = project
        .meta
        .title_block
        .keys()
        .chain(project.pages.iter().flat_map(|p| p.title_block.keys()));
    for k in others {
        if !keys.contains(k) {
            keys.push(k.clone());
        }
    }
    keys
}

/// The report of `kind` in its built-in layout.
pub fn build_builtin(kind: Kind, i: &Inputs<'_>) -> Table {
    match kind {
        Kind::Parts => parts_list(i),
        Kind::Devices => device_list(i),
        Kind::Wires => wire_list(i),
        Kind::Nets => net_list(i),
        Kind::Terminals => terminal_plan(i),
        Kind::Cables => cable_list(i),
        Kind::Plc => plc_io_list(i),
        Kind::PlcCards => plc_card_overview(i),
        Kind::Contents => contents(i),
        Kind::Locations => location_list(i),
        Kind::Networks => network_list(i),
    }
}

/// Locations: every location of the project with its description and how many devices are in
/// it (by the `+` part of their tags; a device in `+L1.L3` counts for `L1.L3`, not for `L1`), then
/// locations device tags use that the project does not list.
pub fn location_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new("Locations", &["Location", "Description", "Devices", "Note"]);
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for tag in all_tags(i).iter().filter(|t| !t.location.is_empty()) {
        *count.entry(tag.location.join(".")).or_default() += 1;
    }
    for l in &i.project.meta.locations {
        let n = count.remove(&l.path).unwrap_or(0);
        t.rows.push(vec![
            format!("+{}", l.path),
            l.description.clone(),
            n.to_string(),
            String::new(),
        ]);
    }
    for (path, n) in count {
        t.rows.push(vec![
            format!("+{path}"),
            String::new(),
            n.to_string(),
            "not in the project's locations".into(),
        ]);
    }
    t
}

pub use wc_core::project::natural_key;

/// Every device tag in the project: drawn ones and ones only listed in `devices.json`.
/// Every real device: tags drawn on pages or listed in the devices file. Interruption points
/// carry tags too, but they only name a connection, so a tag used by nothing else is left out.
fn all_tags(i: &Inputs<'_>) -> BTreeSet<DeviceTag> {
    let p = i.project;
    let index = p.tag_index();
    let only_links = |tag: &DeviceTag| {
        index.get(tag).is_some_and(|uses| {
            !uses.is_empty()
                && uses.iter().all(|(page, item)| {
                    p.page(page)
                        .and_then(|pg| pg.symbol(item))
                        .and_then(|s| i.symbols.get(&s.symbol))
                        .is_some_and(|sym| sym.role == SymbolRole::Interruption)
                })
        })
    };
    index
        .keys()
        .cloned()
        .chain(p.devices.keys().cloned())
        .filter(|t| !only_links(t))
        .collect()
}

/// Page numbers where a tag is drawn, as `2, 3`.
fn pages_of(p: &Project, uses: &[(String, String)]) -> String {
    let mut nums: Vec<String> = uses
        .iter()
        .filter_map(|(pid, _)| p.page(pid).map(|pg| pg.number.clone()))
        .collect();
    nums.sort_by_key(|n| natural_key(n));
    nums.dedup();
    nums.join(", ")
}

/// Parts list (bill of materials): one row per part, with quantity and the devices using it.
/// A device's accessories count with their quantity. Devices without any part come last so the
/// gaps are visible.
pub fn parts_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new(
        "Parts list",
        &["Pos", "Qty", "Manufacturer", "Order number", "Description", "Devices"],
    );
    let mut by_part: BTreeMap<String, (u64, Vec<DeviceTag>)> = BTreeMap::new();
    let mut without: Vec<DeviceTag> = Vec::new();
    for tag in all_tags(i) {
        let parts = i.project.devices.get(&tag).map(|d| d.all_parts()).unwrap_or_default();
        if parts.is_empty() {
            without.push(tag);
            continue;
        }
        for (part, qty) in parts {
            let (n, tags) = by_part.entry(part.to_owned()).or_default();
            *n += u64::from(qty);
            if tags.last() != Some(&tag) {
                tags.push(tag.clone());
            }
        }
    }
    // Jumpers with a part count once each, under their strip.
    for j in &i.project.meta.jumpers {
        if let Some(part) = &j.part {
            let (n, tags) = by_part.entry(part.clone()).or_default();
            *n += 1;
            if !tags.contains(&j.strip) {
                tags.push(j.strip.clone());
            }
        }
    }
    let mut pos = 0;
    for (part_id, (qty, tags)) in &by_part {
        pos += 1;
        let part = i.parts.get(part_id);
        t.rows.push(vec![
            pos.to_string(),
            qty.to_string(),
            part.map(|p| p.manufacturer.clone()).unwrap_or_default(),
            part.map(|p| p.order_number.clone()).unwrap_or_else(|| part_id.clone()),
            part.map(|p| p.description.clone())
                .unwrap_or_else(|| "part not in the project's library copy".into()),
            tags.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
        ]);
    }
    for tag in without {
        t.rows.push(vec![
            String::new(),
            "1".into(),
            String::new(),
            String::new(),
            "no part assigned".into(),
            tag.to_string(),
        ]);
    }
    t
}

/// Device list: every tag with its description, part and the pages it appears on.
pub fn device_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new("Device list", &["Device", "Description", "Part", "Symbols", "Pages"]);
    let index = i.project.tag_index();
    for tag in all_tags(i) {
        let dev = i.project.devices.get(&tag);
        let uses = index.get(&tag).map(Vec::as_slice).unwrap_or(&[]);
        let symbols: BTreeSet<String> = uses
            .iter()
            .filter_map(|(pid, item)| i.project.page(pid)?.symbol(item))
            .map(|s| {
                i.symbols
                    .get(&s.symbol)
                    .map(|y| y.name.clone())
                    .unwrap_or_else(|| s.symbol.clone())
            })
            .collect();
        t.rows.push(vec![
            tag.to_string(),
            dev.map(|d| d.description.clone()).unwrap_or_default(),
            dev.and_then(|d| d.part.clone()).unwrap_or_default(),
            symbols.into_iter().collect::<Vec<_>>().join(", "),
            pages_of(i.project, uses),
        ]);
    }
    t
}

/// How a pin is named in reports: `=A+B-K1:13`, or `page 3 s4:13` for an untagged symbol.
fn pin_label(p: &Project, page_id: &str, item: &str, pin: &str) -> String {
    let page = p.page(page_id);
    match page.and_then(|pg| pg.symbol(item)).and_then(|s| s.tag.as_ref()) {
        Some(tag) => format!("{tag}:{pin}"),
        None => format!(
            "page {} {item}:{pin}",
            page.map(|pg| pg.number.as_str()).unwrap_or(page_id)
        ),
    }
}

/// The net each pin belongs to, with its potential name (or a generated one).
fn net_names(i: &Inputs) -> (Vec<connectivity::Net>, BTreeMap<PinRef, String>) {
    let nets = connectivity::nets_linked(i.project, |id| i.symbols.get(id).map(|s| s.role));
    let mut by_pin = BTreeMap::new();
    for (n, net) in nets.iter().enumerate() {
        let name = net.names.first().cloned().unwrap_or_else(|| format!("N{}", n + 1));
        for pin in &net.pins {
            by_pin.insert(pin.clone(), name.clone());
        }
    }
    (nets, by_pin)
}

/// Wire list: every drawn wire with both ends, then its cross-section, colour and type. A free
/// end is reported as a junction point. Each row links to the page the wire is drawn on
/// ([`Table::row_pages`]).
pub fn wire_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new(
        "Wire list",
        &[
            "Page",
            "Wire",
            "Potential",
            "Cable",
            "From",
            "To",
            "Section",
            "Colour",
            "Type",
        ],
    );
    let (_, net_of) = net_names(i);
    for page in &i.project.pages {
        for w in &page.wires {
            let end = |e: &Endpoint| match e {
                Endpoint::Pin { item, pin } => pin_label(i.project, &page.id, item, pin),
                Endpoint::Point { at } => format!("junction {:.1},{:.1}", at.x, at.y),
            };
            let potential = w.potential.clone().or_else(|| {
                [&w.from, &w.to].into_iter().find_map(|e| match e {
                    Endpoint::Pin { item, pin } => net_of.get(&(page.id.clone(), item.clone(), pin.clone())).cloned(),
                    Endpoint::Point { .. } => None,
                })
            });
            t.push_linked(
                vec![
                    page.number.clone(),
                    w.number.clone().unwrap_or_default(),
                    potential.unwrap_or_default(),
                    w.cable.as_ref().map(cable_core_label).unwrap_or_default(),
                    end(&w.from),
                    end(&w.to),
                    w.spec.section.clone().unwrap_or_default(),
                    w.spec.colour.clone().unwrap_or_default(),
                    w.spec.kind.clone().unwrap_or_default(),
                ],
                &page.id,
            );
        }
    }
    t
}

fn cable_core_label(c: &wc_core::CableCore) -> String {
    if c.core.is_empty() {
        c.cable.to_string()
    } else {
        format!("{}:{}", c.cable, c.core)
    }
}

/// Cable list: every cable with its type, then one row per core with the wire's two ends. Each
/// row links to the page its core is drawn on ([`Table::row_pages`]).
pub fn cable_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new(
        "Cable list",
        &["Cable", "Type", "Description", "Core", "Page", "From", "To"],
    );
    /// A core's row: core, page number, from, to, and the page id its row links to.
    type Core = (String, String, String, String, String);
    let mut cores: BTreeMap<DeviceTag, Vec<Core>> = BTreeMap::new();
    for page in &i.project.pages {
        for w in &page.wires {
            let Some(c) = &w.cable else { continue };
            let end = |e: &Endpoint| match e {
                Endpoint::Pin { item, pin } => pin_label(i.project, &page.id, item, pin),
                Endpoint::Point { at } => format!("junction {:.1},{:.1}", at.x, at.y),
            };
            cores.entry(c.cable.clone()).or_default().push((
                c.core.clone(),
                page.number.clone(),
                end(&w.from),
                end(&w.to),
                page.id.clone(),
            ));
        }
    }
    for (cable, mut list) in cores {
        list.sort_by_key(|a| natural_key(&a.0));
        let device = i.project.devices.get(&cable);
        let part = device.and_then(|d| d.part.as_ref()).and_then(|id| i.parts.get(id));
        let kind = part
            .map(|p| {
                [p.manufacturer.as_str(), p.order_number.as_str()]
                    .join(" ")
                    .trim()
                    .to_owned()
            })
            .unwrap_or_default();
        let description = device
            .map(|d| d.description.clone())
            .filter(|d| !d.is_empty())
            .or_else(|| part.map(|p| p.description.clone()))
            .unwrap_or_default();
        for (n, (core, page, from, to, page_id)) in list.into_iter().enumerate() {
            let first = n == 0;
            t.push_linked(
                vec![
                    if first { cable.to_string() } else { String::new() },
                    if first { kind.clone() } else { String::new() },
                    if first { description.clone() } else { String::new() },
                    core,
                    page,
                    from,
                    to,
                ],
                &page_id,
            );
        }
    }
    t
}

/// Net list: each electrically connected group of pins, across pages.
pub fn net_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new("Net list", &["Net", "Pins", "Connections"]);
    let (nets, _) = net_names(i);
    let mut rows: Vec<(String, Vec<String>)> = nets
        .iter()
        .enumerate()
        .map(|(n, net)| {
            let name = if net.names.is_empty() {
                format!("N{}", n + 1)
            } else {
                net.names.join(" / ")
            };
            let mut pins: Vec<String> = net
                .pins
                .iter()
                .map(|(pg, item, pin)| pin_label(i.project, pg, item, pin))
                .collect();
            pins.sort_by_key(|s| natural_key(s));
            (name, pins)
        })
        .collect();
    rows.sort_by_key(|(name, _)| natural_key(name));
    for (name, pins) in rows {
        t.rows.push(vec![name, pins.len().to_string(), pins.join(", ")]);
    }
    t
}

/// The device property that makes a terminal one level of a multi-level (double- or triple-deck)
/// terminal: `1` is the bottom level, `2` the next one up. A terminal with level 2 or more sits on
/// the same terminal block as the terminal before it in the strip, so `-X1.1` (level 1) and
/// `-X1.2` (level 2) are one two-level terminal.
pub const LEVEL_PROPERTY: &str = "Level";

/// A [`LEVEL_PROPERTY`] value as a level, or why it is not one.
pub fn parse_level(value: &str) -> Result<u32, String> {
    match value.trim().parse::<u32>() {
        Ok(0) => Err("levels start at 1".into()),
        Ok(l) => Ok(l),
        Err(e) => Err(format!("not a whole number ({e})")),
    }
}

/// A terminal's [`LEVEL_PROPERTY`] value, `None` when it has none.
fn level_value<'p>(project: &'p wc_core::Project, strip: &str, number: &str) -> Option<&'p str> {
    let tag = DeviceTag::parse(&format!("{strip}.{number}")).ok()?;
    project
        .devices
        .get(&tag)?
        .properties
        .get(LEVEL_PROPERTY)
        .map(String::as_str)
}

/// A terminal's level from its [`LEVEL_PROPERTY`]; `None` when it has none or the value is not a
/// level ([`level_warnings`] says which and why).
fn terminal_level(project: &wc_core::Project, strip: &str, number: &str) -> Option<u32> {
    parse_level(level_value(project, strip, number)?).ok()
}

/// One warning for each device whose [`LEVEL_PROPERTY`] is set but is not a level, naming the
/// device, the value and the reason. Such a terminal counts as single-level in the terminal plan
/// and the marking export.
pub fn level_warnings(project: &wc_core::Project) -> Vec<String> {
    project
        .devices
        .iter()
        .filter_map(|(tag, dev)| {
            let value = dev.properties.get(LEVEL_PROPERTY)?;
            let reason = parse_level(value).err()?;
            Some(format!(
                "{tag}: {LEVEL_PROPERTY} {value:?} is ignored: {reason}; the terminal counts as single-level"
            ))
        })
        .collect()
}

/// The level of each terminal of `strip` that has one: its own valid [`LEVEL_PROPERTY`], or 1
/// for a terminal with no `Level` of its own right below one at level 2 or more. `drawn` is the
/// strip's drawn terminals in the usual (number) order, each once.
///
/// Levels are inferred in that order, so moving terminals on the rail never changes a terminal's
/// level or which block it is in. The strip's spares count only when they have a valid level of
/// their own, so adding a spare without one never splits a block; a spare with a valid level
/// joins the number order and can change the blocks (and their markers) like a drawn terminal
/// with that level would. This is what the terminal plan's Level column
/// shows; the terminal strip editor shows it too.
pub fn strip_levels(project: &wc_core::Project, strip: &str, drawn: Vec<String>) -> BTreeMap<String, u32> {
    let mut usual = drawn;
    let layout = project.meta.strips.iter().find(|l| l.strip.to_string() == strip);
    for spare in layout.iter().flat_map(|l| &l.spares) {
        // Only a valid level of its own counts; a bad value is reported by level_warnings.
        if usual.contains(spare) || terminal_level(project, strip, spare).is_none() {
            continue;
        }
        let key = (natural_key(spare), spare.clone());
        let at = usual.partition_point(|n| (natural_key(n), n.clone()) < key);
        usual.insert(at, spare.clone());
    }
    let mut by_number = BTreeMap::new();
    for (k, n) in usual.iter().enumerate() {
        let own = terminal_level(project, strip, n);
        // The bottom level of a block needs no property of its own: the level above says so.
        // A bad value is not missing: that terminal stays single-level, as its warning says.
        let missing = level_value(project, strip, n).is_none();
        let below = || {
            usual
                .get(k + 1)
                .and_then(|up| terminal_level(project, strip, up))
                .filter(|l| missing && *l >= 2)
                .map(|_| 1)
        };
        if let Some(l) = own.or_else(below) {
            by_number.insert(n.clone(), l);
        }
    }
    by_number
}

/// What the terminal plan's Page column says for a spare terminal, which no page draws.
pub const SPARE_PAGE: &str = "spare";

/// Terminal plan: for every terminal strip, each terminal with what connects to its two sides.
///
/// The last column is the terminal's level on a multi-level terminal (see [`LEVEL_PROPERTY`]):
/// empty for a single-level terminal, and `1` for the bottom level of a block even when only the
/// levels above it say so.
///
/// A terminal is a symbol with the terminal role. Its strip and number come from the tag:
/// `-X1.3` is terminal 3 of strip `-X1`. Terminals tagged with just the strip (`-X1`) are
/// numbered in drawing order (page, then left to right). Terminals come by number, unless the
/// terminal strip editor put the strip in another order ([`wc_core::project::StripLayout`]);
/// its spare terminals are listed with [`SPARE_PAGE`] in the Page column.
///
/// Each drawn terminal's row links to its page ([`Table::row_pages`]); a spare's links nowhere.
pub fn terminal_plan(i: &Inputs<'_>) -> Table {
    terminal_plan_with_spares(i).0
}

/// Whether a placed symbol is a terminal in the terminal plan and the exports: a symbol of
/// known role is one when it is a terminal symbol; a symbol the project has no copy of is one
/// when its tag has a terminal level (`-X1.3`), as in the strip editor and jumpers.
pub(crate) fn counts_as_terminal(role: Option<SymbolRole>, tag: Option<&DeviceTag>) -> bool {
    match role {
        Some(r) => r == SymbolRole::Terminal,
        None => tag.is_some_and(|t| wc_core::project::is_terminal(t, None)),
    }
}

/// [`terminal_plan`], with which of its rows are spare terminals (by row index): a spare is
/// known by where it comes from, not by what its Page cell says.
pub(crate) fn terminal_plan_with_spares(i: &Inputs<'_>) -> (Table, BTreeSet<usize>) {
    let mut spare_rows: BTreeSet<usize> = BTreeSet::new();
    let mut t = Table::new(
        "Terminal plan",
        &[
            "Strip",
            "Terminal",
            "Upper (pin 1)",
            "Lower (pin 2)",
            "Potential",
            "Page",
            "Jumpered with",
            "Level",
        ],
    );
    // The potential column sees jumpers; the Upper and Lower columns show what is wired to that
    // side of this terminal itself, so they come from the nets without jumpers.
    let (_, net_of) = net_names(i);
    let mut unjumpered = i.project.clone();
    unjumpered.meta.jumpers.clear();
    let (nets, _) = net_names(&Inputs {
        project: &unjumpered,
        symbols: i.symbols,
        parts: i.parts,
    });
    // Pins connected to a pin, excluding the pin itself.
    let others = |pin: &PinRef| -> Vec<String> {
        nets.iter()
            .find(|n| n.pins.contains(pin))
            .map(|n| {
                let mut v: Vec<String> = n
                    .pins
                    .iter()
                    .filter(|p| *p != pin)
                    .map(|(pg, it, pn)| pin_label(i.project, pg, it, pn))
                    .collect();
                v.sort_by_key(|s| natural_key(s));
                v
            })
            .unwrap_or_default()
    };
    struct Term {
        strip: String,
        number: Option<String>,
        page: String,
        page_id: String,
        item: String,
        pins: Vec<String>,
        x: f64,
        /// The other terminals of its jumper, e.g. `3, 5`.
        jumpered: String,
        /// Its symbol, when the project has no copy of it: what it connects is not known.
        missing: Option<String>,
    }
    let mut terms: Vec<Term> = Vec::new();
    // The numbers jumpers refer to: a terminal tagged with only its strip gets the next one.
    let numbers = wc_core::project::terminal_numbers(i.project, |id| i.symbols.get(id).map(|s| s.role));
    // An overview (line diagram) draws terminals again; the plan lists each one once.
    for page in i.project.pages.iter().filter(|pg| !pg.overview) {
        for s in &page.symbols {
            // One rule with the strip editor and jumpers (terminal_numbers): a symbol the project
            // has no copy of is a terminal when its tag has a terminal level (`-X1.3`). It is
            // listed, marked, so the plan and the exports have the same terminals.
            let sym = i.symbols.get(&s.symbol);
            if !counts_as_terminal(sym.map(|y| y.role), s.tag.as_ref()) {
                continue;
            }
            let (strip, number) = match &s.tag {
                Some(tag) => {
                    let (strip, number) = tag.terminal_parts();
                    (strip.to_string(), number)
                }
                None => ("(untagged)".to_owned(), None),
            };
            let assigned = numbers.get(&(page.id.clone(), s.id.clone()));
            let number = assigned.map(|(_, n)| n.clone()).or(number);
            let jumpered = assigned
                .and_then(|(strip, n)| {
                    let j = i
                        .project
                        .meta
                        .jumpers
                        .iter()
                        .find(|j| j.strip == *strip && j.terminals.contains(n))?;
                    Some(
                        j.terminals
                            .iter()
                            .filter(|t| *t != n)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", "),
                    )
                })
                .unwrap_or_default();
            terms.push(Term {
                strip,
                number,
                jumpered,
                page: page.number.clone(),
                page_id: page.id.clone(),
                item: s.id.clone(),
                pins: sym
                    .map(|sym| sym.pins.iter().map(|p| p.name.clone()).collect())
                    .unwrap_or_default(),
                x: s.at.x,
                missing: sym.is_none().then(|| s.symbol.clone()),
            });
        }
    }
    let mut strips: BTreeMap<String, Vec<Term>> = BTreeMap::new();
    for term in terms {
        strips.entry(term.strip.clone()).or_default().push(term);
    }
    // A strip arranged in the terminal strip editor is listed even when only spares are on it.
    for l in &i.project.meta.strips {
        strips.entry(l.strip.to_string()).or_default();
    }
    for (strip, mut list) in strips {
        // Numbered ones by number; unnumbered ones keep drawing order and are numbered after.
        let page_order = |id: &str| i.project.pages.iter().position(|p| p.id == id).unwrap_or(usize::MAX);
        list.sort_by(|a, b| {
            let ka = a.number.as_deref().map(natural_key);
            let kb = b.number.as_deref().map(natural_key);
            match (ka, kb) {
                // `1` and `01` have equal keys: order them exactly too, as the marking export does.
                (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.number.cmp(&b.number)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => page_order(&a.page_id)
                    .cmp(&page_order(&b.page_id))
                    .then(a.x.total_cmp(&b.x)),
            }
        });
        // As wc_core::project::terminal_numbers: a spare's number is taken.
        let spares = i
            .project
            .meta
            .strips
            .iter()
            .find(|l| l.strip.to_string() == strip)
            .map(|l| l.spares.as_slice())
            .unwrap_or(&[]);
        let mut next = list
            .iter()
            .filter_map(|t| t.number.as_deref()?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        let numbered: Vec<(Term, String)> = list
            .into_iter()
            .map(|term| {
                let number = term
                    .number
                    .clone()
                    .or_else(|| wc_core::project::next_free_number(&mut next, spares))
                    .unwrap_or_default();
                (term, number)
            })
            .collect();
        // The rail order and spares the terminal strip editor set: a terminal drawn twice
        // stays together, a spare gets a row of its own with nothing connected.
        let layout = i.project.meta.strips.iter().find(|l| l.strip.to_string() == strip);
        let mut usual: Vec<String> = numbered.iter().map(|(_, n)| n.clone()).collect();
        usual.dedup();
        let by_number = strip_levels(i.project, &strip, usual);
        let numbered: Vec<(Option<Term>, String)> = match layout {
            None => numbered.into_iter().map(|(t, n)| (Some(t), n)).collect(),
            Some(layout) => {
                let mut drawn: Vec<String> = numbered.iter().map(|(_, n)| n.clone()).collect();
                drawn.dedup();
                let mut rest: Vec<Option<(Term, String)>> = numbered.into_iter().map(Some).collect();
                let mut out = Vec::new();
                for (n, spare) in layout.arrange(&drawn) {
                    if spare {
                        out.push((None, n));
                        continue;
                    }
                    for slot in rest.iter_mut() {
                        if slot.as_ref().is_some_and(|(_, m)| *m == n)
                            && let Some((t, m)) = slot.take()
                        {
                            out.push((Some(t), m));
                        }
                    }
                }
                out
            }
        };
        let levels: Vec<Option<u32>> = numbered
            .iter()
            .map(|(_, n)| by_number.get(n.as_str()).copied())
            .collect();
        for ((term, number), level) in numbered.into_iter().zip(levels) {
            let level = level.map(|l| l.to_string()).unwrap_or_default();
            let Some(term) = term else {
                // A spare: on the rail, not drawn. Its jumper can still be set.
                let jumpered = DeviceTag::parse(&strip)
                    .ok()
                    .and_then(|st| {
                        i.project
                            .meta
                            .jumpers
                            .iter()
                            .find(|j| j.strip == st && j.terminals.contains(&number))
                    })
                    .map(|j| {
                        j.terminals
                            .iter()
                            .filter(|t| **t != number)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                spare_rows.insert(t.rows.len());
                t.rows.push(vec![
                    strip.clone(),
                    number,
                    String::new(),
                    String::new(),
                    String::new(),
                    SPARE_PAGE.to_owned(),
                    jumpered,
                    level,
                ]);
                continue;
            };
            let side = |n: usize| -> (String, Option<String>) {
                let Some(pin) = term.pins.get(n) else {
                    return (String::new(), None);
                };
                let r: PinRef = (term.page_id.clone(), term.item.clone(), pin.clone());
                (others(&r).join(", "), net_of.get(&r).cloned())
            };
            let (mut upper, pa) = side(0);
            let (lower, pb) = side(1);
            if let Some(symbol) = &term.missing {
                upper = format!("(symbol {symbol} missing)");
            }
            t.push_linked(
                vec![
                    strip.clone(),
                    number,
                    upper,
                    lower,
                    pa.or(pb).unwrap_or_default(),
                    term.page.clone(),
                    term.jumpered.clone(),
                    level,
                ],
                &term.page_id,
            );
        }
    }
    // A spare has no wiring of its own, but a jumper to a drawn terminal puts it on that
    // terminal's potential.
    for &k in &spare_rows {
        let cell = |r: &[String], c: usize| r.get(c).cloned().unwrap_or_default();
        let Some(row) = t.rows.get(k) else { continue };
        if !cell(row, 4).is_empty() {
            continue;
        }
        let (strip, number) = (cell(row, 0), cell(row, 1));
        let Some(jumper) = DeviceTag::parse(&strip).ok().and_then(|st| {
            i.project
                .meta
                .jumpers
                .iter()
                .find(|j| j.strip == st && j.terminals.contains(&number))
        }) else {
            continue;
        };
        let potential = t
            .rows
            .iter()
            .enumerate()
            .find(|(n, r)| {
                !spare_rows.contains(n)
                    && cell(r, 0) == strip
                    && jumper.terminals.contains(&cell(r, 1))
                    && !cell(r, 4).is_empty()
            })
            .map(|(_, r)| cell(r, 4));
        if let (Some(p), Some(slot)) = (potential, t.rows.get_mut(k).and_then(|r| r.get_mut(4))) {
            *slot = p;
        }
    }
    (t, spare_rows)
}

/// Table of contents: every schematic page with its title and what is drawn on it.
pub fn contents(i: &Inputs<'_>) -> Table {
    let mut t = Table::new("Table of contents", &["Page", "Title", "Devices", "Wires"]);
    t.front = true;
    for page in &i.project.pages {
        t.row_pages.push(page.id.clone());
        let tags: BTreeSet<String> = page
            .symbols
            .iter()
            .filter_map(|s| s.tag.as_ref().map(ToString::to_string))
            .collect();
        t.rows.push(vec![
            page.number.clone(),
            page.title.clone(),
            tags.len().to_string(),
            page.wires.len().to_string(),
        ]);
    }
    t
}

/// Whether a symbol is a PLC input or output, from its library category and name or pin.
fn plc_kind(sym: &Symbol) -> Option<&'static str> {
    let category = sym.category.to_ascii_lowercase();
    let id = sym.id.to_ascii_lowercase();
    if category != "plc" && !id.contains(".plc.") {
        return None;
    }
    let name = format!("{} {}", sym.name, sym.id).to_ascii_lowercase();
    let pins: Vec<&str> = sym.pins.iter().map(|p| p.name.as_str()).collect();
    if name.contains("input") || pins == ["I"] {
        Some("Input")
    } else if name.contains("output") || pins == ["Q"] {
        Some("Output")
    } else {
        None
    }
}

/// One PLC input or output drawn on a page, as the PLC I/O list shows it.
#[derive(Debug, Clone)]
pub(crate) struct PlcPoint {
    /// The device property `Address` (or `PLC address`), trimmed; empty when there is none.
    pub address: String,
    /// `Input` or `Output`.
    pub kind: &'static str,
    /// The device tag, or `<item id> (no tag)`.
    pub device: String,
    pub description: String,
    /// What it is wired to.
    pub connected: String,
    /// The number and the id of the page it is drawn on.
    pub page_number: String,
    pub page_id: String,
}

impl PlcPoint {
    /// Its row in the PLC I/O list.
    fn row(&self) -> Vec<String> {
        vec![
            self.address.clone(),
            self.kind.to_owned(),
            self.device.clone(),
            self.description.clone(),
            self.connected.clone(),
            self.page_number.clone(),
        ]
    }
}

/// PLC I/O list: every PLC input and output drawn, with its address, the device it belongs to
/// and what it is wired to. The address is the device property `Address` (or `PLC address`);
/// points without one sort last so the gaps are visible. Each row links to its page
/// ([`Table::row_pages`]).
pub fn plc_io_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new(
        "PLC I/O list",
        &["Address", "Type", "Device", "Description", "Connected to", "Page"],
    );
    for point in plc_points(i) {
        t.push_linked(point.row(), &point.page_id);
    }
    t
}

/// The drawn PLC inputs and outputs in the order of the PLC I/O list: by address, points without
/// one last.
pub(crate) fn plc_points(i: &Inputs<'_>) -> Vec<PlcPoint> {
    let (nets, _) = net_names(i);
    let mut net_of: BTreeMap<PinRef, usize> = BTreeMap::new();
    for (n, net) in nets.iter().enumerate() {
        for pin in &net.pins {
            net_of.insert(pin.clone(), n);
        }
    }
    // (no address, address sort key, point)
    type Row = (bool, Vec<(String, u64)>, PlcPoint);
    let mut rows: Vec<Row> = Vec::new();
    for page in &i.project.pages {
        for s in &page.symbols {
            let Some(sym) = i.symbols.get(&s.symbol) else { continue };
            let Some(kind) = plc_kind(sym) else { continue };
            let dev = s.tag.as_ref().and_then(|tag| i.project.devices.get(tag));
            let address = dev
                .and_then(|d| {
                    d.properties
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("address") || k.eq_ignore_ascii_case("plc address"))
                        .map(|(_, v)| v.trim().to_owned())
                })
                .unwrap_or_default();
            let mut others: BTreeSet<String> = BTreeSet::new();
            for p in &sym.pins {
                let key = (page.id.clone(), s.id.clone(), p.name.clone());
                let Some(n) = net_of.get(&key) else { continue };
                for other in nets.get(*n).map(|net| net.pins.as_slice()).unwrap_or(&[]) {
                    if *other != key {
                        let (pg, item, pin) = other;
                        others.insert(pin_label(i.project, pg, item, pin));
                    }
                }
            }
            let mut others: Vec<String> = others.into_iter().collect();
            others.sort_by_key(|o| natural_key(o));
            let point = PlcPoint {
                address: address.clone(),
                kind,
                device: s
                    .tag
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| format!("{} (no tag)", s.id)),
                description: dev.map(|d| d.description.clone()).unwrap_or_default(),
                connected: others.join(", "),
                page_number: page.number.clone(),
                page_id: page.id.clone(),
            };
            rows.push((address.is_empty(), natural_key(&address), point));
        }
    }
    rows.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    rows.into_iter().map(|(_, _, p)| p).collect()
}
