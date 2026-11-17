//! Project archives from other electrical CAD tools (`.tewzip`, `.zw1`, `.elk`, plain zip).
//!
//! These archives are closed formats. What WiringCraft can do without vendor documentation is
//! open the container, recognise the open formats inside it (PDF, DXF, AutomationML/XML, CSV,
//! images) and import those, and identify the closed parts (databases, binary drawings) so the
//! user knows what is and is not carried over. [`inspect`] gives that report; [`open_entries`]
//! extracts the importable entries.

use std::io::{Cursor, Read};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Pdf,
    Dxf,
    Dwg,
    AutomationMl,
    /// A KiCad symbol library (`.kicad_sym`).
    KicadSymbols,
    Xml,
    Csv,
    Spreadsheet,
    SqliteDb,
    AccessDb,
    SqlServerData,
    Image,
    Archive,
    Other,
}

impl Kind {
    pub fn importable(self) -> bool {
        matches!(
            self,
            Kind::Pdf | Kind::Dxf | Kind::AutomationMl | Kind::Csv | Kind::KicadSymbols
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Pdf => "PDF (importable as trace pages)",
            Kind::Dxf => "DXF drawing (importable)",
            Kind::Dwg => "DWG drawing (needs a converter)",
            Kind::AutomationMl => "AutomationML (importable)",
            Kind::KicadSymbols => "KiCad symbol library (importable into a library)",
            Kind::Xml => "XML (vendor data)",
            Kind::Csv => "CSV list (importable: parts list into a library, or PLC addresses)",
            Kind::Spreadsheet => "spreadsheet",
            Kind::SqliteDb => "SQLite database (vendor project data)",
            Kind::AccessDb => "Access database (vendor project data)",
            Kind::SqlServerData => "SQL Server data/backup (vendor project data)",
            Kind::Image => "image",
            Kind::Archive => "nested archive",
            Kind::Other => "other",
        }
    }
}

/// Recognise an entry by its first bytes, then by its name.
pub fn classify(name: &str, head: &[u8]) -> Kind {
    let lower = name.to_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    if head.starts_with(b"%PDF") {
        return Kind::Pdf;
    }
    if head.starts_with(b"AC10") {
        return Kind::Dwg;
    }
    if head.starts_with(b"SQLite format 3") {
        return Kind::SqliteDb;
    }
    if head.len() > 20 && (head[4..].starts_with(b"Standard Jet DB") || head[4..].starts_with(b"Standard ACE DB")) {
        return Kind::AccessDb;
    }
    if head.starts_with(b"TAPE") || matches!(ext, "bak" | "mdf" | "ldf") {
        return Kind::SqlServerData;
    }
    if head.starts_with(b"PK\x03\x04") {
        return if matches!(ext, "xlsx" | "xlsm") {
            Kind::Spreadsheet
        } else {
            Kind::Archive
        };
    }
    if head.starts_with(b"\x89PNG") || head.starts_with(b"\xFF\xD8\xFF") || head.starts_with(b"BM") {
        return Kind::Image;
    }
    let text = String::from_utf8_lossy(&head[..head.len().min(512)]);
    // Before the AutomationML test: a KiCad description may quote anything.
    if ext == "kicad_sym" || text.trim_start().starts_with("(kicad_symbol_lib") {
        return Kind::KicadSymbols;
    }
    if text.contains("<CAEXFile") || ext == "aml" {
        return Kind::AutomationMl;
    }
    if ext == "dxf" || text.trim_start().starts_with("0\r\nSECTION") || text.trim_start().starts_with("0\nSECTION") {
        return Kind::Dxf;
    }
    match ext {
        "xml" => Kind::Xml,
        "csv" | "txt" if text.contains(';') || text.contains(',') => Kind::Csv,
        "xls" => Kind::Spreadsheet,
        "zip" | "7z" | "rar" => Kind::Archive,
        _ if text.trim_start().starts_with("<?xml") => Kind::Xml,
        _ => Kind::Other,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// `zip`, or a description of an unrecognised container.
    pub container: String,
    pub entries: Vec<Entry>,
}

const HEAD: usize = 4096;
/// Largest single entry extracted into memory.
const MAX_ENTRY: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 200_000;

fn container_of(bytes: &[u8]) -> String {
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        "zip".into()
    } else if bytes.starts_with(b"7z\xBC\xAF\x27\x1C") {
        "7z".into()
    } else if bytes.starts_with(&[0x1f, 0x8b]) {
        "gzip".into()
    } else {
        let hex: String = bytes.iter().take(8).map(|b| format!("{b:02x}")).collect();
        format!("unknown (starts with {hex})")
    }
}

/// List an archive's contents with what each entry is.
pub fn inspect(bytes: &[u8]) -> Result<Report> {
    let container = container_of(bytes);
    if container != "zip" {
        return Ok(Report {
            container,
            entries: Vec::new(),
        });
    }
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| Error::Format(format!("zip: {e}")))?;
    let mut entries = Vec::new();
    for i in 0..z.len().min(MAX_ENTRIES) {
        let Ok(mut f) = z.by_index(i) else { continue };
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_owned();
        let size = f.size();
        let mut head = Vec::with_capacity(HEAD);
        let _ = (&mut f).take(HEAD as u64).read_to_end(&mut head);
        entries.push(Entry {
            kind: classify(&name, &head),
            name,
            size,
        });
    }
    entries.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
    Ok(Report { container, entries })
}

/// Extract the entries of the given kinds (each at most 256 MB), in name order.
pub fn open_entries(bytes: &[u8], kinds: &[Kind]) -> Result<Vec<(String, Kind, Vec<u8>)>> {
    let report = inspect(bytes)?;
    if report.container != "zip" {
        return Err(Error::Format(format!("cannot open a {} container", report.container)));
    }
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| Error::Format(format!("zip: {e}")))?;
    let mut out = Vec::new();
    for e in report.entries.iter().filter(|e| kinds.contains(&e.kind)) {
        if e.size > MAX_ENTRY {
            continue;
        }
        let Ok(f) = z.by_name(&e.name) else { continue };
        let mut data = Vec::new();
        if f.take(MAX_ENTRY).read_to_end(&mut data).is_ok() {
            out.push((e.name.clone(), e.kind, data));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            for (name, data) in files {
                let opts = zip::write::SimpleFileOptions::default();
                if w.start_file(*name, opts).is_ok() {
                    let _ = w.write_all(data);
                }
            }
            let _ = w.finish();
        }
        buf.into_inner()
    }

    #[test]
    fn classifies_and_extracts() -> Result<()> {
        let z = make_zip(&[
            ("project/pages/1.pdf", b"%PDF-1.7\n..."),
            ("project/data.mdf", b"\x01\x0f\x00\x00"),
            (
                "project/symbols/coil.dxf",
                b"0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0\nEOF\n",
            ),
            ("export.aml", b"<?xml version=\"1.0\"?><CAEXFile/>"),
            ("cache.db", b"SQLite format 3\0..."),
            ("readme.bin", b"\x00\x01"),
            ("lib/Relays.kicad_sym", b"(kicad_symbol_lib (version 20231120))"),
            ("lib/unnamed", b"  (kicad_symbol_lib)"),
            (
                "lib/quoting",
                b"(kicad_symbol_lib (symbol \"x\" (property \"Description\" \"reads <CAEXFile>\")))",
            ),
        ]);
        let r = inspect(&z)?;
        assert_eq!(r.container, "zip");
        let kind = |n: &str| r.entries.iter().find(|e| e.name.ends_with(n)).map(|e| e.kind);
        assert_eq!(kind("1.pdf"), Some(Kind::Pdf));
        assert_eq!(kind("data.mdf"), Some(Kind::SqlServerData));
        assert_eq!(kind("coil.dxf"), Some(Kind::Dxf));
        assert_eq!(kind("export.aml"), Some(Kind::AutomationMl));
        assert_eq!(kind("cache.db"), Some(Kind::SqliteDb));
        assert_eq!(kind("Relays.kicad_sym"), Some(Kind::KicadSymbols));
        assert_eq!(kind("unnamed"), Some(Kind::KicadSymbols));
        assert_eq!(kind("quoting"), Some(Kind::KicadSymbols));
        assert!(Kind::KicadSymbols.importable());
        let got = open_entries(&z, &[Kind::Pdf, Kind::Dxf])?;
        assert_eq!(got.len(), 2);
        Ok(())
    }

    #[test]
    fn unknown_containers_are_reported_not_failed() -> Result<()> {
        let r = inspect(b"\x12\x34\x56\x78rest")?;
        assert!(r.container.starts_with("unknown"));
        assert!(open_entries(b"\x12\x34", &[Kind::Pdf]).is_err());
        assert!(inspect(b"PK\x03\x04garbage").is_err());
        Ok(())
    }
}
