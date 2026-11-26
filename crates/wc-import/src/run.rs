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

/// What an import had to say, in order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Log(pub Vec<Msg>);

impl Log {
    /// Add a result line.
    pub fn out(&mut self, line: String) {
        self.0.push(Msg::Out(line));
    }

    /// Add a warning or note line.
    pub fn err(&mut self, line: String) {
        self.0.push(Msg::Err(line));
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
    let mut p = load(dir)?;
    let bytes = read_file(file)?;
    let (rows, warnings) = crate::csv::file_rows(&bytes)
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

/// Parts from a BMEcat catalogue into the library folder `lib` (`import parts-bmecat`).
pub fn parts_bmecat(file: &Path, lib: &Path, log: &mut Log) -> Res<()> {
    let (n, warnings) = import_bmecat(file, lib)?;
    for w in warnings {
        err!(log, "warning: {}", escape_field(&w));
    }
    out!(log, "imported {n} part(s) into {}", lib.display());
    Ok(())
}

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

/// Parts from a CSV list into the library folder `lib` (`import parts-csv`).
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
    out!(log, "imported {} part(s) into {}", parts.len(), lib.display());
    Ok(())
}

/// PDF pages as trace pages numbered from `first_page` (`import pdf`).
pub fn pdf(dir: &Path, file: &Path, first_page: usize, log: &mut Log) -> Res<()> {
    let source = file
        .file_name()
        .map(|n| escape_field(&n.to_string_lossy()))
        .unwrap_or_default();
    import_pdf(dir, file, first_page, &source, log)?;
    Ok(())
}

/// An Eplan Electric P8 project (.elk, .edb folder or zip) into the project (`import eplan`).
pub fn eplan(dir: &Path, path: &Path, first_page: usize, log: &mut Log) -> Res<()> {
    let mut p = load(dir)?;
    let tables = eplan_tables(path)?;
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
        let file = std::io::BufReader::new(std::fs::File::open(path).map_err(e)?);
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
        let file = std::io::BufReader::new(std::fs::File::open(path).map_err(e)?);
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
        let file = std::io::BufReader::new(std::fs::File::open(path).map_err(e)?);
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
    let bytes = read_file(file)?;
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
            Ok(symbols) if symbols.is_empty() => continue,
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
                next += n.max(1);
                r
            }
            Kind::Dxf => {
                let mut p = load(dir)?;
                let id = format!("dxf-{next}");
                next += 1;
                // Page files are named by id, and some folders do not tell `DXF-300`
                // from `dxf-300`.
                if p.pages.iter().any(|pg| pg.id.eq_ignore_ascii_case(&id)) {
                    // Never added over an earlier page: removing it again on failure
                    // would delete that page.
                    Err(format!(
                        "page {id} exists; remove it or import with --first-page to number the pages from a free number"
                    ))
                } else {
                    p.pages.push(Page::new(id.clone(), (next - 1).to_string(), raw.clone()));
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
                let r = aml(dir, &path, next, log);
                next += 10;
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
    let mut p = load(dir)?;
    let lib = wc_library::Library::open(dir, &p).map_err(e)?;
    let known: std::collections::BTreeSet<String> = lib.index(ItemKind::Symbol).into_keys().collect();
    let text = read_file(file)?;
    let imported = crate::dxf::page(&String::from_utf8_lossy(&text), scale, &known).map_err(e)?;
    for w in &imported.warnings {
        err!(log, "warning: {}", escape_field(w));
    }
    // The page must be there before anything is taken (a retry would skip the copies).
    page_mut(&mut p, page)?;
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
    // What is in the vendor folder now, so a write that fails part way leaves no copy
    // without its pin (a retry would skip a readable one).
    let before: Vec<(PathBuf, Option<String>)> = needed
        .iter()
        .map(|id| {
            let path = store::vendor_path(dir, ItemKind::Symbol, id).map_err(e)?;
            let was = match std::fs::read_to_string(&path) {
                Ok(text) => Some(text),
                Err(x) if x.kind() == std::io::ErrorKind::NotFound => None,
                Err(x) => return Err(format!("{}: {x}", path.display())),
            };
            Ok((path, was))
        })
        .collect::<Res<_>>()?;
    let put_back = |why: String| -> String {
        let failed: Vec<String> = before
            .iter()
            .filter_map(|(path, was)| {
                // Written only when it differs, and whole or not at all.
                let r = match was {
                    Some(text) => store::write_if_changed(path, text)
                        .map(|_| ())
                        .map_err(|x| x.to_string()),
                    None => std::fs::remove_file(path).or_else(|x| {
                        if x.kind() == std::io::ErrorKind::NotFound {
                            Ok(())
                        } else {
                            Err(format!("{}: {x}", path.display()))
                        }
                    }),
                };
                r.err()
            })
            .collect();
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
    save(dir, &p)?;
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

/// Save a project whose jumpers already follow their terminals: after `add_built`, which
/// moves the project's own jumpers itself and adds the imported ones with their final numbers.
pub fn save_imported(dir: &Path, p: &Project) -> Res<()> {
    store::save(dir, p).map(|_| ()).map_err(e)
}

/// `import pdf`: the pages of `file` as trace pages numbered from `first_page`. `source` names
/// the PDF in warnings (the file name, or the entry name inside an archive, where `file` is a
/// staging file), already escaped with [`escape_field`] by the caller.
pub fn import_pdf(dir: &Path, file: &Path, first_page: usize, source: &str, log: &mut Log) -> Result<(), String> {
    let mut p = load(dir)?;
    let bytes = read_file(file)?;
    let traced = crate::pdf::pages(&bytes).map_err(e)?;
    let stem = file
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for (i, t) in traced.into_iter().enumerate() {
        let id = format!("pdf-{}", first_page + i);
        if p.page(&id).is_some() {
            return Err(format!("page {id} exists; pick another --first-page"));
        }
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
                std::fs::write(dir.join(&file), &scan.raster.bytes).map_err(e)?;
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
    save(dir, &p)?;
    Ok(())
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

/// `import page-dwg`: convert `file` with the converter (`template`, see [`dwg_to_dxf`]) and
/// import the DXF onto `page`. The converter writes into a private folder of its own, removed
/// with everything in it when this ends, also on failure.
pub fn import_page_dwg(
    dir: &Path,
    file: &Path,
    page: &str,
    scale: f64,
    template: Option<String>,
    log: &mut Log,
) -> Res<()> {
    let tmp = staging_dir("wiringcraft-dwg-").map_err(e)?;
    let dxf = dwg_to_dxf(file, tmp.path(), template)?;
    page_dxf(dir, &dxf, page, scale, log)
}

/// Convert DWG to a DXF in `tmp` (a private folder, see [`staging_dir`]) with whatever
/// converter is installed: the command `template` (the WIRINGCRAFT_DWG2DXF setting, with `{in}`
/// and `{out}`) when given, otherwise `dwg2dxf` or the ODA File Converter. DWG is a closed
/// format; WiringCraft does not read it itself.
pub fn dwg_to_dxf(file: &Path, tmp: &Path, template: Option<String>) -> Res<PathBuf> {
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
                return Ok(out);
            }
        }
        return Err("WIRINGCRAFT_DWG2DXF did not produce a DXF".into());
    }
    let mut c = Command::new("dwg2dxf");
    c.args(["-y", "-o", &output, &input]);
    if ran(c) && out.exists() {
        return Ok(out);
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
            return Ok(out);
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
    let (symbols, warnings) = crate::kicad::symbols(text, prefix, scale).map_err(e)?;
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
            imp.kicad_name,
            path.display(),
            imp.symbol.pins.len(),
            imp.symbol.graphics.len()
        );
    }
    Ok(n)
}

/// The tables of an Eplan project given as an .elk file, an .edb folder or a zip of the folder.
pub fn eplan_tables(path: &Path) -> Result<crate::eplan::Tables, String> {
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
    for entry in std::fs::read_dir(&folder).map_err(|x| format!("{}: {x}", folder.display()))? {
        let entry = entry.map_err(|x| format!("{}: {x}", folder.display()))?;
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if crate::eplan::wanted(&name) {
            // A table that is there but cannot be read is not taken for an empty one.
            let path = entry.path();
            let bytes = std::fs::read(&path).map_err(|x| format!("{}: {x}", path.display()))?;
            tables.insert(name, bytes);
        }
    }
    Ok(tables)
}

/// Add imported symbols, parts, pages and devices to a project, pinning symbols and parts in
/// the lockfile under `source`.
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

/// Whether an import may replace a vendored item the project pins in another version.
#[derive(Clone, Copy, PartialEq)]
pub enum Replace {
    /// Never: an Eplan, AML or SolidWorks Electrical project import makes its own symbols, and a
    /// page the project kept may still use the old one's pins.
    No,
    /// When the project's copy is still the pinned one (a library import, #258).
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
/// folders, and the files of the pages and panels about to be added. [`put_files_back`] restores
/// them when a save fails half way.
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
/// one that could not be put back failed.
pub fn put_files_back(files: &[(PathBuf, Option<Vec<u8>>)]) -> Vec<String> {
    files
        .iter()
        .rev()
        .filter_map(|(path, before)| put_file_back(path, before.as_deref()).err())
        .collect()
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
    // an earlier one and then fail half way.
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
    let numbers: std::collections::BTreeSet<&str> = p.pages.iter().map(|pg| pg.number.as_str()).collect();
    if let Some(page) = built.pages.iter().find(|pg| numbers.contains(pg.number.as_str())) {
        return Err(format!(
            "page number {} is taken; import with --first-page to number the pages from a free number",
            page.number
        ));
    }
    // A new version of a symbol must keep every pin a wire in the project ends on.
    for symbol in &built.symbols {
        if p.lock.symbols.get(&symbol.id).is_some_and(|e| e.hash != symbol.hash()) {
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
        if let Some(entry) = p.lock.map(kind).get(id) {
            if replace == Replace::No && entry.hash != hash {
                return Err(format!("{name} exists with other content; import into a fresh project"));
            }
            let have = match (&before, kind) {
                (None, _) => None,
                (Some(_), ItemKind::Symbol) => Some(store::read_vendored_symbol(dir, id).map(|x| x.hash())),
                (Some(_), ItemKind::Part) => Some(store::read_vendored_part(dir, id).map(|x| x.hash())),
            };
            match have {
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
        }
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
        // A symbol the project has no readable copy of, which the import brings, changes which
        // placed items are terminals. Whether the jumpers' and rail order's numbers were given
        // with it or without it cannot be told, so a strip it renumbers is not guessed at.
        let (own, _) = store::vendored_symbols(dir, p);
        let mut symbols = own.clone();
        for s in &built.symbols {
            symbols.entry(s.id.clone()).or_insert_with(|| s.clone());
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
            let missing: Vec<&str> = built
                .symbols
                .iter()
                .filter(|s| !own.contains_key(&s.id))
                .map(|s| s.id.as_str())
                .filter(|id| p.pages.iter().any(|pg| pg.symbols.iter().any(|ps| ps.symbol == *id)))
                .collect();
            return Err(format!(
                "the jumpers and rail order of {strip} cannot be kept on their terminals: the project has no readable copy of {} that the import brings, so its terminal numbers are unclear; restore the file (for example from the project's history) and import again",
                missing.join(", ")
            ));
        }
        Some(with)
    };
    // A write that fails puts back the files written before it (and removes the ones it made),
    // so each copy still matches its pin. Only files this import changed are put back: one it
    // left as it was keeps whatever it holds now.
    let writes = built
        .symbols
        .iter()
        .map(|s| store::write_vendored_symbol(dir, s))
        .chain(built.parts.iter().map(|x| store::write_vendored_part(dir, x)));
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

/// The path and bytes of a vendored copy, kept before an import replaces it.
/// The vendored file of an item and its bytes, none when there is no such file.
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
    // The staging file is made new (never one already there, which could be a link to a file
    // elsewhere), and only that file is removed when something fails.
    let mut made = None;
    for n in 0..100u32 {
        let mut part = path.as_os_str().to_owned();
        part.push(format!(".restore-{}-{n}", std::process::id()));
        let part = PathBuf::from(part);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&part) {
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
    let done = file.write_all(bytes).and_then(|()| file.sync_all()).and_then(|()| {
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
