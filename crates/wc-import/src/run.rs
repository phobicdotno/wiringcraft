//! The imports into a project or a library folder, as the CLI's `wiringcraft import` and the
//! app's File > Import run them: read the file, add what it holds and save. What an import has
//! to say (counts, reports, warnings) goes to a [`Log`] in order, for the caller to print or
//! show.

use std::path::{Path, PathBuf};

use wc_core::lock::ItemKind;
use wc_core::{DeviceTag, Page, PageSize, Project, store};

/// The result of an import step: the reason as text when it fails.
pub type Res<T> = Result<T, String>;

fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

/// One message of an import, in the order it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// A result line (the CLI prints it on standard output).
    Out(String),
    /// A warning or note (the CLI prints it on standard error), with its `warning: ` prefix.
    Err(String),
}

/// What an import had to say, in order. A log made with [`Log::echoing`] also hands each line
/// on as it is added, so a long import shows what it has done so far (the CLI prints it).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Log(pub Vec<Msg>, Echo);

/// Where a log's lines go as they are added. Not part of what a log holds: a clone has none,
/// and logs compare by their lines only.
#[derive(Default)]
pub struct Echo(Option<EchoFn>);

/// What an [`Echo`] hands each line to.
type EchoFn = Box<dyn FnMut(&Msg) + Send>;

impl std::fmt::Debug for Echo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "Echo(on)" } else { "Echo(off)" })
    }
}

impl Clone for Echo {
    fn clone(&self) -> Self {
        Echo(None)
    }
}

impl PartialEq for Echo {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Echo {}

impl Log {
    /// A log that also gives each line to `echo` as it is added.
    pub fn echoing(echo: impl FnMut(&Msg) + Send + 'static) -> Self {
        Log(Vec::new(), Echo(Some(Box::new(echo))))
    }

    fn push(&mut self, m: Msg) {
        if let Some(echo) = self.1.0.as_mut() {
            echo(&m);
        }
        self.0.push(m);
    }

    /// Add a result line.
    pub fn out(&mut self, line: String) {
        self.push(Msg::Out(line));
    }

    /// Add a warning or note line.
    pub fn err(&mut self, line: String) {
        self.push(Msg::Err(line));
    }

    /// Every line, in order, as one text.
    pub fn text(&self) -> String {
        let lines: Vec<&str> = self
            .0
            .iter()
            .map(|m| match m {
                Msg::Out(s) | Msg::Err(s) => s.as_str(),
            })
            .collect();
        lines.join("\n")
    }

    /// The warning and note lines.
    pub fn warnings(&self) -> impl Iterator<Item = &str> {
        self.0.iter().filter_map(|m| match m {
            Msg::Err(s) => Some(s.as_str()),
            Msg::Out(_) => None,
        })
    }
}

macro_rules! out {
    ($log:expr, $($t:tt)*) => { $log.out(format!($($t)*)) };
}

macro_rules! err {
    ($log:expr, $($t:tt)*) => { $log.err(format!($($t)*)) };
}

/// PLC addresses from a CSV list onto the project's devices (`import plc-csv`).
pub fn plc_csv(dir: &Path, file: &Path, log: &mut Log) -> Res<()> {
    let bytes = read_file(file)?;
    plc_csv_bytes(dir, &bytes, log)
}

/// [`plc_csv`] from the file's bytes, read already (a command line reads them before it takes
/// the project's lock).
pub fn plc_csv_bytes(dir: &Path, bytes: &[u8], log: &mut Log) -> Res<()> {
    let mut p = load(dir)?;
    let (rows, warnings) = crate::csv::file_rows(bytes)
        .and_then(|rows| crate::csv::plc_addresses_from_rows(&rows))
        .map_err(e)?;
    for w in warnings {
        err!(log, "warning: {}", escape_field(&w));
    }
    let applied = crate::csv::apply_plc_addresses(&mut p, &rows);
    for t in &applied.unknown {
        err!(log, "warning: no symbol carries {t}, skipped");
    }
    if applied.changed > 0 {
        save(dir, &p)?;
    }
    out!(
        log,
        "PLC addresses: {} device(s) changed, {} already up to date, {} unknown",
        applied.changed,
        applied.unchanged,
        applied.unknown.len()
    );
    Ok(())
}

/// Parts from a BMEcat catalogue into the library folder `lib` (`import parts-bmecat`). A
/// catalogue that gives no part is an error.
pub fn parts_bmecat(file: &Path, lib: &Path, log: &mut Log) -> Res<()> {
    let (n, warnings) = import_bmecat(file, lib)?;
    for w in warnings {
        err!(log, "warning: {}", escape_field(&w));
    }
    if n == 0 {
        return Err(NO_PART.into());
    }
    out!(log, "imported {n} part(s) into {}", lib.display());
    Ok(())
}

/// Why `import parts-csv` and `import parts-bmecat` fail on a file that gives no part.
const NO_PART: &str = "no part could be imported from the file";

/// Parts from an EDZ archive into the library folder `lib` (`import parts-edz`).
pub fn parts_edz(file: &Path, lib: &Path, symbol_map: Option<&Path>, log: &mut Log) -> Res<()> {
    let (n, report, warnings) = import_edz(file, lib, symbol_map)?;
    for w in warnings {
        err!(log, "warning: {}", escape_field(&w));
    }
    for line in report.lines() {
        out!(log, "{line}");
    }
    out!(log, "imported {n} part(s) into {}", lib.display());
    Ok(())
}

/// Parts from a CSV list into the library folder `lib` (`import parts-csv`). A list that gives
/// no part is an error.
pub fn parts_csv(file: &Path, lib: &Path, log: &mut Log) -> Res<()> {
    let bytes = read_file(file)?;
    let (parts, warnings) = crate::csv::file_rows(&bytes)
        .and_then(|rows| crate::csv::parts_from_rows(&rows))
        .map_err(e)?;
    for p in &parts {
        wc_library::write_part(lib, p).map_err(e)?;
    }
    for w in warnings {
        err!(log, "warning: {}", escape_field(&w));
    }
    if parts.is_empty() {
        return Err(NO_PART.into());
    }
    out!(log, "imported {} part(s) into {}", parts.len(), lib.display());
    Ok(())
}

/// The highest number an import may give its first new page: far above any real page number,
/// and low enough that numbering the pages after it never runs past the largest number.
pub const MAX_FIRST_PAGE: usize = 1_000_000_000;

/// Refuse a first page number above [`MAX_FIRST_PAGE`], before anything is read or written.
fn first_page_in_range(first_page: usize) -> Res<()> {
    if first_page > MAX_FIRST_PAGE {
        return Err(format!(
            "the first page number {first_page} is too large: {MAX_FIRST_PAGE} at most"
        ));
    }
    Ok(())
}

/// The page number `by` after `number`, or why there is none: an archive counts its entries'
/// pages on from the first page number, past [`MAX_FIRST_PAGE`] when it starts near it.
fn page_after(number: usize, by: usize) -> Res<usize> {
    number
        .checked_add(by)
        .ok_or_else(|| format!("the page numbers after {number} run past the largest number"))
}

/// PDF pages as trace pages numbered from `first_page` (`import pdf`).
pub fn pdf(dir: &Path, file: &Path, first_page: usize, log: &mut Log) -> Res<()> {
    first_page_in_range(first_page)?;
    let source = file
        .file_name()
        .map(|n| escape_field(&n.to_string_lossy()))
        .unwrap_or_default();
    import_pdf(dir, file, first_page, &source, log)?;
    Ok(())
}

/// An Eplan Electric P8 project (.elk, .edb folder or zip) into the project (`import eplan`).
pub fn eplan(dir: &Path, path: &Path, first_page: usize, log: &mut Log) -> Res<()> {
    first_page_in_range(first_page)?;
    let mut p = load(dir)?;
    let (tables, read_warnings) = eplan_tables(path)?;
    for w in &read_warnings {
        err!(log, "warning: {}", escape_field(w));
    }
    let ep = crate::eplan::read(&tables).map_err(e)?;
    let (built, warnings) = crate::eplan::build(&ep, first_page);
    for w in ep.warnings.iter().chain(&warnings) {
        err!(log, "warning: {}", escape_field(w));
    }
    let source = format!(
        "import:{}",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    // Customer and project description fill empty title block fields.
    for (id, key) in [(10015u32, "customer"), (10011, "description")] {
        if let Some((_, _, v)) = ep.properties.iter().find(|(i, _, _)| *i == id)
            && !v.is_empty()
        {
            p.meta.title_block.entry(key.to_owned()).or_insert_with(|| v.clone());
        }
    }
    let (n_dev, n_pages) = (built.devices.len(), built.pages.len());
    let n_drawn = built.pages.iter().filter(|pg| pg.id.starts_with("eplan-page-")).count();
    add_and_save(dir, &mut p, built, &source, Replace::No)?;
    out!(
        log,
        "imported {} Eplan pages as {n_pages} page(s), {n_drawn} of them as drawn: {n_dev} devices, {} \
         connections",
        ep.pages.len(),
        ep.connections.len()
    );
    Ok(())
}

/// A SolidWorks Electrical project archive or database into the project (`import swe`).
pub fn swe(
    dir: &Path,
    path: &Path,
    pick: usize,
    first_page: Option<usize>,
    symbol_map: Option<&Path>,
    log: &mut Log,
) -> Res<()> {
    let project = pick;
    if let Some(n) = first_page {
        first_page_in_range(n)?;
    }
    let started = std::time::Instant::now();
    let mut p = load(dir)?;
    // A database file directly, or an archive read from disk: only the picked project's
    // database is unpacked, so a large archive costs little memory.
    let mut head = [0u8; 16];
    let mut file = std::fs::File::open(path).map_err(e)?;
    let is_db = std::io::Read::read_exact(&mut file, &mut head).is_ok() && &head == b"SQLite format 3\0";
    let db = if is_db {
        // The same size cap as for a database taken out of an archive.
        let limit = crate::swe::MAX_DATABASE_BYTES;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(
            &mut std::io::Read::take(std::fs::File::open(path).map_err(e)?, limit + 1),
            &mut bytes,
        )
        .map_err(e)?;
        if bytes.len() as u64 > limit {
            return Err(format!("{} is larger than {} MB", path.display(), limit >> 20));
        }
        (bytes, None, None)
    } else {
        // One open file for the database, the catalog and the drawings, so they all come from
        // the same export even when the archive is replaced during the import.
        std::io::Seek::rewind(&mut file).map_err(e)?;
        let file = std::io::BufReader::new(&mut file);
        let got = crate::swe::database_from_archive(file, project).map_err(|x| format!("{x} (use --pick)"))?;
        if got.count > 1 {
            err!(
                log,
                "the archive holds {} projects; importing number {project} ({})",
                got.count,
                got.folder
            );
        }
        let info_warnings = got.info.iter().flat_map(|i| &i.warnings);
        for w in got.info_warning.iter().chain(info_warnings) {
            err!(log, "warning: {}", escape_field(w));
        }
        (got.bytes, got.info, Some(got.folder))
    };
    let (db, info, folder) = db;
    let conn = crate::swe::open(&db).map_err(e)?;
    let mut sw = crate::swe::read(&conn).map_err(e)?;
    sw.info = info;
    let choice = swe_symbol_choice(dir, &p, symbol_map)?;
    for w in &choice.2 {
        err!(log, "warning: {}", escape_field(w));
    }
    // The archive's catalog: devices that name one of its parts are linked to it, the part
    // comes along, and its size is used for layout footprints the source gives no box.
    let catalog_read = !is_db;
    if catalog_read {
        std::io::Seek::rewind(&mut file).map_err(e)?;
        let file = std::io::BufReader::new(&mut file);
        match crate::swe::library::catalog_from_archive(file) {
            Ok(lib) => {
                // Why a catalog row was left out, so a device it would have matched is
                // not left unlinked without a reason.
                for w in &lib.warnings {
                    err!(log, "warning: catalog: {w}");
                }
                sw.catalog = lib.parts;
            }
            Err(x) => err!(log, "warning: devices keep their part numbers as text: {x}"),
        }
    }
    // The symbol and page drawings (DWG): symbols without one stay boxes.
    if let Some(folder) = &folder {
        std::io::Seek::rewind(&mut file).map_err(e)?;
        let file = std::io::BufReader::new(&mut file);
        match crate::swe::drawings_from_archive(file, folder) {
            Ok(drawings) => {
                for w in &drawings.warnings {
                    err!(log, "warning: drawings: {w}");
                }
                if !drawings.skipped.is_empty() {
                    let kinds: Vec<String> = drawings.skipped.iter().map(|(k, n)| format!("{n} {k}")).collect();
                    err!(log, "warning: drawings: not drawn: {}", kinds.join(", "));
                }
                sw.drawings = drawings;
            }
            Err(x) => err!(log, "warning: symbols are drawn as boxes: {x}"),
        }
    }
    let (built, warnings, report) = crate::swe::build_with(&sw, first_page, &choice.0);
    for w in sw.warnings.iter().chain(&warnings) {
        err!(log, "warning: {}", escape_field(w));
    }
    let linked =
        (catalog_read && !sw.catalog.is_empty()).then(|| built.devices.values().filter(|d| d.part.is_some()).count());
    let source = format!(
        "import:{}",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    let (n_sym, joined) = add_swe_built(dir, &mut p, built, &source, &choice.1)?;
    out!(
        log,
        "{:<18} {:>8} {:>9}  why the rest is not imported",
        "",
        "source",
        "imported"
    );
    for r in &report {
        out!(log, "{:<18} {:>8} {:>9}  {}", r.what, r.source, r.imported, r.note);
    }
    if joined > 0 {
        out!(
            log,
            "{joined} imported jumper group(s) joined a jumper the project already had"
        );
    }
    if let Some(n) = linked {
        out!(
            log,
            "{n} device(s) and cable(s) linked to a part from the archive's catalog"
        );
    }
    out!(
        log,
        "{n_sym} symbol shape(s) for the boxes; done in {} ms",
        started.elapsed().as_millis()
    );
    Ok(())
}

/// The libraries of a SolidWorks Electrical archive, into `lib` or vendored into the project
/// (`import swe-library`).
pub fn swe_library(dir: &Path, path: &Path, lib: Option<&Path>, log: &mut Log) -> Res<()> {
    let source = format!(
        "import:{}",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    let (report, warnings) = import_swe_library(dir, path, lib, &source)?;
    for w in warnings {
        err!(log, "warning: {}", escape_field(&w));
    }
    for line in report.lines() {
        out!(log, "{line}");
    }
    match lib {
        Some(lib) => out!(log, "written to {}", lib.display()),
        None => out!(log, "vendored into the project"),
    }
    Ok(())
}

/// AutomationML devices and connections as overview pages (`import aml`).
pub fn aml(dir: &Path, file: &Path, first_page: usize, log: &mut Log) -> Res<()> {
    first_page_in_range(first_page)?;
    import_aml(dir, file, first_page, log)
}

/// [`aml`] without the limit on the first page number: an archive's later entries are
/// numbered on from the earlier ones, which may be past it.
fn import_aml(dir: &Path, file: &Path, first_page: usize, log: &mut Log) -> Res<()> {
    let mut p = load(dir)?;
    let text = read_file(file)?;
    let imp = crate::aml::parse(&String::from_utf8_lossy(&text)).map_err(e)?;
    for w in &imp.warnings {
        err!(log, "warning: {}", escape_field(w));
    }
    let built = crate::aml::build(&imp, first_page);
    let source = format!(
        "import:{}",
        file.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    // Through the shared preflight: conflicts are refused before anything is written.
    let (n_pages, n_dev) = (built.pages.len(), built.devices.len());
    add_and_save(dir, &mut p, built, &source, Replace::No)?;
    out!(
        log,
        "imported {n_dev} devices, {} connections on {n_pages} page(s)",
        imp.links.len()
    );
    Ok(())
}

/// The open-format parts of another tool's project archive (`import archive`).
pub fn archive(dir: &Path, file: &Path, first_page: usize, lib: Option<&Path>, log: &mut Log) -> Res<()> {
    use crate::archive::Kind;
    first_page_in_range(first_page)?;
    let bytes = read_file(file)?;
    // Old binary `.xls` workbooks are not read: named, so the user knows to save them as `.xlsx`.
    // Taken from the listing, which has no size limit, so a large one is named too.
    if let Ok(report) = crate::archive::inspect(&bytes) {
        for en in report.entries.iter().filter(|en| en.kind == Kind::OldSpreadsheet) {
            err!(
                log,
                "warning: {}: skipped, an old Excel workbook (.xls) is not read; save it as .xlsx",
                escape_field(&en.name)
            );
        }
    }
    let entries = crate::archive::open_entries(
        &bytes,
        &[
            Kind::Pdf,
            Kind::Dxf,
            Kind::AutomationMl,
            Kind::KicadSymbols,
            Kind::Csv,
            Kind::Spreadsheet,
        ],
    )
    .map_err(e)?;
    if entries.is_empty() {
        return Err(
            "no importable PDF, DXF, AutomationML, KiCad symbols, CSV lists or Excel lists inside; run `wiringcraft inspect` to see what is there"
                .into(),
        );
    }
    // Symbol libraries go into a library folder, not the project's pages.
    let (symbols, entries): (Vec<_>, Vec<_>) = entries
        .into_iter()
        .partition(|(_, kind, _)| *kind == Kind::KicadSymbols);
    // Entry names are printed: written so a name cannot send the terminal control
    // sequences. A library's prefix and a page's title are made from the name as it is.
    let entries: Vec<_> = entries
        .into_iter()
        .map(|(raw, kind, data)| ((escape_field(&raw), raw), kind, data))
        .collect();
    // CSV and Excel lists come last, so PLC addresses reach devices that pages in the same
    // archive add.
    let (csvs, entries): (Vec<_>, Vec<_>) = entries
        .into_iter()
        .partition(|(_, kind, _)| matches!(kind, Kind::Csv | Kind::Spreadsheet));
    // Two entries with the same file name would write the same ids over each other.
    let mut prefixes = std::collections::BTreeSet::new();
    for (raw, _, data) in symbols {
        let name = escape_field(&raw);
        out!(log, "{name}:");
        let Some(lib) = &lib else {
            err!(log, "warning: {name}: KiCad symbols, not imported: pass --lib <folder>");
            continue;
        };
        let prefix = kicad_prefix(&raw);
        if prefixes.contains(&prefix) {
            err!(
                log,
                "warning: {name}: skipped, an earlier entry already imports as {prefix}"
            );
            continue;
        }
        // Read first: an entry that cannot be read writes nothing and keeps its prefix free.
        let read = match read_kicad(&String::from_utf8_lossy(&data), &prefix, 1.0, log) {
            Ok(symbols) if symbols.is_empty() => {
                err!(log, "warning: {name}: no symbol could be imported");
                continue;
            }
            Ok(symbols) => symbols,
            Err(err) => {
                err!(log, "warning: {name}: {err}");
                continue;
            }
        };
        prefixes.insert(prefix);
        if let Err(err) = write_kicad(read, lib, "", log) {
            err!(log, "warning: {name}: {err}");
        }
    }
    if entries.is_empty() && csvs.is_empty() {
        return Ok(());
    }
    // Removed when it goes out of scope, also on an early return.
    let staging = staging_dir("wiringcraft-archive-").map_err(e)?;
    let tmp = staging.path();
    let mut next = first_page;
    for (i, ((name, raw), kind, data)) in entries.into_iter().enumerate() {
        let ext = match kind {
            Kind::Pdf => "pdf",
            Kind::Dxf => "dxf",
            _ => "aml",
        };
        let path = tmp.join(format!("{i}.{ext}"));
        std::fs::write(&path, &data).map_err(e)?;
        out!(log, "{name}:");
        let r = match kind {
            Kind::Pdf => {
                let n = crate::pdf::pages(&data).map(|p| p.len()).unwrap_or(0);
                let r = import_pdf(dir, &path, next, &name, log);
                next = page_after(next, n.max(1))?;
                r
            }
            Kind::Dxf => {
                let mut p = load(dir)?;
                let number = next;
                let id = format!("dxf-{number}");
                next = page_after(next, 1)?;
                // The number first: the refusal then names the page that holds it.
                if let Err(why) = number_free(&p, &number.to_string()) {
                    Err(why)
                // Page files are named by id, and some folders do not tell `DXF-300`
                // from `dxf-300`.
                } else if p.pages.iter().any(|pg| pg.id.eq_ignore_ascii_case(&id)) {
                    // Never added over an earlier page: removing it again on failure
                    // would delete that page.
                    Err(format!(
                        "page {id} exists; remove it or import with --first-page to number the pages from a free number"
                    ))
                } else {
                    p.pages.push(Page::new(id.clone(), number.to_string(), raw.clone()));
                    p.sort_pages();
                    // A save that fails part way is cleaned up like a failed import.
                    let r = save(dir, &p).and_then(|()| page_dxf(dir, &path, &id, 1.0, log));
                    // A drawing that could not be imported leaves no blank page behind.
                    // When that fails too, the entry's own error still counts and the
                    // other entries still come in.
                    if r.is_err() {
                        let remove = || -> Res<()> {
                            let mut p = load(dir)?;
                            p.pages.retain(|pg| pg.id != id);
                            save(dir, &p)
                        };
                        if let Err(why) = remove() {
                            // The save may fail after the page file is gone already.
                            let left = load(dir).map_or(true, |p| p.pages.iter().any(|pg| pg.id == id));
                            if left {
                                err!(log, "warning: {name}: the empty page {id} could not be removed: {why}");
                            } else {
                                err!(
                                    log,
                                    "warning: {name}: the empty page {id} was removed, but the project could not be saved in full: {why}"
                                );
                            }
                        }
                    }
                    r
                }
            }
            _ => {
                let r = import_aml(dir, &path, next, log);
                next = page_after(next, 10)?;
                r
            }
        };
        if let Err(err) = r {
            err!(log, "warning: {name}: {err}");
        }
    }
    for (i, ((name, _), _, data)) in csvs.into_iter().enumerate() {
        out!(log, "{name}:");
        // Told apart by header, the same column names `plc-csv` and `parts-csv` read.
        let rows = match crate::csv::file_rows(&data) {
            Ok(rows) => rows,
            Err(err) => {
                err!(log, "warning: {name}: {err}");
                continue;
            }
        };
        let ext = if crate::xlsx::is_xlsx(&data) { "xlsx" } else { "csv" };
        let path = tmp.join(format!("list-{i}.{ext}"));
        let cmd = match crate::csv::kind_of_rows(&rows) {
            Some(crate::csv::CsvKind::PlcAddresses) => None,
            Some(crate::csv::CsvKind::Parts) => match lib {
                Some(lib) => Some(lib),
                None => {
                    err!(log, "warning: {name}: parts list, not imported: pass --lib <folder>");
                    continue;
                }
            },
            None => {
                err!(
                    log,
                    "warning: {name}: not a parts list (order number column) or PLC address list (device and address columns), skipped"
                );
                continue;
            }
        };
        std::fs::write(&path, &data).map_err(e)?;
        let r = match cmd {
            None => plc_csv(dir, &path, log),
            Some(lib) => parts_csv(&path, lib, log),
        };
        if let Err(err) = r {
            err!(log, "warning: {name}: {err}");
        }
    }
    Ok(())
}

/// A DXF drawing onto the page `page` (`import page-dxf`).
pub fn page_dxf(dir: &Path, file: &Path, page: &str, scale: f64, log: &mut Log) -> Res<()> {
    let text = read_file(file)?;
    onto_page(dir, page, log, |known| {
        // A name from the file in the error cannot steer the terminal.
        crate::dxf::page(&String::from_utf8_lossy(&text), scale, known).map_err(|why| escape_field(&why.to_string()))
    })
}

/// Read a drawing with `read` (given the ids of the library's symbols) and add it to page
/// `page`: its graphics, and its symbols with the library symbols they need taken into the
/// project. Shared by `import page-dxf` and `import page-dwg`.
fn onto_page(
    dir: &Path,
    page: &str,
    log: &mut Log,
    read: impl FnOnce(&std::collections::BTreeSet<String>) -> Res<crate::dxf::ImportedPage>,
) -> Res<()> {
    let mut p = load(dir)?;
    let lib = wc_library::Library::open(dir, &p).map_err(e)?;
    let known: std::collections::BTreeSet<String> = lib.index(ItemKind::Symbol).into_keys().collect();
    let imported = read(&known)?;
    for w in &imported.warnings {
        err!(log, "warning: {}", escape_field(w));
    }
    // The page must be there before anything is taken (a retry would skip the copies). A
    // graphic page takes the drawings but no symbol.
    let target = page_mut(&mut p, page)?;
    if target.graphic && !imported.symbols.is_empty() {
        return Err(format!(
            "page {:?} is a graphic page, which holds drawings only, and the drawing has {} symbol(s); import it onto a schematic page",
            target.id,
            imported.symbols.len()
        ));
    }
    let needed: std::collections::BTreeSet<&str> = imported
        .symbols
        .iter()
        .map(|s| s.symbol.as_str())
        .filter(|id| store::read_vendored_symbol(dir, id).is_err())
        .collect();
    // Check every symbol before taking any, so a refused import leaves no copy behind
    // without its pin (a retry would take that copy for a pinned one).
    for id in &needed {
        let (s, source) = lib.symbol(id).map_err(e)?;
        if s.id != *id {
            return Err(format!("{source}: the symbol file for {id:?} declares id {:?}", s.id));
        }
    }
    // What is in the vendor folder and the project files now, so a copy or a save that fails
    // part way leaves the project folder as it was (a retry would skip a readable copy without
    // its pin).
    let project_files = snapshot_project(dir, &[], &[])?;
    let before: Written = needed
        .iter()
        .map(|id| vendored_bytes(dir, ItemKind::Symbol, id))
        .collect::<Res<_>>()?;
    let put_back = |why: String| -> String {
        let mut failed = put_files_back(&project_files);
        failed.extend(put_files_back(&before));
        if failed.is_empty() {
            why
        } else {
            format!("{why}; symbol copies could not be put back: {}", failed.join("; "))
        }
    };
    for id in needed {
        wc_library::take(dir, &mut p, &lib, ItemKind::Symbol, id).map_err(|x| put_back(x.to_string()))?;
    }
    let pg = page_mut(&mut p, page)?;
    let n_g = imported.graphics.len();
    let n_s = imported.symbols.len();
    pg.graphics.extend(imported.graphics);
    for mut s in imported.symbols {
        s.id = pg.next_id("s");
        pg.symbols.push(s);
    }
    save(dir, &p).map_err(put_back)?;
    out!(log, "imported {n_g} shapes and {n_s} symbols onto page {page}");
    Ok(())
}

/// A value as one tab-separated field: backslash, tab and line breaks written as `\\`, `\t`,
/// `\n` and `\r`, other control characters (such as ESC) as `\u{1b}`, so a multi-line text
/// stays one hit per line and a found value cannot send the terminal control sequences.
pub fn escape_field(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            // ESC and other control characters could steer the terminal.
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", u32::from(c))),
            c => out.push(c),
        }
    }
    out
}

pub fn page_mut<'a>(p: &'a mut Project, id: &str) -> Res<&'a mut Page> {
    p.page_mut(id).ok_or_else(|| format!("no page {id:?}"))
}

/// The project in `dir`; a folder without one says so with [`no_project`].
pub fn load(dir: &Path) -> Res<Project> {
    // The entry itself, not where a link leads: a broken link is reported as the load fails.
    if let Err(err) = std::fs::symlink_metadata(dir.join(store::PROJECT_FILE))
        && err.kind() == std::io::ErrorKind::NotFound
    {
        return Err(no_project(dir));
    }
    store::load(dir).map_err(e)
}

/// What a command says when its folder holds no project: where it looked, and how to get one.
pub fn no_project(dir: &Path) -> String {
    format!(
        "no WiringCraft project in {} (no {}): use -C <folder> or create one with wiringcraft new <folder>",
        escape_field(&dir.display().to_string()),
        store::PROJECT_FILE
    )
}

/// The bytes of the input file `path`; a failure names the file.
pub fn read_file(path: impl AsRef<Path>) -> Res<Vec<u8>> {
    let path = path.as_ref();
    std::fs::read(path).map_err(|err| format!("cannot read {}: {err}", escape_field(&path.display().to_string())))
}

/// The copies of the macro `m` a table places, one per row: `bytes` is the table (CSV or
/// `.xlsx`), each row's cells fill the placeholders and a `set` column picks the row's value
/// set (else `default_set`). `macro generate` and the app's Insert > Macro > From table both
/// prepare the table here.
///
/// `name` is what the messages call the table (its path); it is escaped with [`escape_field`]
/// here. A file cut short inside a quoted cell is refused: it would place a row with half a
/// value.
pub fn macro_table_copies(
    m: &wc_core::macros::Macro,
    bytes: &[u8],
    name: &str,
    default_set: Option<&str>,
) -> Res<Vec<wc_core::macros::Macro>> {
    macro_table(m, bytes, name, default_set, &[], |other| {
        Err(format!("no macro {other:?} to pick"))
    })
    .map(|(copies, _)| copies)
}

/// [`macro_table_copies`], with the page title each copy asks for (the table's `page`
/// column), as [`wc_core::generate::generate_titled`] takes them. A row of the table's
/// `macro` column places the macro `find` gives for its name instead of `m`, as does a row
/// that names none and matches one of `rules`.
pub fn macro_table(
    m: &wc_core::macros::Macro,
    bytes: &[u8],
    name: &str,
    default_set: Option<&str>,
    rules: &[wc_core::generate::MacroRule],
    mut find: impl FnMut(&str) -> Res<wc_core::macros::Macro>,
) -> Res<(Vec<wc_core::macros::Macro>, Vec<Option<String>>)> {
    let shown = escape_field(name);
    let rows = crate::csv::file_rows(bytes).map_err(|err| format!("{shown}: {err}"))?;
    if let Some(line) = rows.open_quote {
        return Err(format!(
            "{shown}: line {line}: a quoted cell is not closed before the end of the file; nothing was placed"
        ));
    }
    let rows = wc_core::generate::rows_from_table_ruled(m, &rows.cells, &rows.numbers, rules).map_err(e)?;
    let copies = wc_core::generate::apply_rows_with(m, &rows, default_set, |name| {
        find(name).map_err(wc_core::Error::Invalid)
    })
    .map_err(e)?;
    Ok((copies, wc_core::generate::row_pages(&rows)))
}

/// Save the project. Jumpers and strip layouts first follow their terminals: what the command
/// renamed, moved or deleted is found by comparing with the project as it is on disk.
pub fn save(dir: &Path, p: &Project) -> Res<()> {
    let mut p = p.clone();
    // A new project has nothing on disk to compare with; any other load failure is reported.
    let on_disk = dir.join(store::PROJECT_FILE).exists();
    if p.meta.follows_terminals() && on_disk {
        let old = store::load(dir).map_err(|err| format!("jumpers not updated: {}", e(err)))?;
        let (old_symbols, _) = store::vendored_symbols(dir, &old);
        let before = wc_core::project::terminal_numbers(&old, |id| old_symbols.get(id).map(|s| s.role));
        let (symbols, _) = store::vendored_symbols(dir, &p);
        wc_core::project::follow_terminals_in(&mut p, &before, |id| symbols.get(id).map(|s| s.role));
    }
    store::save(dir, &p).map(|_| ()).map_err(e)
}

/// The [`terminal_numbers`](wc_core::project::terminal_numbers) of `p` read with the symbols
/// `incoming` added to the project's own readable copies (a copy the project can read is kept).
///
/// A symbol the project has no readable copy of, which the command brings, changes which
/// placed items are terminals. Whether the jumpers' and rail order's numbers were given with
/// it or without it cannot be told, so a strip it renumbers is not guessed at: the command is
/// refused, before it writes anything. `what` names what brings the symbols ("import",
/// "macro") and `again` what to do once the file is back ("import again").
pub fn terminal_numbers_with(
    dir: &Path,
    p: &Project,
    incoming: &[wc_core::Symbol],
    what: &str,
    again: &str,
) -> Res<wc_core::project::TerminalNumbers> {
    terminal_numbers_after(dir, p, incoming, &[], what, again)
}

/// [`terminal_numbers_with`], for a command that also writes the symbols `replaced` over the
/// project's readable copies of them (an unpinned copy a macro takes from the library again).
///
/// The save that follows reads the copies as they are then, so it cannot tell that a replaced
/// symbol changed which placed items are terminals: a strip with jumpers or a rail order that
/// such a symbol renumbers is refused too, before anything is written.
pub fn terminal_numbers_after(
    dir: &Path,
    p: &Project,
    incoming: &[wc_core::Symbol],
    replaced: &[wc_core::Symbol],
    what: &str,
    again: &str,
) -> Res<wc_core::project::TerminalNumbers> {
    let (own, _) = store::vendored_symbols(dir, p);
    let mut symbols = own.clone();
    for s in incoming {
        symbols.entry(s.id.clone()).or_insert_with(|| s.clone());
    }
    for s in replaced {
        symbols.insert(s.id.clone(), s.clone());
    }
    let with = wc_core::project::terminal_numbers(p, |id| symbols.get(id).map(|s| s.role));
    let without = wc_core::project::terminal_numbers(p, |id| own.get(id).map(|s| s.role));
    let strips: std::collections::BTreeSet<&DeviceTag> = with
        .iter()
        .filter(|(k, v)| without.get(*k) != Some(*v))
        .map(|(_, (strip, _))| strip)
        .chain(
            without
                .iter()
                .filter(|(k, v)| with.get(*k) != Some(*v))
                .map(|(_, (strip, _))| strip),
        )
        .collect();
    if let Some(strip) = strips.into_iter().find(|s| {
        p.meta.jumpers.iter().any(|j| &j.strip == *s) || p.meta.strip_layout(s).is_some_and(|l| !l.is_empty())
    }) {
        let missing: Vec<&str> = incoming
            .iter()
            .filter(|s| !own.contains_key(&s.id))
            .map(|s| s.id.as_str())
            .filter(|id| p.pages.iter().any(|pg| pg.symbols.iter().any(|ps| ps.symbol == *id)))
            .collect();
        // Replaced symbols that are a terminal in one version and not in the other.
        let terminal = |s: &wc_core::Symbol| s.role == wc_core::symbol::SymbolRole::Terminal;
        let changed: Vec<&str> = replaced
            .iter()
            .filter(|s| own.get(&s.id).is_some_and(|old| terminal(old) != terminal(s)))
            .map(|s| s.id.as_str())
            .collect();
        if missing.is_empty() && !changed.is_empty() {
            return Err(format!(
                "the jumpers and rail order of {strip} cannot be kept on their terminals: the {what} replaces the project's copy of {} with the library's version, which changes which items are terminals; remove the strip's jumpers and rail order, {again} and add them back",
                changed.join(", ")
            ));
        }
        return Err(format!(
            "the jumpers and rail order of {strip} cannot be kept on their terminals: the project has no readable copy of {} that the {what} brings, so its terminal numbers are unclear; restore the file (for example from the project's history) and {again}",
            missing.join(", ")
        ));
    }
    Ok(with)
}

/// Save a project whose jumpers already follow their terminals: after `add_built`, which
/// moves the project's own jumpers itself and adds the imported ones with their final numbers.
pub fn save_imported(dir: &Path, p: &Project) -> Res<()> {
    store::save(dir, p).map(|_| ()).map_err(e)
}

/// Refuses a page number that a page of `p` already has, naming that page: two pages with one
/// number make cross-references to it ambiguous.
fn number_free(p: &Project, number: &str) -> Res<()> {
    let number = number.trim();
    match p.pages.iter().find(|pg| pg.number.trim() == number) {
        Some(pg) => Err(format!(
            "page number {} is taken by page {}; import with --first-page to number the pages from a free number",
            escape_field(number),
            escape_field(&pg.id)
        )),
        None => Ok(()),
    }
}

/// `import pdf`: the pages of `file` as trace pages numbered from `first_page`. `source` names
/// the PDF in warnings (the file name, or the entry name inside an archive, where `file` is a
/// staging file), already escaped with [`escape_field`] by the caller.
///
/// The limit on a first page number ([`MAX_FIRST_PAGE`]) is the caller's to check, on the
/// number the user gave: an archive numbers a later entry on from the earlier ones, which may
/// be past it. Here only a number the pages cannot be counted on from is refused.
pub fn import_pdf(dir: &Path, file: &Path, first_page: usize, source: &str, log: &mut Log) -> Result<(), String> {
    let mut p = load(dir)?;
    let bytes = read_file(file)?;
    let traced = crate::pdf::pages(&bytes).map_err(e)?;
    // Checked once, before anything is written: every `first_page + i` below is in range.
    page_after(first_page, traced.len())?;
    // Refused before anything is written, the underlay files of scanned pages too.
    for i in 0..traced.len() {
        number_free(&p, &(first_page + i).to_string())?;
    }
    // Every page id too, and every picture a scanned page would be saved over: a later page's
    // refusal must not come after an earlier page's scan replaced a picture (#1583).
    for (i, t) in traced.iter().enumerate() {
        let id = format!("pdf-{}", first_page + i);
        // Page files are named by id, and the save refuses ids that differ only in case.
        if let Some(pg) = p.pages.iter().find(|pg| pg.id.eq_ignore_ascii_case(&id)) {
            return Err(format!(
                "page {} exists; pick another --first-page",
                escape_field(&pg.id)
            ));
        }
        let Some(scan) = &t.scan else { continue };
        let file = format!("{}/{id}.{}", wc_core::Underlay::DIR, scan.raster.format.extension());
        // Some folders do not tell `PDF-5.png` from `pdf-5.png`, and `underlays/./pdf-5.png` or
        // `underlays\pdf-5.png` in a project file names the same picture.
        let parts = |f: &str| -> Vec<String> {
            f.split(['/', '\\'])
                .filter(|s| !s.is_empty() && *s != ".")
                .map(str::to_ascii_lowercase)
                .collect()
        };
        let uses = |pg: &&Page| pg.underlay.as_ref().is_some_and(|u| parts(&u.file) == parts(&file));
        if let Some(pg) = p.pages.iter().find(uses) {
            return Err(format!(
                "the scanned sheet of page {} would replace {file}, the picture under page {}; pick another --first-page",
                first_page + i,
                escape_field(&pg.id)
            ));
        }
    }
    let stem = file
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // What a failure after the first write puts back (#1751): every file the save may write, and
    // each scan's file with what it holds now (a picture no page uses, or nothing). Read before
    // anything is written, so a path that cannot be read is a refusal too.
    let ids: Vec<String> = (0..traced.len()).map(|i| format!("pdf-{}", first_page + i)).collect();
    let project_files = snapshot_project(dir, &ids.iter().map(String::as_str).collect::<Vec<_>>(), &[])?;
    let underlays = dir.join(wc_core::Underlay::DIR);
    let mut scans: Written = Vec::new();
    for (id, t) in ids.iter().zip(&traced) {
        let Some(scan) = &t.scan else { continue };
        let path = underlays.join(format!("{id}.{}", scan.raster.format.extension()));
        let shown = || escape_field(&path.display().to_string());
        // The path itself, not what a link there points at: putting a link back as it was is
        // not something `put_files_back` does (a dangling one reads as no file and would be
        // removed), so anything but a regular file or nothing is refused.
        let before = match std::fs::symlink_metadata(&path) {
            Err(x) if x.kind() == std::io::ErrorKind::NotFound => None,
            Err(x) => return Err(format!("{}: {x}", shown())),
            Ok(m) if m.is_file() => Some(std::fs::read(&path).map_err(|x| format!("{}: {x}", shown()))?),
            Ok(_) => {
                return Err(format!(
                    "{} is in the way of a scanned sheet and is not a regular file; move it or pick another --first-page",
                    shown()
                ));
            }
        };
        scans.push((path, before));
    }
    // The folder is removed again only when this import makes it.
    let makes_folder = !scans.is_empty()
        && matches!(std::fs::symlink_metadata(&underlays), Err(x) if x.kind() == std::io::ErrorKind::NotFound);
    let mut save_started = false;
    let written = write_pdf_pages(dir, &mut p, traced, first_page, (&stem, source), log, &mut save_started);
    written.map_err(|why| {
        // A scan that could not be written fails before the save: the project's own files are
        // untouched then and are left alone, a link among them that points nowhere too (the
        // snapshot holds it as no file, and putting that back would remove it).
        let mut failed = if save_started {
            put_files_back(&project_files)
        } else {
            Vec::new()
        };
        failed.extend(put_files_back(&scans));
        if makes_folder
            && let Err(x) = std::fs::remove_dir(&underlays)
            && x.kind() != std::io::ErrorKind::NotFound
        {
            failed.push(format!("{}: {x}", underlays.display()));
        }
        if failed.is_empty() {
            why
        } else {
            format!(
                "{why}; the project folder could not be put back as it was: {}",
                escape_field(&failed.join("; "))
            )
        }
    })
}

/// The writing half of [`import_pdf`], after its refusals: the scan of each scanned page into
/// the underlay folder, the pages into `p`, and the save. The caller puts the folder back as it
/// was when this fails. The pair of names is the PDF's file stem (for page titles) and its name
/// in warnings. `save_started` is set once the save is reached: before that only scans are written.
fn write_pdf_pages(
    dir: &Path,
    p: &mut Project,
    traced: Vec<crate::pdf::TracePage>,
    first_page: usize,
    (stem, source): (&str, &str),
    log: &mut Log,
    save_started: &mut bool,
) -> Res<()> {
    for (i, t) in traced.into_iter().enumerate() {
        let id = format!("pdf-{}", first_page + i);
        if t.graphics.is_empty() && t.scan.is_none() {
            // The reason can quote the file: written so it cannot send control sequences.
            let why = escape_field(t.scan_problem.as_deref().unwrap_or("no picture covers the page"));
            err!(
                log,
                "warning: {} page {}: no vector content and no usable scan ({why})",
                source,
                i + 1,
            );
        }
        let underlay = match &t.scan {
            Some(scan) => {
                let file = format!("{}/{id}.{}", wc_core::Underlay::DIR, scan.raster.format.extension());
                std::fs::create_dir_all(dir.join(wc_core::Underlay::DIR)).map_err(e)?;
                // Staged and renamed, so a write that fails leaves no picture cut short.
                store::replace_whole(&dir.join(&file), &scan.raster.bytes).map_err(e)?;
                out!(
                    log,
                    "page {}: scanned sheet saved as {file}, shown under the page",
                    first_page + i
                );
                if let Some(warning) = &t.scan_warning {
                    // The reason can quote the file: written so it cannot send control sequences.
                    err!(
                        log,
                        "warning: {} page {}: the scanned sheet was read only in part ({})",
                        source,
                        i + 1,
                        escape_field(warning)
                    );
                }
                Some(wc_core::Underlay {
                    file,
                    min: scan.min,
                    max: scan.max,
                })
            }
            None => None,
        };
        let mut page = Page::new(id, (first_page + i).to_string(), format!("{stem} p.{}", i + 1));
        page.underlay = underlay;
        let (size, why) = sheet_for(t.size_mm);
        if let Some(why) = why {
            err!(
                log,
                "warning: page {} ({} page {}): {why}",
                first_page + i,
                source,
                i + 1
            );
        }
        page.size = size;
        out!(log, "page {}: {} shapes", page.number, t.graphics.len());
        page.graphics = t.graphics;
        p.pages.push(page);
    }
    p.sort_pages();
    *save_started = true;
    save(dir, p)
}

/// The sheet for a traced drawing of `(width, height)` millimetres: the smallest ISO size
/// that holds both sides, upright when the drawing is. A drawing larger than A0 gets a sheet
/// of its own size, each side at least [`PageSize::MIN_MM`]; one beyond the custom range gets
/// A0, with the reason as a warning.
pub fn sheet_for((w, h): (f64, f64)) -> (PageSize, Option<String>) {
    // PDF sizes are in whole points, so an A3 page reads a fraction of a millimetre off; one
    // point of slack covers that rounding and no more, so no drawing is cut by the sheet.
    const SLACK: f64 = 25.4 / 72.0;
    let portrait = h > w;
    if let Some(named) = PageSize::NAMED
        .into_iter()
        .filter(|s| s.is_portrait() == portrait)
        .find(|s| {
            let (sw, sh) = s.mm();
            w <= sw + SLACK && h <= sh + SLACK
        })
    {
        return (named, None);
    }
    let side = |mm: f64| (mm.ceil() as u32).max(PageSize::MIN_MM);
    match PageSize::custom(side(w), side(h)) {
        Ok(size) => (size, None),
        Err(why) => {
            let a0 = if portrait { PageSize::A0Portrait } else { PageSize::A0 };
            (a0, Some(format!("{why}, so it is drawn on {}", a0.label())))
        }
    }
}

/// The largest DWG WiringCraft's own reader takes into memory, far above real drawings. A larger
/// file is refused by the reader and goes to the converter, as other refusals do.
pub const MAX_DWG_BYTES: u64 = 256 << 20;

/// Read `file` for the built-in DWG reader, at most `limit` bytes: its bytes, or (inside `Ok`)
/// why the reader does not take it. A file that cannot be opened or read is an error.
fn read_dwg(file: &Path, limit: u64) -> Res<std::result::Result<Vec<u8>, String>> {
    use std::io::Read;
    let cannot = |err: std::io::Error| format!("cannot read {}: {err}", escape_field(&file.display().to_string()));
    let mut bytes = Vec::new();
    // At most one byte past the limit, however large the file is.
    std::fs::File::open(file)
        .map_err(cannot)?
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(cannot)?;
    if bytes.len() as u64 > limit {
        return Ok(Err(format!("the file is larger than the {} MB it reads", limit >> 20)));
    }
    Ok(Ok(bytes))
}

/// `import page-dwg`: read `file` with WiringCraft's own DWG reader (AutoCAD 2004 to 2018) and
/// import it onto `page`. Only when that reader refuses the file (another version, or a file
/// it cannot read) is it converted to DXF with an installed converter (`template`, see
/// [`dwg_to_dxf`]); the converter writes into a private folder of its own, removed with
/// everything in it when this ends, also on failure. Says which of the two read the file.
pub fn import_page_dwg(
    dir: &Path,
    file: &Path,
    page: &str,
    scale: f64,
    template: Option<String>,
    log: &mut Log,
) -> Res<()> {
    let bytes = read_dwg(file, MAX_DWG_BYTES)?;
    // A scale the import refuses is refused before any converter runs.
    if !(scale.is_finite() && scale > 0.0) {
        return Err("scale must be a positive number".into());
    }
    // The reader refuses a file of another version, or one whose object structure it cannot
    // read; a converter may still read those. What it reads but finds nothing to draw in, and
    // project or library errors, are not the converter's to fix.
    let read = bytes.and_then(|bytes| {
        crate::dwg::read_sections(&bytes)
            .and_then(|sections| crate::dwg::page_drawing(&sections))
            .map_err(|why| why.to_string())
    });
    let refused = match read {
        Ok(drawing) => {
            return onto_page(dir, page, log, |known| dwg_page(&drawing, scale, known))
                .map(|()| out!(log, "read with WiringCraft's own DWG reader"));
        }
        // Names from the file may hold control characters: escaped for the terminal.
        Err(why) => escape_field(&why),
    };
    let tmp = staging_dir("wiringcraft-dwg-").map_err(e)?;
    let (dxf, converter) = dwg_to_dxf(file, tmp.path(), template)
        .map_err(|why| format!("WiringCraft's own DWG reader refused the file ({refused}); {why}"))?;
    out!(
        log,
        "converted to DXF with {converter}: WiringCraft's own DWG reader refused the file ({refused})"
    );
    let text = read_file(&dxf)?;
    onto_page(dir, page, log, |known| {
        // Only a DXF that cannot be read gets both reasons; project errors stay as they are.
        crate::dxf::page(&String::from_utf8_lossy(&text), scale, known).map_err(|why| {
            format!(
                "WiringCraft's own DWG reader refused the file ({refused}); the DXF {converter} made from it could not be read either: {}",
                escape_field(&why.to_string())
            )
        })
    })
}

/// The drawing as a page, its error escaped for the terminal: names from the file (a block
/// name in the reasons a drawing with nothing to draw gives) may hold control characters.
pub(crate) fn dwg_page(
    drawing: &crate::dwg::PageDrawing,
    scale: f64,
    known: &std::collections::BTreeSet<String>,
) -> Res<crate::dxf::ImportedPage> {
    drawing.page(scale, known).map_err(|why| escape_field(&why.to_string()))
}

/// Convert DWG to a DXF in `tmp` (a private folder, see [`staging_dir`]) with whatever
/// converter is installed: the command `template` (the WIRINGCRAFT_DWG2DXF setting, with `{in}`
/// and `{out}`) when given, otherwise `dwg2dxf` or the ODA File Converter. Returns the DXF and
/// the converter's name. For the DWG files WiringCraft's own reader refuses.
pub fn dwg_to_dxf(file: &Path, tmp: &Path, template: Option<String>) -> Res<(PathBuf, &'static str)> {
    use std::process::Command;
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "drawing".into());
    let out = tmp.join(format!("{stem}.dxf"));
    let input = file.to_string_lossy().into_owned();
    let output = out.to_string_lossy().into_owned();
    let ran = |mut c: Command| c.output().map(|o| o.status.success()).unwrap_or(false);
    if let Some(template) = template {
        let parts: Vec<String> = template
            .split_whitespace()
            .map(|p| p.replace("{in}", &input).replace("{out}", &output))
            .collect();
        if let Some((prog, args)) = parts.split_first() {
            let mut c = Command::new(prog);
            c.args(args);
            if ran(c) && out.exists() {
                return Ok((out, "the WIRINGCRAFT_DWG2DXF command"));
            }
        }
        return Err("WIRINGCRAFT_DWG2DXF did not produce a DXF".into());
    }
    let mut c = Command::new("dwg2dxf");
    c.args(["-y", "-o", &output, &input]);
    if ran(c) && out.exists() {
        return Ok((out, "dwg2dxf"));
    }
    // ODA File Converter works on folders: <in dir> <out dir> <version> <type> <recurse> <audit> <filter>.
    let in_dir = file
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".into());
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for prog in ["ODAFileConverter", "ODAFileConverter.exe"] {
        let mut c = Command::new(prog);
        c.args([
            in_dir.as_str(),
            tmp.to_string_lossy().as_ref(),
            "ACAD2018",
            "DXF",
            "0",
            "1",
            name.as_str(),
        ]);
        if ran(c) && out.exists() {
            return Ok((out, "ODA File Converter"));
        }
    }
    Err("no DWG converter found: install ODA File Converter or dwg2dxf, or set WIRINGCRAFT_DWG2DXF".into())
}

/// Import a BMEcat catalogue's parts into a library folder. Files that are not UTF-8 are read
/// as Latin-1, which BMEcat 1.2 catalogues often declare. Returns the count and warnings.
pub fn import_bmecat(file: &Path, lib: &Path) -> Res<(usize, Vec<String>)> {
    let bytes = read_file(file)?;
    let (parts, warnings) = crate::bmecat::parts(&crate::csv::decode(&bytes)).map_err(e)?;
    for p in &parts {
        wc_library::write_part(lib, p).map_err(e)?;
    }
    Ok((parts.len(), warnings))
}

/// Import an EDZ archive's parts into a library folder. Returns the count, what the archive
/// held and warnings.
pub fn import_edz(file: &Path, lib: &Path, symbol_map: Option<&Path>) -> Res<(usize, crate::edz::Report, Vec<String>)> {
    // The map is read first, so a bad one stops the import before anything is written.
    let map = match symbol_map {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|x| format!("{}: {x}", path.display()))?;
            Some(crate::swe::parse_symbol_map(&text).map_err(|x| format!("{}: {x}", path.display()))?)
        }
        None => None,
    };
    let bytes = std::fs::read(file).map_err(|x| format!("{}: {x}", file.display()))?;
    let crate::edz::Edz {
        mut parts,
        symbols,
        mut report,
        mut warnings,
    } = crate::edz::read_all(&bytes).map_err(|x| format!("{}: {x}", file.display()))?;
    // The parts whose symbol the map chose.
    let mut mapped: std::collections::BTreeSet<String> = Default::default();
    if let Some(map) = &map {
        let mut probe: Vec<wc_core::Part> = parts
            .iter()
            .map(|p| wc_core::Part {
                symbol: None,
                ..p.clone()
            })
            .collect();
        crate::edz::apply_symbol_map(&mut probe, map, &mut report.clone());
        mapped = probe.into_iter().filter(|p| p.symbol.is_some()).map(|p| p.id).collect();
        crate::edz::apply_symbol_map(&mut parts, map, &mut report);
    }
    // Symbols first, so no part is written pointing at a symbol the library lacks. A symbol that
    // cannot be written is skipped, and its parts are written without it.
    let mut unwritten: std::collections::BTreeSet<String> = Default::default();
    for s in &symbols {
        if let Err(x) = wc_library::write_symbol(lib, s) {
            warnings.push(format!("symbol {} not written: {x}", s.id));
            unwritten.insert(s.id.clone());
        }
    }
    if !unwritten.is_empty() {
        // A link the symbol map chose is kept while the library still holds a readable symbol of
        // that id; a link to the macro's own symbol, which was not saved, is dropped.
        let readable = |id: &str| {
            std::fs::read_to_string(lib.join("symbols").join(format!("{id}.json")))
                .ok()
                .and_then(|t| serde_json::from_str::<wc_core::Symbol>(&t).ok())
                .is_some()
        };
        let keep = |part: &str, id: &str| !unwritten.contains(id) || (mapped.contains(part) && readable(id));
        for p in &mut parts {
            if p.symbol.as_deref().is_some_and(|s| !keep(&p.id, s)) {
                p.symbol = None;
            }
        }
        for m in &mut report.part_macros {
            if m.symbol.as_deref().is_some_and(|s| !keep(&m.part, s)) {
                m.symbol = None;
            }
        }
    }
    for p in &parts {
        wc_library::write_part(lib, p).map_err(e)?;
    }
    Ok((parts.len(), report, warnings))
}

/// Import a SolidWorks Electrical archive's libraries into a library folder, or vendor them into
/// the project (pinned to `source`) when no folder is given. Returns the report and warnings.
pub fn import_swe_library(
    dir: &Path,
    file: &Path,
    lib: Option<&Path>,
    source: &str,
) -> Res<(crate::swe::library::LibraryReport, Vec<String>)> {
    let reader = std::io::BufReader::new(std::fs::File::open(file).map_err(|x| format!("{}: {x}", file.display()))?);
    let got = crate::swe::library::from_archive(reader).map_err(|x| format!("{}: {x}", file.display()))?;
    match lib {
        Some(lib) => {
            for p in &got.parts {
                wc_library::write_part(lib, p).map_err(e)?;
            }
            for s in &got.symbols {
                wc_library::write_symbol(lib, s).map_err(e)?;
            }
        }
        None => {
            let mut p = load(dir)?;
            let built = crate::aml::Built {
                symbols: got.symbols,
                parts: got.parts,
                ..Default::default()
            };
            add_and_save(dir, &mut p, built, source, Replace::Unchanged)?;
        }
    }
    Ok((got.report, got.warnings))
}

/// Import QElectroTech elements into a symbol library folder, one symbol per file. A file that
/// cannot be read is reported and skipped. Returns how many were written and how many failed.
/// The default id prefix for a KiCad library: `kicad.<file name without extension>`, lower
/// case, other characters as `_`. `name` may be a path or an archive entry name.
pub fn kicad_prefix(name: &str) -> String {
    let leaf = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = leaf.rsplit_once('.').map_or(leaf, |(stem, _)| stem);
    format!("kicad.{}", crate::id_segment(stem))
}

/// Read a KiCad library's symbols, printing its warnings; nothing is written yet.
pub fn read_kicad(text: &str, prefix: &str, scale: f64, log: &mut Log) -> Res<Vec<crate::kicad::Imported>> {
    read_kicad_described(text, None, prefix, scale, log)
}

/// [`read_kicad`], an old `.lib` library taking its descriptions from `docs`, its `.dcm` text.
pub fn read_kicad_described(
    text: &str,
    docs: Option<&str>,
    prefix: &str,
    scale: f64,
    log: &mut Log,
) -> Res<Vec<crate::kicad::Imported>> {
    let (symbols, warnings) = crate::kicad::symbols_described(text, docs, prefix, scale).map_err(e)?;
    for w in warnings {
        err!(log, "warning: {}", escape_field(&w));
    }
    Ok(symbols)
}

/// Write read KiCad symbols into `lib`, printing each.
pub fn write_kicad(symbols: Vec<crate::kicad::Imported>, lib: &Path, category: &str, log: &mut Log) -> Res<usize> {
    let n = symbols.len();
    for mut imp in symbols {
        imp.symbol.category = category.to_owned();
        let path = wc_library::write_symbol(lib, &imp.symbol).map_err(e)?;
        out!(
            log,
            "{} -> {} ({} pins, {} shapes)",
            escape_field(&imp.kicad_name),
            path.display(),
            imp.symbol.pins.len(),
            imp.symbol.graphics.len()
        );
    }
    Ok(n)
}

/// The tables of an Eplan project given as an .elk file, an .edb folder or a zip of the folder,
/// with warnings. A table that is there but cannot be read is an error naming it; an unreadable
/// `ProjectInfo.xml` is a warning, and the project is read without its title block data.
pub fn eplan_tables(path: &Path) -> Result<(crate::eplan::Tables, Vec<String>), String> {
    let e = |x: std::io::Error| format!("{}: {x}", path.display());
    let folder = if path.is_dir() {
        path.to_path_buf()
    } else {
        let bytes = std::fs::read(path).map_err(e)?;
        if bytes.starts_with(b"PK") {
            return crate::eplan::tables_from_zip(&bytes).map_err(|x| x.to_string());
        }
        let name = crate::eplan::elk_target(&bytes)
            .ok_or_else(|| format!("{} is not an Eplan .elk file, .edb folder or zip", path.display()))?;
        path.with_file_name(name)
    };
    let mut tables = crate::eplan::Tables::new();
    let mut warnings = Vec::new();
    for entry in std::fs::read_dir(&folder).map_err(|x| format!("{}: {x}", folder.display()))? {
        let entry = entry.map_err(|x| format!("{}: {x}", folder.display()))?;
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if crate::eplan::wanted(&name) {
            // A table that is there but cannot be read is not taken for an empty one.
            let path = entry.path();
            match std::fs::read(&path) {
                Ok(bytes) => {
                    tables.insert(name, bytes);
                }
                Err(x) if crate::eplan::optional(&name) => {
                    warnings.push(format!("{}: {x}; read without it", path.display()));
                }
                Err(x) => return Err(format!("{}: {x}", path.display())),
            }
        }
    }
    Ok((tables, warnings))
}

/// The library symbols a SolidWorks import may draw, with the library each comes from: the
/// project's own copy where it has one (so a repeated import matches it), otherwise the
/// project's libraries. With `map`, the user's block map from that file. Also returns the
/// warnings to show: copies or library symbols that cannot be read, mapped symbols no library has.
pub fn swe_symbol_choice(
    dir: &Path,
    p: &Project,
    map: Option<&Path>,
) -> Res<(
    crate::swe::SymbolChoice,
    std::collections::BTreeMap<String, String>,
    Vec<String>,
)> {
    let mut choice = crate::swe::SymbolChoice::default();
    if let Some(path) = map {
        let text = std::fs::read_to_string(path).map_err(|x| format!("{}: {x}", path.display()))?;
        choice.by_block = crate::swe::parse_symbol_map(&text).map_err(|x| format!("{}: {x}", path.display()))?;
    }
    let mut wanted: std::collections::BTreeSet<String> = choice.by_block.values().cloned().collect();
    wanted.extend(crate::swe::BUILTIN_BY_PINS.iter().map(|(_, id)| (*id).to_owned()));
    let mut warnings = Vec::new();
    let lib = readable_libraries(dir, p, &mut warnings);
    let mut sources = std::collections::BTreeMap::new();
    for id in wanted {
        // The project's own copy first, used on a page or not.
        if p.lock.symbols.contains_key(&id) {
            match project_copy(dir, &id) {
                Ok(Some(s)) => {
                    let source = p.lock.symbols.get(&id).map(|l| l.source.clone()).unwrap_or_default();
                    sources.insert(id.clone(), source);
                    choice.library.insert(id, s);
                    continue;
                }
                Ok(None) => {}
                Err(why) => warnings.push(format!("{why}; trying the libraries")),
            }
        }
        match lib.as_ref().map(|l| l.symbol(&id)) {
            Some(Ok((s, source))) => {
                sources.insert(id.clone(), source);
                choice.library.insert(id, s);
            }
            // Only a symbol that is really missing is "no library has"; one that cannot be read
            // was reported with its reason.
            Some(Err(wc_library::Error::NotFound { .. })) | None => {
                if choice.by_block.values().any(|v| *v == id) {
                    warnings.push(format!(
                        "symbol map names {id}, which no library has; those blocks stay boxes"
                    ));
                }
            }
            Some(Err(why)) => warnings.push(format!(
                "library symbol {id} cannot be read, blocks it would match stay boxes: {why}"
            )),
        }
    }
    Ok((choice, sources, warnings))
}

/// The project's libraries that can be opened, in their order. A source that cannot be opened (a
/// git source not synced yet) only means fewer matches, and says so; the others stay usable.
pub fn readable_libraries(dir: &Path, p: &Project, warnings: &mut Vec<String>) -> Option<wc_library::Library> {
    let mut readable = p.clone();
    readable.meta.libraries.retain(|source| {
        let mut one = p.clone();
        one.meta.libraries = vec![source.clone()];
        wc_library::Library::open(dir, &one)
            .inspect_err(|x| warnings.push(format!("library {} not read, blocks may stay boxes: {x}", source.name)))
            .is_ok()
    });
    wc_library::Library::open(dir, &readable)
        .inspect_err(|x| warnings.push(format!("libraries not read, blocks may stay boxes: {x}")))
        .ok()
}

/// The project's own copy of symbol `id`: `None` when it has none, the reason when the copy is
/// there but cannot be read.
pub fn project_copy(dir: &Path, id: &str) -> Res<Option<wc_core::Symbol>> {
    let path = dir
        .join(store::VENDOR_DIR)
        .join(ItemKind::Symbol.dir())
        .join(format!("{id}.json"));
    match std::fs::metadata(&path) {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Err(format!("project copy of symbol {id} is not a file: {}", path.display())),
        Err(x) if x.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(x) => {
            return Err(format!(
                "project copy of symbol {id} cannot be read: {}: {x}",
                path.display()
            ));
        }
    }
    store::read_vendored_symbol(dir, id)
        .map(Some)
        .map_err(|why| format!("project copy of symbol {id} cannot be read: {why}"))
}

/// Adds a SolidWorks Electrical import to `p` and saves it ([`add_and_save`]), keeping the
/// library symbols it used (named in `sources`, new to the project or pinned before) pinned to
/// their libraries. Returns how many symbol shapes the import made itself, and how many imported
/// jumper groups joined the project's own.
pub fn add_swe_built(
    dir: &Path,
    p: &mut Project,
    built: crate::aml::Built,
    source: &str,
    sources: &std::collections::BTreeMap<String, String>,
) -> Res<(usize, usize)> {
    let n_sym = built.symbols.iter().filter(|s| !sources.contains_key(&s.id)).count();
    let from_library: Vec<String> = built
        .symbols
        .iter()
        .filter(|s| sources.contains_key(&s.id))
        .map(|s| s.id.clone())
        .collect();
    // Not replacing: a generated symbol may still be placed on pages the project keeps. Saved
    // like every import: a failed save puts every file back, jumpers and pages together.
    let joined = add_and_save_with(dir, p, built, source, Replace::No, |p| {
        keep_library_sources(p, &from_library, sources);
    })?;
    Ok((n_sym, joined))
}

/// Library symbols an import used stay pinned to the library they came from (`add_built` pins
/// everything it adds to the import), whether they were pinned before or not.
pub fn keep_library_sources(p: &mut Project, ids: &[String], sources: &std::collections::BTreeMap<String, String>) {
    for id in ids {
        if let (Some(lib), Some(entry)) = (sources.get(id), p.lock.symbols.get_mut(id))
            && !lib.is_empty()
        {
            entry.source.clone_from(lib);
        }
    }
}

/// A project has one cover page. When the project already has one, imported cover pages are
/// dropped and their title-block values (the first non-blank one of each field) come back, to
/// fill the fields the existing cover leaves empty with [`fill_cover`]; otherwise only the first
/// imported cover is kept. The project itself is not changed, so a refused import leaves it as
/// it was.
pub fn keep_one_cover(p: &Project, pages: &mut Vec<Page>) -> std::collections::BTreeMap<String, String> {
    let mut kept = p.cover_page().is_some();
    let mut fields = std::collections::BTreeMap::new();
    pages.retain(|pg| {
        if !pg.cover {
            return true;
        }
        if kept {
            for (k, v) in pg.title_block.iter().filter(|(_, v)| !v.trim().is_empty()) {
                fields.entry(k.clone()).or_insert_with(|| v.clone());
            }
            return false;
        }
        kept = true;
        true
    });
    fields
}

/// Fill the project cover's empty title-block fields from `fields` ([`keep_one_cover`]).
pub fn fill_cover(p: &mut Project, fields: std::collections::BTreeMap<String, String>) {
    let Some(cover) = p.cover_page().and_then(|i| p.pages.get_mut(i)) else {
        return;
    };
    for (k, v) in fields {
        let slot = cover.title_block.entry(k).or_default();
        if slot.trim().is_empty() {
            *slot = v;
        }
    }
}

/// What [`add_built`] wrote: each vendored file with what it held before (none when it had no
/// such file), for [`put_files_back`] when the save after it fails.
pub type Written = Vec<(PathBuf, Option<Vec<u8>>)>;

/// Whether an import may replace a vendored item the project has in another version.
#[derive(Clone, Copy, PartialEq)]
pub enum Replace {
    /// Never: an Eplan, AML or SolidWorks Electrical project import makes its own symbols, and a
    /// page the project kept may still use the old one's pins. A copy the lock does not list is
    /// kept just the same (#649).
    No,
    /// When the project's copy is still the pinned one (a library import, #258). A copy the lock
    /// does not list has no pin to compare with and is replaced.
    Unchanged,
}

/// [`add_built`], then save: when the save fails, the vendored files and every project file
/// the save may have written go back as they were, so the project on disk is the one before.
pub fn add_and_save(dir: &Path, p: &mut Project, built: crate::aml::Built, source: &str, replace: Replace) -> Res<()> {
    add_and_save_with(dir, p, built, source, replace, |_| {}).map(|_| ())
}

/// [`add_and_save`], with `finish` changing the project after the import and before the save,
/// and giving how many imported jumper groups joined the project's own ([`add_built`]).
pub fn add_and_save_with(
    dir: &Path,
    p: &mut Project,
    built: crate::aml::Built,
    source: &str,
    replace: Replace,
    finish: impl FnOnce(&mut Project),
) -> Res<usize> {
    let pages: Vec<&str> = built.pages.iter().map(|pg| pg.id.as_str()).collect();
    let panels: Vec<&str> = built.panels.iter().map(|n| n.id.as_str()).collect();
    let project_files = snapshot_project(dir, &pages, &panels)?;
    let (written, joined) = add_built(dir, p, built, source, replace)?;
    finish(p);
    if let Err(err) = save_imported(dir, p) {
        let mut failed = put_files_back(&project_files);
        failed.extend(put_files_back(&written));
        if failed.is_empty() {
            return Err(err);
        }
        return Err(format!(
            "{err}; putting back the vendored files failed too: {}",
            failed.join("; ")
        ));
    }
    Ok(joined)
}

/// Every file a save of the project may write or remove, with its bytes before (`None` when it
/// does not exist yet): the project's own files, every file in its page, panel and comment
/// folders, the black box symbol files its cleanup may remove, and the files of the pages and
/// panels about to be added. [`put_files_back`] restores them when a save fails half way.
pub fn snapshot_project(dir: &Path, new_pages: &[&str], new_panels: &[&str]) -> Res<Written> {
    // Every file the save may write or remove, read before anything is written: the project's
    // own files, every file in its page, panel and comment folders, and the files the new pages
    // and panels will get.
    let mut paths: Vec<PathBuf> = [store::PROJECT_FILE, store::DEVICES_FILE, store::LOCK_FILE]
        .iter()
        .map(|name| dir.join(name))
        .collect();
    // Only the names the store reads and writes: another file there is never changed by the save.
    for (folder, suffix) in [
        (store::PAGES_DIR, store::PAGE_SUFFIX),
        (store::PANELS_DIR, store::PANEL_SUFFIX),
        (store::COMMENTS_DIR, store::COMMENTS_SUFFIX),
    ] {
        let folder = dir.join(folder);
        // A folder that cannot be listed would leave files the save may change without a copy.
        let entries = match std::fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(x) if x.kind() == std::io::ErrorKind::NotFound => continue,
            Err(x) => return Err(format!("{}: {x}", folder.display())),
        };
        for entry in entries {
            let path = entry.map_err(|x| format!("{}: {x}", folder.display()))?.path();
            let ours = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(suffix));
            if ours && path.is_file() {
                paths.push(path);
            }
        }
    }
    // The save ends by removing unused black box symbol files: one that fails half way through
    // that cleanup must be able to put back the ones already gone (#606).
    paths.extend(store::removable_box_symbol_files(dir).map_err(e)?);
    for id in new_pages {
        paths.push(dir.join(store::PAGES_DIR).join(format!("{id}{}", store::PAGE_SUFFIX)));
    }
    for id in new_panels {
        paths.push(dir.join(store::PANELS_DIR).join(format!("{id}{}", store::PANEL_SUFFIX)));
    }
    paths.sort();
    paths.dedup();
    let mut project_files: Written = Vec::new();
    for path in paths {
        let before = match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(x) if x.kind() == std::io::ErrorKind::NotFound => None,
            Err(x) => return Err(format!("{}: {x}", path.display())),
        };
        project_files.push((path, before));
    }
    Ok(project_files)
}

/// Wire ends in `p` on pins of the symbol `symbol.id` that `symbol` does not have:
/// `page 3 -K1:13` each.
pub fn wired_pins_lost(p: &Project, symbol: &wc_core::Symbol) -> Vec<String> {
    let mut out = Vec::new();
    for page in &p.pages {
        for w in &page.wires {
            for end in [&w.from, &w.to] {
                let wc_core::Endpoint::Pin { item, pin } = end else {
                    continue;
                };
                let Some(placed) = page.symbol(item) else { continue };
                if placed.symbol == symbol.id && symbol.pin(pin).is_none() {
                    let name = placed.tag.as_ref().map_or_else(|| item.clone(), ToString::to_string);
                    out.push(format!("page {} {name}:{pin}", page.number));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Put each file back as it was ([`put_file_back`]), the last written first. Returns why each
/// one that could not be put back failed. The bytes of a file that could not be put back are
/// kept next to it ([`keep_before`]), and the reason names where (#1889).
pub fn put_files_back(files: &[(PathBuf, Option<Vec<u8>>)]) -> Vec<String> {
    files
        .iter()
        .rev()
        .filter_map(|(path, before)| {
            let err = put_file_back(path, before.as_deref()).err()?;
            let Some(bytes) = before else { return Some(err) };
            Some(match keep_before(path, bytes) {
                Ok(kept) => format!("{err}; its content from before is kept in {}", kept.display()),
                Err(x) => format!("{err}; keeping its content from before failed too: {x}"),
            })
        })
        .collect()
}

/// Write `bytes` to a new file next to `path`: `<file>.before`, or `<file>.before-2`, ... when
/// that name is taken (a file already there is never replaced). On Unix it is readable by its
/// owner only, as the staging file of [`put_file_back`] is. Gives the file's path.
pub fn keep_before(path: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write as _;
    for n in 1..=100u32 {
        let mut name = path.as_os_str().to_owned();
        name.push(if n == 1 {
            ".before".to_string()
        } else {
            format!(".before-{n}")
        });
        let kept = PathBuf::from(name);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = match options.open(&kept) {
            Ok(file) => file,
            Err(x) if x.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(x) => return Err(format!("{}: {x}", kept.display())),
        };
        return match file.write_all(bytes).and_then(|()| file.sync_all()) {
            Ok(()) => Ok(kept),
            Err(x) => {
                drop(file);
                match std::fs::remove_file(&kept) {
                    Ok(()) => Err(format!("{}: {x}", kept.display())),
                    Err(y) => Err(format!(
                        "{}: {x}; the incomplete copy is left, removing it failed: {y}",
                        kept.display()
                    )),
                }
            }
        };
    }
    Err(format!(
        "{}: no free name to keep its content from before",
        path.display()
    ))
}

/// Add an import to `p` (not saved yet), giving the vendored files it wrote and how many
/// imported jumper groups joined a jumper the project already had (directly, or through another
/// imported group that did): the saved project holds those as part of that jumper, not as
/// jumpers of their own.
pub fn add_built(
    dir: &Path,
    p: &mut Project,
    mut built: crate::aml::Built,
    source: &str,
    replace: Replace,
) -> Result<(Written, usize), String> {
    let cover_fields = keep_one_cover(p, &mut built.pages);
    // Refuse before anything is written: a repeated import must not overwrite the symbol files of
    // an earlier one and then fail half way. Numbers first, so the refusal names the page that
    // holds a number taken.
    for page in &built.pages {
        number_free(p, &page.number)?;
    }
    if let Some(page) = built.pages.iter().find(|pg| p.page(&pg.id).is_some()) {
        return Err(format!(
            "page {} exists; remove it or import into a fresh project",
            page.id
        ));
    }
    let mut ids = std::collections::BTreeSet::new();
    if let Some(page) = built.pages.iter().find(|pg| !ids.insert(pg.id.as_str())) {
        return Err(format!("the import has page {} twice", page.id));
    }
    // A new version of a symbol must keep every pin a wire in the project ends on: one the lock
    // pins in another version, and one the project only has a copy of (no lock entry), which
    // an import that may replace (`Replace::Unchanged`) replaces just the same.
    for symbol in &built.symbols {
        let replaced = match p.lock.symbols.get(&symbol.id) {
            Some(entry) => entry.hash != symbol.hash(),
            None => match store::read_vendored_symbol(dir, &symbol.id) {
                Ok(have) => have.hash() != symbol.hash(),
                // A copy that cannot be read is replaced too; no copy at all is nothing replaced.
                Err(_) => vendored_bytes(dir, ItemKind::Symbol, &symbol.id)?.1.is_some(),
            },
        };
        if replaced {
            let lost = wired_pins_lost(p, symbol);
            if !lost.is_empty() {
                return Err(format!(
                    "symbol {}: wires end on pins the new version no longer has ({}); move those wires first",
                    symbol.id,
                    lost.join(", ")
                ));
            }
        }
    }
    // A new version replaces the project's copy only when both are the same item of their
    // library: ids given by name can move to another block in another catalog (#301).
    for symbol in &built.symbols {
        if replace == Replace::No || !p.lock.symbols.get(&symbol.id).is_some_and(|e| e.hash != symbol.hash()) {
            continue;
        }
        if let Ok(have) = store::read_vendored_symbol(dir, &symbol.id)
            && let Some(why) = wc_core::symbol::replace_refused(&have, symbol)
        {
            return Err(format!("{why}; import into a fresh project"));
        }
    }
    // A library item already in the project may come again unchanged (same content hash), or
    // in a new version when the project's copy is still the one its lock pinned: nobody changed
    // it in the project, so the import replaces it (a block now drawn, a newer catalog). Every
    // file is read first, so a failed write can put them all back as they were.
    let mut files: Vec<(PathBuf, Option<Vec<u8>>)> = Vec::new();
    let mut kept: Vec<bool> = Vec::new();
    let items = built
        .symbols
        .iter()
        .map(|s| (ItemKind::Symbol, s.id.as_str(), s.hash()))
        .chain(built.parts.iter().map(|x| (ItemKind::Part, x.id.as_str(), x.hash())));
    for (kind, id, hash) in items {
        let name = match kind {
            ItemKind::Symbol => format!("symbol {id}"),
            ItemKind::Part => format!("part {id}"),
        };
        let (path, before) = vendored_bytes(dir, kind, id)?;
        // The content hash of the project's copy: none when it has no such file.
        let have = || match (&before, kind) {
            (None, _) => None,
            (Some(_), ItemKind::Symbol) => Some(store::read_vendored_symbol(dir, id).map(|x| x.hash())),
            (Some(_), ItemKind::Part) => Some(store::read_vendored_part(dir, id).map(|x| x.hash())),
        };
        if let Some(entry) = p.lock.map(kind).get(id) {
            if replace == Replace::No && entry.hash != hash {
                return Err(format!("{name} exists with other content; import into a fresh project"));
            }
            match have() {
                Some(Ok(h)) if h == entry.hash => {}
                Some(Ok(_)) => {
                    return Err(format!("{name} exists with other content; import into a fresh project"));
                }
                Some(Err(why)) => {
                    return Err(format!(
                        "{name}: its copy {} cannot be read ({why}); repair it and import again",
                        path.display()
                    ));
                }
                None if entry.hash == hash => {}
                None => {
                    return Err(format!(
                        "{name}: the project pins another version and has no copy of it ({}); restore the pinned copy from the project's history, or import the pinned version itself",
                        path.display()
                    ));
                }
            }
        } else if replace == Replace::No {
            // No lock entry (the copy was put there by hand, or its pin was lost): not replacing
            // keeps that copy too, so only the same content may come again (#649).
            match have() {
                None => {}
                Some(Ok(h)) if h == hash => {}
                Some(Ok(_)) => {
                    return Err(format!("{name} exists with other content; import into a fresh project"));
                }
                Some(Err(why)) => {
                    return Err(format!(
                        "{name}: its copy {} cannot be read ({why}); repair it and import again",
                        path.display()
                    ));
                }
            }
        }
        // Not replacing, a copy that passed the checks above holds the incoming content: it is
        // not written again, so the file keeps its own bytes whatever its layout.
        kept.push(replace == Replace::No && before.is_some());
        files.push((path, before));
    }
    if let Some(panel) = built
        .panels
        .iter()
        .find(|n| p.panels.iter().any(|have| have.id == n.id))
    {
        return Err(format!(
            "panel {} exists; remove it or import into a fresh project",
            panel.id
        ));
    }
    // A device the project already has under the same tag may come again only unchanged.
    if let Some((tag, _)) = built
        .devices
        .iter()
        .find(|(t, d)| p.devices.get(*t).is_some_and(|have| have != *d))
    {
        return Err(format!(
            "device {tag} exists with other data; rename it or import into a fresh project"
        ));
    }
    // Jumpers join the project's own: one that shares a terminal with a jumper of the same
    // strip becomes one jumper with it. Checked here, before anything is written.
    let mut jumpers = p.meta.clone();
    for j in &built.jumpers {
        jumpers
            .add_jumper(&j.strip, &j.terminals)
            .map_err(|why| format!("jumper on {}: {why}", j.strip))?;
    }
    // The imported pages can renumber the project's terminals tagged with only their strip, so
    // the project's own jumpers follow first; the imported ones already have the final numbers.
    let before = if !p.meta.follows_terminals() {
        None
    } else {
        Some(terminal_numbers_with(dir, p, &built.symbols, "import", "import again")?)
    };
    // A write that fails puts back the files written before it (and removes the ones it made),
    // so each copy still matches its pin. Only files this import changed are put back: one it
    // left as it was keeps whatever it holds now.
    let (keep_symbols, keep_parts) = kept.split_at(built.symbols.len().min(kept.len()));
    let writes = built
        .symbols
        .iter()
        .zip(keep_symbols)
        .map(|(s, keep)| {
            if *keep {
                Ok(false)
            } else {
                store::write_vendored_symbol(dir, s)
            }
        })
        .chain(built.parts.iter().zip(keep_parts).map(|(x, keep)| {
            if *keep {
                Ok(false)
            } else {
                store::write_vendored_part(dir, x)
            }
        }));
    let mut changed: Written = Vec::new();
    for (file, written) in files.into_iter().zip(writes) {
        match written {
            Ok(true) => changed.push(file),
            Ok(false) => {}
            Err(err) => {
                let failed = put_files_back(&changed);
                if failed.is_empty() {
                    return Err(err.to_string());
                }
                return Err(format!(
                    "{err}; putting back the files written before it failed too: {}",
                    failed.join("; ")
                ));
            }
        }
    }
    fill_cover(p, cover_fields);
    for s in &built.symbols {
        p.lock.symbols.insert(
            s.id.clone(),
            wc_core::LockEntry {
                source: source.to_owned(),
                hash: s.hash(),
            },
        );
    }
    for part in &built.parts {
        p.lock.parts.insert(
            part.id.clone(),
            wc_core::LockEntry {
                source: source.to_owned(),
                hash: part.hash(),
            },
        );
    }
    for page in built.pages {
        if p.page(&page.id).is_some() {
            return Err(format!(
                "page {} exists; remove it or import into a fresh project",
                page.id
            ));
        }
        p.pages.push(page);
    }
    p.sort_pages();
    for (t, d) in built.devices {
        p.devices.insert(t, d);
    }
    for loc in built.locations {
        p.meta.merge_location(loc);
    }
    if let Some(before) = before {
        let (symbols, _) = store::vendored_symbols(dir, p);
        wc_core::project::follow_terminals_in(p, &before, |id| symbols.get(id).map(|s| s.role));
    }
    // Terminals held by the project's own jumpers (with their numbers after following), and by
    // the imported groups that joined one of them.
    let mut joined_terminals: std::collections::BTreeSet<(DeviceTag, String)> = p
        .meta
        .jumpers
        .iter()
        .flat_map(|j| j.terminals.iter().map(|t| (j.strip.clone(), t.clone())))
        .collect();
    // Until nothing more joins, so a group that reaches the project's jumper only through one
    // listed after it counts too.
    let mut joined_groups = vec![false; built.jumpers.len()];
    loop {
        let mut more = false;
        for (j, done) in built.jumpers.iter().zip(joined_groups.iter_mut()) {
            if !*done
                && j.terminals
                    .iter()
                    .any(|t| joined_terminals.contains(&(j.strip.clone(), t.clone())))
            {
                *done = true;
                more = true;
                joined_terminals.extend(j.terminals.iter().map(|t| (j.strip.clone(), t.clone())));
            }
        }
        if !more {
            break;
        }
    }
    let joined = joined_groups.iter().filter(|j| **j).count();
    for j in &built.jumpers {
        p.meta
            .add_jumper(&j.strip, &j.terminals)
            .map_err(|why| format!("jumper on {}: {why}", j.strip))?;
    }
    p.panels.extend(built.panels);
    p.panels.sort_by(|a, b| a.id.cmp(&b.id));
    // Title-block fields fill the ones the project has not set; nothing typed in is overwritten.
    p.meta.fill_title_block(built.title_block);
    Ok((changed, joined))
}

/// The path of an item's vendored file and its bytes, none when there is no such file.
pub fn vendored_bytes(dir: &Path, kind: ItemKind, id: &str) -> Result<(PathBuf, Option<Vec<u8>>), String> {
    let path = store::vendor_path(dir, kind, id).map_err(|x| x.to_string())?;
    match std::fs::read(&path) {
        Ok(bytes) => Ok((path, Some(bytes))),
        Err(x) if x.kind() == std::io::ErrorKind::NotFound => Ok((path, None)),
        Err(x) => Err(format!("{}: {x}", path.display())),
    }
}

/// Put `path` back to `before` (remove it when it had none). The bytes go to a file beside it
/// first and then replace it, so a failure never leaves it cut off. Returns why it failed.
pub fn put_file_back(path: &Path, before: Option<&[u8]>) -> Result<(), String> {
    let fail = |x: std::io::Error| format!("{}: {x}", path.display());
    let Some(bytes) = before else {
        return match std::fs::remove_file(path) {
            Err(x) if x.kind() != std::io::ErrorKind::NotFound => Err(fail(x)),
            _ => Ok(()),
        };
    };
    if std::fs::read(path).is_ok_and(|now| now == bytes) {
        return Ok(());
    }
    // The file put back keeps the permissions the file there has now (a regular file, not a
    // link): the staging file is made readable by its owner only (#664).
    // A missing file has none to keep, and is put back owner-only; a file whose metadata cannot
    // be read is not put back with other permissions unsaid.
    let keep = match std::fs::symlink_metadata(path) {
        Ok(m) => m.is_file().then(|| m.permissions()),
        Err(x) if x.kind() == std::io::ErrorKind::NotFound => None,
        Err(x) => return Err(fail(x)),
    };
    // The staging file is made new (never one already there, which could be a link to a file
    // elsewhere), and only that file is removed when something fails.
    let mut made = None;
    for n in 0..100u32 {
        let mut part = path.as_os_str().to_owned();
        part.push(format!(".restore-{}-{n}", std::process::id()));
        let part = PathBuf::from(part);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        // Made readable by its owner only before any bytes go in: the bytes may be a file of
        // the project's that others are not to read.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(&part) {
            Ok(file) => {
                made = Some((part, file));
                break;
            }
            Err(x) if x.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(x) => return Err(fail(x)),
        }
    }
    let Some((part, mut file)) = made else {
        return Err(format!("{}: no free name for the staging file", path.display()));
    };
    use std::io::Write as _;
    let done = file
        .write_all(bytes)
        .and_then(|()| keep.map_or(Ok(()), |perm| file.set_permissions(perm)))
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            drop(file);
            std::fs::rename(&part, path)
        });
    done.map_err(|x| match std::fs::remove_file(&part) {
        Ok(()) => fail(x),
        Err(y) => format!("{}; the staging file {} is left: {y}", fail(x), part.display()),
    })
}

/// A fresh folder for files taken out of an archive or written by a converter, its name
/// starting with `prefix`: unique, and on Unix readable only by this user (mode 0700, set as it
/// is created), so nobody can plant a file or link there. Removed when the returned value is
/// dropped.
pub fn staging_dir(prefix: &str) -> std::io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        match tempfile::tempdir() {
            Ok(t) => t,
            Err(x) => unreachable!("no temp folder: {x}"),
        }
    }

    /// A macro's table gives one copy per row, and its name is escaped in the refusals that
    /// name it (#1865).
    #[test]
    fn a_macro_table_gives_a_copy_per_row_and_is_named_escaped() {
        let m = wc_core::macros::Macro {
            name: "Pump".to_owned(),
            placeholders: vec![wc_core::macros::Placeholder {
                name: "what".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let name = "pu\u{1b}[2Jmps\t.csv";
        let copies = |bytes: &[u8]| macro_table_copies(&m, bytes, name, None).map(|c| c.len());
        assert_eq!(copies(b"What\nPump 1\nPump 2\nFan\n"), Ok(3));
        assert_eq!(
            copies(b"what\nPump 1\n\"Pump 2\n"),
            Err(
                "pu\\u{1b}[2Jmps\\t.csv: line 3: a quoted cell is not closed before the end of the file; nothing was placed"
                    .to_owned()
            )
        );
        let old_xls = copies(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1");
        assert!(
            matches!(&old_xls, Err(why) if why.starts_with("pu\\u{1b}[2Jmps\\t.csv: ") && why.contains(".xls")),
            "{old_xls:?}"
        );
        // A column no placeholder has is the macro's own refusal, without the table's name.
        let typo = copies(b"wat\nPump 1\n");
        assert!(matches!(&typo, Err(why) if why.contains("\"wat\"")), "{typo:?}");
    }

    /// The built-in DWG reader takes a file up to its limit, and refuses one a byte larger with
    /// the size in the reason (so the import goes on to the converter, #671).
    #[test]
    fn a_dwg_over_the_size_limit_is_refused_by_the_reader_with_its_size() {
        let t = tmp();
        let path = t.path().join("big.dwg");
        let limit = 1 << 20;
        assert!(std::fs::write(&path, vec![0u8; limit as usize]).is_ok());
        assert!(matches!(read_dwg(&path, limit), Ok(Ok(b)) if b.len() as u64 == limit));
        assert!(std::fs::write(&path, vec![0u8; limit as usize + 1]).is_ok());
        assert_eq!(
            read_dwg(&path, limit),
            Ok(Err("the file is larger than the 1 MB it reads".to_owned()))
        );
        // A file that is not there is an error of its own, not a refusal.
        assert!(read_dwg(&t.path().join("missing.dwg"), limit).is_err());
    }

    /// A file that cannot be put back keeps its bytes from before next to it, under a name not
    /// taken yet, and the reason names that file (#1889).
    #[test]
    fn a_file_that_cannot_be_put_back_keeps_its_content_from_before() {
        let t = tmp();
        let path = t.path().join("p1.page.json");
        // A folder where the file was: putting the file back cannot replace it.
        assert!(std::fs::create_dir(&path).is_ok());
        assert!(std::fs::write(t.path().join("p1.page.json.before"), "older").is_ok());
        let failed = put_files_back(&[(path.clone(), Some(b"before".to_vec()))]);
        let kept = t.path().join("p1.page.json.before-2");
        assert_eq!(failed.len(), 1);
        assert!(
            failed[0].ends_with(&format!("; its content from before is kept in {}", kept.display())),
            "{failed:?}"
        );
        assert_eq!(std::fs::read(&kept).ok().as_deref(), Some(&b"before"[..]));
        // The name already taken is left as it was.
        assert_eq!(
            std::fs::read(t.path().join("p1.page.json.before")).ok().as_deref(),
            Some(&b"older"[..])
        );
        // A file that was not there before has nothing to keep.
        let new = t.path().join("p2.page.json");
        assert!(std::fs::create_dir(&new).is_ok());
        assert!(std::fs::write(new.join("x"), "x").is_ok());
        let failed = put_files_back(&[(new.clone(), None)]);
        assert_eq!(failed.len(), 1);
        assert!(!failed[0].contains("before"), "{failed:?}");
    }

    /// A file put back keeps the permissions it has, not the default ones of a new file (#664).
    #[cfg(unix)]
    #[test]
    fn a_file_put_back_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt as _;
        let t = tmp();
        let mode = |p: &Path| std::fs::metadata(p).map(|m| m.permissions().mode() & 0o777).ok();
        for want in [0o600, 0o640, 0o444] {
            let path = t.path().join(format!("page-{want:o}.toml"));
            assert!(std::fs::write(&path, "now").is_ok());
            assert!(std::fs::set_permissions(&path, std::fs::Permissions::from_mode(want)).is_ok());
            assert_eq!(put_file_back(&path, Some(b"before")), Ok(()));
            assert_eq!(std::fs::read(&path).ok().as_deref(), Some(&b"before"[..]));
            assert_eq!(mode(&path), Some(want), "{want:o}");
        }
    }

    #[test]
    fn a_first_page_number_too_large_to_count_on_from_is_refused_before_anything_is_read() {
        let tmp = tmp();
        let (dir, file) = (tmp.path().join("p"), tmp.path().join("missing"));
        for first in [MAX_FIRST_PAGE + 1, usize::MAX] {
            let mut log = Log::default();
            let said = [
                pdf(&dir, &file, first, &mut log),
                eplan(&dir, &file, first, &mut log),
                swe(&dir, &file, 1, Some(first), None, &mut log),
                aml(&dir, &file, first, &mut log),
                archive(&dir, &file, first, None, &mut log),
            ];
            for r in said {
                assert!(matches!(&r, Err(why) if why.contains("is too large")), "{r:?}");
            }
        }
        // The largest allowed gets as far as the project, which is not there.
        let r = pdf(&dir, &file, MAX_FIRST_PAGE, &mut Log::default());
        assert!(matches!(&r, Err(why) if !why.contains("is too large")), "{r:?}");
    }

    /// Two connected devices.
    const AML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<CAEXFile FileName="t.aml" SchemaVersion="3.0" xmlns="http://www.dke.de/CAEX">
  <InstanceHierarchy Name="Plant">
    <InternalElement Name="=PUMP" ID="f1">
      <InternalElement Name="K1" ID="{e1}">
        <Attribute Name="ReferenceDesignation"><Value>=PUMP+CAB1-K1</Value></Attribute>
        <ExternalInterface Name="A1" ID="i1"/>
      </InternalElement>
      <InternalElement Name="X1" ID="{e2}">
        <ExternalInterface Name="1" ID="i3"/>
      </InternalElement>
      <InternalLink Name="101" RefPartnerSideA="i1" RefPartnerSideB="i3"/>
    </InternalElement>
  </InstanceHierarchy>
</CAEXFile>"#;

    /// A one-page PDF with one line on it.
    fn one_page_pdf() -> Vec<u8> {
        let content = "10 10 m 110 10 l S";
        let mut b = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 1190.55 841.89] >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n".to_vec();
        b.extend_from_slice(format!("4 0 obj\n<< /Length {} >>\nstream\n{content}", content.len()).as_bytes());
        b.extend_from_slice(b"\nendstream\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");
        b
    }

    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            for (name, data) in files {
                let opts = zip::write::SimpleFileOptions::default();
                assert!(w.start_file(*name, opts).is_ok());
                assert!(std::io::Write::write_all(&mut w, data).is_ok());
            }
            assert!(w.finish().is_ok());
        }
        buf.into_inner()
    }

    #[test]
    fn an_archive_at_the_largest_first_page_number_imports_every_entry() {
        let tmp = tmp();
        let dir = tmp.path().join("p");
        assert!(store::save(&dir, &Project::new("t")).is_ok());
        let pdf = one_page_pdf();
        let file = tmp.path().join("plant.zip");
        let bytes = zip_of(&[("a.pdf", &pdf), ("b.pdf", &pdf), ("c.aml", AML.as_bytes())]);
        assert!(std::fs::write(&file, bytes).is_ok());
        let mut log = Log::default();
        let r = archive(&dir, &file, MAX_FIRST_PAGE, None, &mut log);
        assert!(r.is_ok(), "{r:?}");
        // The entries after the first are numbered past the limit on the first page number.
        assert_eq!(log.warnings().collect::<Vec<_>>(), Vec::<&str>::new(), "{}", log.text());
        let p = match load(&dir) {
            Ok(p) => p,
            Err(why) => unreachable!("{why}"),
        };
        // The AutomationML page takes the number after the two PDF pages.
        let mut numbers: Vec<&str> = p.pages.iter().map(|pg| pg.number.as_str()).collect();
        numbers.sort_unstable();
        let want: Vec<String> = (0..3).map(|i| (MAX_FIRST_PAGE + i).to_string()).collect();
        assert_eq!(numbers, want, "{}", log.text());
        assert!(p.page(&format!("pdf-{}", MAX_FIRST_PAGE + 1)).is_some());
    }

    #[test]
    fn a_kicad_library_with_no_symbols_in_an_archive_is_warned_about() {
        let tmp = tmp();
        let dir = tmp.path().join("p");
        assert!(store::save(&dir, &Project::new("t")).is_ok());
        let file = tmp.path().join("plant.zip");
        let empty: &[u8] = b"(kicad_symbol_lib (version 20231120))";
        let bytes = zip_of(&[("a.pdf", &one_page_pdf()), ("empty.kicad_sym", empty)]);
        assert!(std::fs::write(&file, bytes).is_ok());
        let lib = tmp.path().join("lib");
        let mut log = Log::default();
        let r = archive(&dir, &file, 1, Some(&lib), &mut log);
        assert!(r.is_ok(), "{r:?}");
        let warnings: Vec<_> = log.warnings().collect();
        assert_eq!(
            warnings,
            vec!["warning: empty.kicad_sym: no symbol could be imported"],
            "{}",
            log.text()
        );
        // The other entries still come in.
        assert!(matches!(load(&dir), Ok(p) if p.pages.len() == 1));
    }

    #[test]
    fn an_old_xls_workbook_in_an_archive_is_skipped_with_a_reason() {
        let tmp = tmp();
        let dir = tmp.path().join("p");
        assert!(store::save(&dir, &Project::new("t")).is_ok());
        let xls: &[u8] = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1rest";
        let file = tmp.path().join("plant.zip");
        let bytes = zip_of(&[("a.pdf", &one_page_pdf()), ("lists/old.xls", xls)]);
        assert!(std::fs::write(&file, bytes).is_ok());
        let mut log = Log::default();
        let r = archive(&dir, &file, 1, None, &mut log);
        assert!(r.is_ok(), "{r:?}");
        let warnings: Vec<_> = log.warnings().collect();
        assert_eq!(
            warnings,
            vec!["warning: lists/old.xls: skipped, an old Excel workbook (.xls) is not read; save it as .xlsx"],
            "{}",
            log.text()
        );
        assert!(matches!(load(&dir), Ok(p) if p.pages.len() == 1));

        // With nothing else inside, the import still says there was nothing to import.
        let file = tmp.path().join("only.zip");
        assert!(std::fs::write(&file, zip_of(&[("old.xls", xls)])).is_ok());
        let mut log = Log::default();
        let r = archive(&dir, &file, 1, None, &mut log);
        assert!(matches!(&r, Err(why) if why.contains("no importable")), "{r:?}");
        assert_eq!(log.warnings().count(), 1, "{}", log.text());

        // One too large to extract is still named.
        let mut bytes = zip_of(&[("big.xls", xls)]);
        let Some(cd) = bytes.windows(4).position(|w| w == b"PK\x01\x02") else {
            unreachable!("no central directory")
        };
        // The uncompressed size in the central directory header: 512 MiB.
        bytes[cd + 24..cd + 28].copy_from_slice(&(512u32 << 20).to_le_bytes());
        assert!(matches!(crate::archive::inspect(&bytes), Ok(r) if r.entries.iter().any(|en| en.size > 256 << 20)));
        let file = tmp.path().join("big.zip");
        assert!(std::fs::write(&file, bytes).is_ok());
        let mut log = Log::default();
        let r = archive(&dir, &file, 1, None, &mut log);
        assert!(matches!(&r, Err(why) if why.contains("no importable")), "{r:?}");
        assert_eq!(
            log.warnings().collect::<Vec<_>>(),
            vec!["warning: big.xls: skipped, an old Excel workbook (.xls) is not read; save it as .xlsx"],
            "{}",
            log.text()
        );
    }

    #[test]
    fn a_pdf_whose_pages_cannot_be_counted_on_from_the_first_is_refused_before_it_is_written() {
        let tmp = tmp();
        let dir = tmp.path().join("p");
        assert!(store::save(&dir, &Project::new("t")).is_ok());
        let file = tmp.path().join("a.pdf");
        assert!(std::fs::write(&file, one_page_pdf()).is_ok());
        let r = import_pdf(&dir, &file, usize::MAX, "a.pdf", &mut Log::default());
        assert!(
            matches!(&r, Err(why) if why.contains("run past the largest number")),
            "{r:?}"
        );
        assert!(matches!(load(&dir), Ok(p) if p.pages.is_empty()));
        assert_eq!(page_after(MAX_FIRST_PAGE, 10), Ok(MAX_FIRST_PAGE + 10));
        assert!(page_after(usize::MAX - 1, 2).is_err());
    }

    /// A PDF of two scanned sheets: one grey picture over the whole of each page.
    fn two_scans_pdf() -> Vec<u8> {
        let content = "q 595 0 0 842 0 0 cm /Im Do Q";
        let samples = [10u8, 20, 30, 40, 50, 60];
        let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /XObject << /Im 5 0 R >> >> >>";
        let mut b = format!(
            "%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>\nendobj\n3 0 obj\n{page}\nendobj\n6 0 obj\n{page}\nendobj\n4 0 obj\n<< /Length {} >>\nstream\n{content}\nendstream\nendobj\n5 0 obj\n<< /Type /XObject /Subtype /Image /Width 3 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length {} >>\nstream\n",
            content.len(),
            samples.len()
        )
        .into_bytes();
        b.extend_from_slice(&samples);
        b.extend_from_slice(b"\nendstream\nendobj\n%%EOF\n");
        b
    }

    /// A PDF import refuses a page id in use, or a scan that would be saved over a picture in
    /// use, before the scan of any page is written (#1583): the project folder stays as it was.
    #[test]
    fn a_pdf_import_refuses_before_it_writes_the_scan_of_any_page() {
        let tmp = tmp();
        let file = tmp.path().join("scans.pdf");
        assert!(std::fs::write(&file, two_scans_pdf()).is_ok());
        let picture = format!("{}/pdf-5.png", wc_core::Underlay::DIR);
        let project = |name: &str, pages: Vec<Page>| {
            let dir = tmp.path().join(name);
            let mut p = Project::new("t");
            p.pages = pages;
            assert!(store::save(&dir, &p).is_ok());
            assert!(std::fs::create_dir_all(dir.join(wc_core::Underlay::DIR)).is_ok());
            assert!(std::fs::write(dir.join(&picture), b"mine").is_ok());
            dir
        };

        // The second page's id is taken: the first page's scan is not written over a picture.
        let dir = project("id", vec![Page::new("pdf-6", "60", "Mine")]);
        let before = files(&dir);
        let r = pdf(&dir, &file, 5, &mut Log::default());
        assert!(matches!(&r, Err(why) if why.starts_with("page pdf-6 exists;")), "{r:?}");
        assert_eq!(files(&dir), before);

        // An id that differs only in case is refused here too, not by the save after the write.
        let dir = project("case", vec![Page::new("PDF-6", "60", "Mine")]);
        let before = files(&dir);
        let r = pdf(&dir, &file, 5, &mut Log::default());
        assert!(matches!(&r, Err(why) if why.starts_with("page PDF-6 exists;")), "{r:?}");
        assert_eq!(files(&dir), before);

        // The first page's scan would replace the picture under another page.
        let mut mine = Page::new("x", "60", "Mine");
        mine.underlay = Some(wc_core::Underlay {
            file: picture.clone(),
            min: wc_core::Point::new(0.0, 0.0),
            max: wc_core::Point::new(210.0, 297.0),
        });
        let dir = project("picture", vec![mine]);
        let before = files(&dir);
        let r = pdf(&dir, &file, 5, &mut Log::default());
        assert!(
            matches!(&r, Err(why) if why.contains(&format!("would replace {picture}, the picture under page x;"))),
            "{r:?}"
        );
        assert_eq!(files(&dir), before);

        // The same picture under another spelling of its path.
        let mut dotted = Page::new("y", "61", "Mine");
        dotted.underlay = Some(wc_core::Underlay {
            file: format!("{}/./PDF-5.png", wc_core::Underlay::DIR),
            min: wc_core::Point::new(0.0, 0.0),
            max: wc_core::Point::new(210.0, 297.0),
        });
        let other = project("spelling", vec![dotted]);
        let before_other = files(&other);
        let r = pdf(&other, &file, 5, &mut Log::default());
        assert!(
            matches!(&r, Err(why) if why.contains("the picture under page y;")),
            "{r:?}"
        );
        assert_eq!(files(&other), before_other);

        // Free numbers go through, each page with its scan.
        assert_eq!(pdf(&dir, &file, 7, &mut Log::default()), Ok(()));
        for id in ["pdf-7", "pdf-8"] {
            let scan = format!("{}/{id}.png", wc_core::Underlay::DIR);
            assert!(dir.join(&scan).is_file(), "{scan}");
            assert!(
                matches!(load(&dir), Ok(p) if p.page(id).and_then(|pg| pg.underlay.as_ref()).is_some_and(|u| u.file == scan))
            );
        }
    }

    /// A PDF import whose save fails leaves the project folder as it was (#1751): the scans it
    /// wrote are removed, a picture no page used that a scan replaced is back, the underlay
    /// folder it made is gone, and so are the page files the save got to write.
    #[test]
    fn a_pdf_import_whose_save_fails_leaves_the_folder_as_it_was() {
        let tmp = tmp();
        let file = tmp.path().join("scans.pdf");
        assert!(std::fs::write(&file, two_scans_pdf()).is_ok());
        // A folder with a file in it where the save stages the second page's file: the save
        // writes the first page and fails at the second, after both scans are written.
        let block = |dir: &Path| {
            dir.join(store::PAGES_DIR)
                .join(format!("pdf-6{}.tmp", store::PAGE_SUFFIX))
        };
        let project = |name: &str| {
            let dir = tmp.path().join(name);
            assert!(store::save(&dir, &Project::new("t")).is_ok());
            assert!(std::fs::create_dir_all(block(&dir)).is_ok());
            assert!(std::fs::write(block(&dir).join("kept"), b"x").is_ok());
            dir
        };
        let underlays = |dir: &Path| dir.join(wc_core::Underlay::DIR);

        // No underlay folder before: none after.
        let dir = project("fresh");
        let before = files(&dir);
        let mut log = Log::default();
        let r = pdf(&dir, &file, 5, &mut log);
        assert!(matches!(&r, Err(why) if why.contains("pdf-6")), "{r:?}");
        // The failure came after the scans were written, not from a refusal before them.
        assert!(log.text().contains("page 6: scanned sheet saved as"), "{}", log.text());
        assert_eq!(files(&dir), before);
        assert!(!underlays(&dir).exists());
        assert!(matches!(load(&dir), Ok(p) if p.pages.is_empty()));

        // A picture no page uses, where the first scan goes, and another beside it: both as
        // before, and the folder stays.
        let dir = project("stray");
        assert!(std::fs::create_dir_all(underlays(&dir)).is_ok());
        assert!(std::fs::write(underlays(&dir).join("pdf-5.png"), b"mine").is_ok());
        assert!(std::fs::write(underlays(&dir).join("other.png"), b"other").is_ok());
        let before = files(&dir);
        let r = pdf(&dir, &file, 5, &mut Log::default());
        assert!(matches!(&r, Err(why) if why.contains("pdf-6")), "{r:?}");
        assert_eq!(files(&dir), before);

        // An underlay folder that was there and empty stays.
        let dir = project("empty");
        assert!(std::fs::create_dir_all(underlays(&dir)).is_ok());
        let before = files(&dir);
        assert!(pdf(&dir, &file, 5, &mut Log::default()).is_err());
        assert_eq!(files(&dir), before);
        assert!(underlays(&dir).is_dir());

        // With nothing in the way the same import goes through, and leaves no staging file.
        assert!(std::fs::remove_dir_all(block(&dir)).is_ok());
        assert_eq!(pdf(&dir, &file, 5, &mut Log::default()), Ok(()));
        let mut names: Vec<String> = std::fs::read_dir(underlays(&dir))
            .into_iter()
            .flatten()
            .flatten()
            .map(|en| en.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["pdf-5.png", "pdf-6.png"]);
    }

    /// A symbolic link where a scan goes is refused before any scan is written (#1751): a
    /// dangling one reads as no file, and a failure later would remove it. The link stays, with
    /// or without a file behind it, and the error names its path.
    #[cfg(unix)]
    #[test]
    fn a_pdf_import_refuses_a_link_where_a_scan_goes() {
        let tmp = tmp();
        let file = tmp.path().join("scans.pdf");
        assert!(std::fs::write(&file, two_scans_pdf()).is_ok());
        let target = tmp.path().join("elsewhere.png");
        for (name, behind) in [("dangling", false), ("live", true)] {
            let dir = tmp.path().join(name);
            assert!(store::save(&dir, &Project::new("t")).is_ok());
            let underlays = dir.join(wc_core::Underlay::DIR);
            assert!(std::fs::create_dir_all(&underlays).is_ok());
            if behind {
                assert!(std::fs::write(&target, b"mine").is_ok());
            }
            // Where the second page's scan goes: the first page's would be written before it.
            let link = underlays.join("pdf-6.png");
            assert!(std::os::unix::fs::symlink(&target, &link).is_ok());
            let before = files(&dir);
            let mut log = Log::default();
            let r = pdf(&dir, &file, 5, &mut log);
            assert!(
                matches!(&r, Err(why) if why.contains("pdf-6.png") && why.contains("not a regular file")),
                "{name}: {r:?}"
            );
            assert!(!log.text().contains("scanned sheet saved"), "{name}: {}", log.text());
            assert!(std::fs::symlink_metadata(&link).is_ok_and(|m| m.is_symlink()), "{name}");
            assert!(!underlays.join("pdf-5.png").exists(), "{name}");
            assert_eq!(files(&dir), before, "{name}");
            assert!(matches!(load(&dir), Ok(p) if p.pages.is_empty()), "{name}");
        }
        assert_eq!(std::fs::read(&target).ok().as_deref(), Some(&b"mine"[..]));
    }

    /// A scan that cannot be written fails the import before the save has started: the project's
    /// own files are left alone, so a `library.lock` that is a link pointing nowhere is still
    /// there (the snapshot reads it as no file, and putting that back would remove it), and the
    /// scan written before the failing one is gone again.
    #[cfg(unix)]
    #[test]
    fn a_pdf_import_whose_scan_write_fails_keeps_a_dangling_lock_link() {
        let tmp = tmp();
        let file = tmp.path().join("scans.pdf");
        assert!(std::fs::write(&file, two_scans_pdf()).is_ok());
        let dir = tmp.path().join("project");
        assert!(store::save(&dir, &Project::new("t")).is_ok());
        let lock = dir.join(store::LOCK_FILE);
        if std::fs::symlink_metadata(&lock).is_ok() {
            assert!(std::fs::remove_file(&lock).is_ok());
        }
        assert!(std::os::unix::fs::symlink(tmp.path().join("nowhere.lock"), &lock).is_ok());
        // A folder with a file in it where the second scan is staged: its write fails, after
        // the first scan is written and before the save.
        let underlays = dir.join(wc_core::Underlay::DIR);
        let block = underlays.join("pdf-6.png.tmp");
        assert!(std::fs::create_dir_all(&block).is_ok());
        assert!(std::fs::write(block.join("kept"), b"x").is_ok());
        let mut log = Log::default();
        let r = pdf(&dir, &file, 5, &mut log);
        assert!(matches!(&r, Err(why) if why.contains("pdf-6.png.tmp")), "{r:?}");
        assert!(log.text().contains("page 5: scanned sheet saved as"), "{}", log.text());
        assert!(!log.text().contains("page 6: scanned sheet saved as"), "{}", log.text());
        assert!(std::fs::symlink_metadata(&lock).is_ok_and(|m| m.is_symlink()));
        assert!(!underlays.join("pdf-5.png").exists());
        assert!(underlays.is_dir());
        assert!(matches!(load(&dir), Ok(p) if p.pages.is_empty()));
    }

    /// No import gives a new page a number another page has (#869): each route refuses, naming
    /// the page with that number, and the project folder stays as it was.
    #[test]
    fn an_import_onto_a_page_number_in_use_is_refused_and_writes_nothing() {
        let tmp = tmp();
        let dir = tmp.path().join("p");
        let mut p = Project::new("t");
        p.pages.push(Page::new("p0001", " 1 ", "Cover"));
        p.pages.push(Page::new("x", "200", "Mine"));
        // Holds the id the archive's drawing would get, not its number: the number is named.
        p.pages.push(Page::new("DXF-200", "5", "Mine too"));
        assert!(store::save(&dir, &p).is_ok());
        let before = files(&dir);
        let pdf_file = tmp.path().join("sheet.pdf");
        assert!(std::fs::write(&pdf_file, one_page_pdf()).is_ok());
        let aml_file = tmp.path().join("plant.aml");
        assert!(std::fs::write(&aml_file, AML).is_ok());
        let dxf = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0\n20\n0\n11\n10\n21\n0\n0\nENDSEC\n0\nEOF\n";
        let zip_file = tmp.path().join("plant.zip");
        assert!(std::fs::write(&zip_file, zip_of(&[("a.dxf", dxf.as_bytes())])).is_ok());

        let taken = |r: Res<()>, number: &str, page: &str| match r {
            Err(why) => assert!(
                why.starts_with(&format!("page number {number} is taken by page {page};"))
                    && why.contains("--first-page"),
                "{why}"
            ),
            Ok(()) => unreachable!("an import onto page number {number} went through"),
        };
        taken(pdf(&dir, &pdf_file, 1, &mut Log::default()), "1", "p0001");
        taken(pdf(&dir, &pdf_file, 200, &mut Log::default()), "200", "x");
        taken(aml(&dir, &aml_file, 200, &mut Log::default()), "200", "x");
        // An archive goes on to its other entries: the refused one is a warning.
        let mut log = Log::default();
        assert_eq!(archive(&dir, &zip_file, 200, None, &mut log), Ok(()));
        let warned: Vec<&str> = log.warnings().collect();
        assert!(
            matches!(warned.as_slice(), [w] if w.contains("page number 200 is taken by page x;")),
            "{}",
            log.text()
        );
        assert_eq!(files(&dir), before);

        // A free number goes through.
        assert_eq!(pdf(&dir, &pdf_file, 2, &mut Log::default()), Ok(()));
        assert!(matches!(load(&dir), Ok(p) if p.page("pdf-2").is_some_and(|pg| pg.number == "2")));
    }

    /// EPLAN and SolidWorks Electrical imports refuse a page number in use the same way, naming
    /// the page with that number before a page id the import shares with the project.
    #[test]
    fn an_eplan_or_swe_import_onto_a_page_number_in_use_is_refused_and_writes_nothing() {
        let tmp = tmp();
        let edb = tmp.path().join("plant.edb");
        assert!(std::fs::create_dir(&edb).is_ok());
        for (name, bytes) in crate::eplan::tests::sample() {
            assert!(std::fs::write(edb.join(name), bytes).is_ok());
        }
        let db = tmp.path().join("plant.db");
        let Some(db_path) = db.to_str() else {
            unreachable!("temp path is not UTF-8")
        };
        let conn = crate::swe::tests::sample();
        assert!(conn.execute("VACUUM INTO ?1", [db_path]).is_ok());
        type Route<'a> = &'a dyn Fn(&Path) -> Res<()>;
        let eplan_route = |dir: &Path| eplan(dir, &edb, 200, &mut Log::default());
        let swe_route = |dir: &Path| swe(dir, &db, 1, Some(200), None, &mut Log::default());
        let routes: [(&str, Route); 2] = [("eplan", &eplan_route), ("swe", &swe_route)];
        for (name, import) in routes {
            // The ids the import gives its pages, from a fresh project.
            let fresh = tmp.path().join(format!("{name}-fresh"));
            assert!(store::save(&fresh, &Project::new("t")).is_ok());
            assert_eq!(import(&fresh), Ok(()), "{name}");
            let Ok(imported) = load(&fresh) else {
                unreachable!("{name}: not saved")
            };
            let Some(first) = imported.pages.iter().find(|pg| pg.number == "200") else {
                unreachable!("{name}: no page 200 in {:?}", imported.pages)
            };

            let dir = tmp.path().join(name);
            let mut p = Project::new("t");
            p.pages.push(Page::new("x", "200", "Mine"));
            p.pages.push(Page::new(first.id.clone(), "999", "Same id"));
            assert!(store::save(&dir, &p).is_ok());
            let before = files(&dir);
            match import(&dir) {
                Err(why) => assert!(
                    why.starts_with("page number 200 is taken by page x;") && why.contains("--first-page"),
                    "{name}: {why}"
                ),
                Ok(()) => unreachable!("{name}: an import onto page number 200 went through"),
            }
            assert_eq!(files(&dir), before, "{name}");
        }
    }

    /// The refusal escapes the page id and number it names: an id from a file stem may hold ESC.
    #[test]
    fn a_taken_page_number_is_named_without_control_characters() {
        let mut p = Project::new("t");
        p.pages.push(Page::new("a\u{1b}[31m", "7\u{1b}", "Mine"));
        let Err(why) = number_free(&p, "7\u{1b}") else {
            unreachable!("a taken page number went through");
        };
        assert!(!why.contains('\u{1b}'), "{why:?}");
        assert!(
            why.starts_with("page number 7\\u{1b} is taken by page a\\u{1b}[31m;"),
            "{why:?}"
        );
    }

    #[test]
    fn the_log_keeps_the_order_and_tells_warnings_apart() {
        let mut log = Log::default();
        out!(log, "a {}", 1);
        err!(log, "warning: b");
        out!(log, "c");
        assert_eq!(log.text(), "a 1\nwarning: b\nc");
        assert_eq!(log.warnings().collect::<Vec<_>>(), vec!["warning: b"]);
    }

    #[test]
    fn an_eplan_table_that_cannot_be_read_is_reported_not_left_out() {
        let t = tmp();
        let edb = t.path().join("plant.edb");
        assert!(std::fs::create_dir_all(edb.join("connection.eod")).is_ok());
        assert!(std::fs::write(edb.join("page.eod"), b"x").is_ok());
        match eplan_tables(&edb) {
            Ok(_) => unreachable!("an unreadable table was left out"),
            Err(why) => assert!(why.contains("connection.eod"), "{why}"),
        }
    }

    #[test]
    fn an_unreadable_project_info_is_a_warning_and_the_tables_are_still_read() {
        let t = tmp();
        let edb = t.path().join("plant.edb");
        // A folder in its place cannot be read as a file.
        assert!(std::fs::create_dir_all(edb.join("ProjectInfo.xml")).is_ok());
        assert!(std::fs::write(edb.join("page.eod"), b"x").is_ok());
        match eplan_tables(&edb) {
            Ok((tables, warnings)) => {
                assert!(tables.contains_key("page.eod"));
                assert!(!tables.contains_key("projectinfo.xml"));
                assert_eq!(warnings.len(), 1);
                assert!(warnings[0].contains("ProjectInfo.xml"), "{warnings:?}");
            }
            Err(why) => unreachable!("optional metadata stopped the import: {why}"),
        }
    }

    #[test]
    fn a_kicad_symbol_name_is_escaped_before_it_is_reported() {
        let t = tmp();
        let text = "(kicad_symbol_lib (symbol \"R\u{1b}[2J\" (symbol \"R_0_1\" (rectangle (start -1 -1) (end 1 1)))))";
        let mut log = Log::default();
        let symbols = read_kicad(text, "kicad.t", 1.0, &mut log).unwrap_or_default();
        assert_eq!(symbols.len(), 1, "{log:?}");
        assert!(write_kicad(symbols, &t.path().join("lib"), "kicad", &mut log).is_ok());
        let shown = log.text();
        assert!(!shown.contains('\u{1b}'), "{shown:?}");
        assert!(shown.contains("\\u{1b}"), "{shown:?}");
    }

    #[test]
    fn parts_csv_writes_each_part_into_the_library_folder_and_says_how_many() {
        let t = tmp();
        let csv = t.path().join("parts.csv");
        let lib = t.path().join("lib");
        assert!(std::fs::write(&csv, "order number;manufacturer\nX-1;Acme\nX-2;Acme\n").is_ok());
        let mut log = Log::default();
        assert_eq!(parts_csv(&csv, &lib, &mut log), Ok(()));
        let files = std::fs::read_dir(lib.join("parts")).map(|d| d.count()).unwrap_or(0);
        assert_eq!(files, 2);
        assert!(
            log.text()
                .ends_with(&format!("imported 2 part(s) into {}", lib.display())),
            "{log:?}"
        );
    }

    #[test]
    fn a_parts_list_or_catalogue_that_gives_no_part_fails() {
        let t = tmp();
        let lib = t.path().join("lib");
        let csv = t.path().join("parts.csv");
        assert!(std::fs::write(&csv, "order number;manufacturer\n").is_ok());
        let mut log = Log::default();
        assert_eq!(parts_csv(&csv, &lib, &mut log), Err(NO_PART.to_owned()));
        assert!(!log.text().contains("imported"), "{log:?}");
        let xml = t.path().join("cat.xml");
        assert!(
            std::fs::write(
                &xml,
                "<BMECAT version=\"2005\"><T_NEW_CATALOG></T_NEW_CATALOG></BMECAT>"
            )
            .is_ok()
        );
        let mut log = Log::default();
        assert_eq!(parts_bmecat(&xml, &lib, &mut log), Err(NO_PART.to_owned()));
        assert!(!lib.exists());
    }

    /// Every file under `dir` with its bytes.
    fn files(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        let mut out = std::collections::BTreeMap::new();
        let mut todo = vec![dir.to_path_buf()];
        while let Some(d) = todo.pop() {
            for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    todo.push(path);
                } else if let Ok(bytes) = std::fs::read(&path) {
                    out.insert(path, bytes);
                }
            }
        }
        out
    }

    #[test]
    fn a_save_that_fails_after_writing_puts_the_project_and_vendored_files_back() {
        let t = tmp();
        let dir = t.path().join("p");
        assert!(store::save(&dir, &Project::new("t")).is_ok());
        let before = files(&dir);
        let mut p = match load(&dir) {
            Ok(p) => p,
            Err(why) => unreachable!("{why}"),
        };
        let built = crate::aml::Built {
            symbols: vec![wc_core::Symbol {
                source: String::new(),
                id: "imp.coil".into(),
                name: "Coil".into(),
                category: String::new(),
                letter_code: "K".into(),
                role: Default::default(),
                graphics: vec![],
                pins: vec![],
                tag_at: Default::default(),
                description: String::new(),
            }],
            ..Default::default()
        };
        // The save writes the project's own files and the lockfile, then stops at a page id it
        // cannot write; the vendored symbol is already on disk by then.
        let r = add_and_save_with(&dir, &mut p, built, "import:t", Replace::No, |p| {
            p.pages.push(wc_core::Page::new("bad/id", "1", "Bad"));
        });
        assert!(r.is_err());
        assert_ne!(
            p.lock.symbols.get("imp.coil"),
            None,
            "the import was not added before the save"
        );
        assert_eq!(files(&dir), before);
    }

    /// A symbol `id` with the pins `pins`.
    fn symbol_with_pins(id: &str, pins: &[&str]) -> wc_core::Symbol {
        wc_core::Symbol {
            source: String::new(),
            id: id.into(),
            name: "Coil".into(),
            category: String::new(),
            letter_code: "K".into(),
            role: Default::default(),
            graphics: vec![],
            pins: pins
                .iter()
                .zip(0u8..)
                .map(|(name, k)| wc_core::symbol::Pin {
                    name: (*name).into(),
                    at: wc_core::geom::Point::new(0.0, f64::from(k) * 10.0),
                    dir: Default::default(),
                    ..Default::default()
                })
                .collect(),
            tag_at: Default::default(),
            description: String::new(),
        }
    }

    #[test]
    fn a_vendored_symbol_without_a_lock_entry_keeps_the_pins_wires_use() {
        let t = tmp();
        let dir = t.path().join("p");
        let mut p = Project::new("t");
        let mut page = wc_core::Page::new("p0001", "1", "Plan");
        page.symbols.push(wc_core::PlacedSymbol {
            id: "s1".into(),
            symbol: "imp.coil".into(),
            at: wc_core::geom::Point::new(50.0, 50.0),
            rotation: Default::default(),
            mirror: false,
            tag: None,
        });
        page.wires.push(wc_core::Wire {
            id: "w1".into(),
            from: wc_core::Endpoint::Pin {
                item: "s1".into(),
                pin: "A2".into(),
            },
            to: wc_core::Endpoint::Point {
                at: wc_core::geom::Point::new(50.0, 90.0),
            },
            via: vec![],
            number: None,
            potential: None,
            cable: None,
            route: Default::default(),
            spec: Default::default(),
        });
        p.pages.push(page);
        assert!(store::save(&dir, &p).is_ok());
        // The project's copy has no lock entry: put there by hand, or its pin was lost.
        assert!(store::write_vendored_symbol(&dir, &symbol_with_pins("imp.coil", &["A1", "A2"])).is_ok());
        assert_eq!(p.lock.symbols.get("imp.coil"), None);
        let before = files(&dir);

        // A new version without the wired pin A2 is refused, and nothing is written.
        let built = |pins: &[&str]| crate::aml::Built {
            symbols: vec![symbol_with_pins("imp.coil", pins)],
            ..Default::default()
        };
        let mut q = p.clone();
        let err = match add_built(&dir, &mut q, built(&["A1"]), "import:t", Replace::No) {
            Err(why) => why,
            Ok(_) => String::new(),
        };
        assert!(err.contains("symbol imp.coil: wires end on pins"), "{err}");
        assert!(err.contains("page 1 s1:A2"), "{err}");
        assert_eq!(files(&dir), before);
        assert_eq!(q, p, "the project was changed by a refused import");

        // So is one from an import that may replace.
        let mut q = p.clone();
        let err = match add_built(&dir, &mut q, built(&["A1"]), "import:t", Replace::Unchanged) {
            Err(why) => why,
            Ok(_) => String::new(),
        };
        assert!(err.contains("symbol imp.coil: wires end on pins"), "{err}");
        assert_eq!(files(&dir), before);
        assert_eq!(q, p, "the project was changed by a refused import");

        // One that keeps A2 (and adds a pin) replaces the copy and pins it, when the import may
        // replace.
        let mut q = p.clone();
        let r = add_built(&dir, &mut q, built(&["A1", "A2", "A3"]), "import:t", Replace::Unchanged);
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(
            store::read_vendored_symbol(&dir, "imp.coil").map(|s| s.pins.len()).ok(),
            Some(3)
        );
        assert!(q.lock.symbols.contains_key("imp.coil"));
    }

    #[test]
    fn not_replacing_keeps_a_vendored_symbol_and_part_the_lock_does_not_list() {
        let t = tmp();
        let dir = t.path().join("p");
        let p = Project::new("t");
        assert!(store::save(&dir, &p).is_ok());
        // The project's copies have no lock entry: put there by hand, or their pins were lost.
        assert!(store::write_vendored_symbol(&dir, &symbol_with_pins("imp.coil", &["A1", "A2"])).is_ok());
        let part = |description: &str| wc_core::Part {
            id: "imp.relay".into(),
            description: description.into(),
            ..Default::default()
        };
        assert!(store::write_vendored_part(&dir, &part("Relay")).is_ok());
        assert_eq!(p.lock.symbols.get("imp.coil"), None);
        assert_eq!(p.lock.parts.get("imp.relay"), None);
        let before = files(&dir);
        let refused = |built: crate::aml::Built| {
            let mut q = p.clone();
            let err = match add_built(&dir, &mut q, built, "import:t", Replace::No) {
                Err(why) => why,
                Ok(_) => String::new(),
            };
            assert_eq!(q, p, "the project was changed by a refused import");
            err
        };
        let symbols = |pins: &[&str]| crate::aml::Built {
            symbols: vec![symbol_with_pins("imp.coil", pins)],
            ..Default::default()
        };
        let parts = |description: &str| crate::aml::Built {
            parts: vec![part(description)],
            ..Default::default()
        };

        // Another version of the symbol is refused, and the file stays as it was.
        assert_eq!(
            refused(symbols(&["A1", "A2", "A3"])),
            "symbol imp.coil exists with other content; import into a fresh project"
        );
        assert_eq!(files(&dir), before);
        // So is another version of the part.
        assert_eq!(
            refused(parts("Relay 24 V")),
            "part imp.relay exists with other content; import into a fresh project"
        );
        assert_eq!(files(&dir), before);

        // The same content may come again: the files stay as they were and get their pins.
        let mut q = p.clone();
        let mut same = symbols(&["A1", "A2"]);
        same.parts = parts("Relay").parts;
        let r = add_built(&dir, &mut q, same, "import:t", Replace::No);
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(files(&dir), before);
        assert!(q.lock.symbols.contains_key("imp.coil"));
        assert!(q.lock.parts.contains_key("imp.relay"));

        // Also when the copies are laid out another way than this program writes them: the
        // content is the same, so the bytes stay.
        for (kind, id) in [(ItemKind::Symbol, "imp.coil"), (ItemKind::Part, "imp.relay")] {
            let path = dir.join(store::VENDOR_DIR).join(kind.dir()).join(format!("{id}.json"));
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            assert!(!text.is_empty());
            assert!(std::fs::write(&path, format!("\n  {text}\n\n")).is_ok());
        }
        let own_layout = files(&dir);
        assert_ne!(own_layout, before);
        let mut q = p.clone();
        let mut same = symbols(&["A1", "A2"]);
        same.parts = parts("Relay").parts;
        let r = add_built(&dir, &mut q, same, "import:t", Replace::No);
        assert!(r.is_ok_and(|(written, _)| written.is_empty()));
        assert_eq!(files(&dir), own_layout);
        assert!(q.lock.symbols.contains_key("imp.coil"));
        assert!(q.lock.parts.contains_key("imp.relay"));
        for (path, bytes) in &before {
            assert!(std::fs::write(path, bytes).is_ok());
        }

        // A copy that cannot be read is not written over either.
        let path = dir
            .join(store::VENDOR_DIR)
            .join(ItemKind::Symbol.dir())
            .join("imp.coil.json");
        assert!(std::fs::write(&path, b"{").is_ok());
        let broken = files(&dir);
        let err = refused(symbols(&["A1", "A2"]));
        assert!(err.starts_with("symbol imp.coil: its copy "), "{err}");
        assert!(err.ends_with("repair it and import again"), "{err}");
        assert_eq!(files(&dir), broken);
    }

    #[test]
    fn a_dxf_page_whose_save_fails_leaves_the_project_folder_as_it_was() {
        let t = tmp();
        let dir = t.path().join("p");
        let lib = t.path().join("lib");
        let symbol = |id: &str| wc_core::Symbol {
            source: String::new(),
            id: id.into(),
            name: "Coil".into(),
            category: String::new(),
            letter_code: "K".into(),
            role: Default::default(),
            graphics: vec![],
            pins: vec![],
            tag_at: Default::default(),
            description: String::new(),
        };
        for id in ["imp.coil", "imp.lamp"] {
            assert!(wc_library::write_symbol(&lib, &symbol(id)).is_ok());
        }
        let mut p = Project::new("t");
        p.meta.libraries.push(wc_core::project::LibrarySource {
            name: "lib".into(),
            kind: "folder".into(),
            location: lib.to_string_lossy().into_owned(),
            reference: None,
        });
        p.pages.push(wc_core::Page::new("1", "1", "Main"));
        assert!(store::save(&dir, &p).is_ok());
        // A copy that cannot be read is taken again, so the import overwrites it.
        let broken = match store::vendor_path(&dir, ItemKind::Symbol, "imp.lamp") {
            Ok(path) => path,
            Err(why) => unreachable!("{why}"),
        };
        assert!(std::fs::create_dir_all(broken.parent().unwrap_or(&dir)).is_ok());
        assert!(std::fs::write(&broken, "not a symbol").is_ok());
        // A folder where the page's staging file goes: the save writes the lockfile with the new
        // pins, then cannot write the page.
        let page_file = dir.join(store::PAGES_DIR).join(format!("1{}", store::PAGE_SUFFIX));
        let mut staging = page_file.into_os_string();
        staging.push(".tmp");
        assert!(std::fs::create_dir_all(&staging).is_ok());
        let before = files(&dir);
        let drawing = t.path().join("page.dxf");
        let ents = "0\nINSERT\n2\nIMP.COIL\n10\n50\n20\n100\n0\nINSERT\n2\nIMP.LAMP\n10\n80\n20\n100\n";
        let text = format!("0\nSECTION\n2\nHEADER\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n{ents}0\nENDSEC\n0\nEOF\n");
        assert!(std::fs::write(&drawing, text).is_ok());
        let mut log = Log::default();
        let r = page_dxf(&dir, &drawing, "1", 1.0, &mut log);
        assert!(r.is_err(), "{log:?}");
        assert_eq!(files(&dir), before);
        // With the page writable again the same import goes through.
        assert!(std::fs::remove_dir(&staging).is_ok());
        assert_eq!(page_dxf(&dir, &drawing, "1", 1.0, &mut log), Ok(()), "{log:?}");
        assert!(store::read_vendored_symbol(&dir, "imp.lamp").is_ok());
        let pins = load(&dir).map(|p| p.lock.symbols.len());
        assert_eq!(pins, Ok(2));
    }

    #[test]
    fn a_dxf_page_error_naming_an_entity_is_escaped_for_the_terminal() {
        let t = tmp();
        let dir = t.path().join("p");
        let mut p = Project::new("t");
        p.pages.push(wc_core::Page::new("1", "1", "Main"));
        assert!(store::save(&dir, &p).is_ok());
        let drawing = t.path().join("page.dxf");
        let text = "0\nSECTION\n2\nENTITIES\n0\nBAD\u{1b}[2J\n8\n0\n0\nENDSEC\n0\nEOF\n";
        assert!(std::fs::write(&drawing, text).is_ok());
        let mut log = Log::default();
        let Err(err) = page_dxf(&dir, &drawing, "1", 1.0, &mut log) else {
            unreachable!("nothing on the page");
        };
        assert!(err.contains("BAD\\u{1b}[2J") && !err.contains('\u{1b}'), "{err}");
    }

    #[test]
    fn an_import_of_a_missing_file_fails_and_leaves_the_project_as_it_was() {
        let t = tmp();
        let dir = t.path().join("p");
        assert!(store::save(&dir, &Project::new("t")).is_ok());
        let before = load(&dir);
        let mut log = Log::default();
        let r = aml(&dir, &t.path().join("missing.aml"), 100, &mut log);
        assert!(r.is_err());
        assert_eq!(load(&dir), before);
    }
}
