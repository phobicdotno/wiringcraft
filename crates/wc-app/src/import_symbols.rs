//! Library → Import symbols…: DXF, SVG, KiCad (`.kicad_sym`) and QElectroTech (`.elmt`) files
//! into a library folder, picked by extension, the same way as `wiringcraft import symbol-*`.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The file extensions the import takes, for the file picker's filter.
pub const EXTENSIONS: &[&str] = &["dxf", "svg", "kicad_sym", "lib", "elmt"];

/// The largest file read; a bigger one fails without being read whole.
const MAX_LEN: usize = 16 << 20;

/// The folder the project's own library lives in, relative to the project.
pub const PROJECT_LIBRARY: &str = "library";

/// A library folder the import can write into.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportTarget {
    pub name: String,
    pub root: PathBuf,
    /// For the project's own folder while the project does not list it yet: the source to add
    /// (first, so it wins) once symbols are written there.
    pub declare: Option<wc_core::project::LibrarySource>,
}

impl ImportTarget {
    pub fn label(&self) -> String {
        if self.declare.is_some() {
            format!("{} ({}, new)", self.name, self.root.display())
        } else {
            format!("{} ({})", self.name, self.root.display())
        }
    }
}

/// Where symbols can be imported: the project's own library folder (`library/` in the project)
/// first, then the project's other library folders (`folders`, as (name, root) in the project's
/// order). The own folder is offered even when the project does not list it yet; it is then
/// named so that it shares no name with any declared source (`taken`, git sources too), since
/// library proposals find their destination by name.
pub fn targets(project_dir: &Path, folders: Vec<(String, PathBuf)>, taken: &[String]) -> Vec<ImportTarget> {
    let own = project_dir.join(PROJECT_LIBRARY);
    let same = |root: &Path| {
        root == own
            || matches!(
                (root.canonicalize(), own.canonicalize()),
                (Ok(a), Ok(b)) if a == b
            )
    };
    let mut out: Vec<ImportTarget> = Vec::new();
    if let Some((name, root)) = folders.iter().find(|(_, r)| same(r)) {
        out.push(ImportTarget {
            name: name.clone(),
            root: root.clone(),
            declare: None,
        });
    } else {
        let mut name = "project".to_owned();
        while folders.iter().any(|(n, _)| *n == name) || taken.contains(&name) {
            name.push('_');
        }
        out.push(ImportTarget {
            name: name.clone(),
            root: own.clone(),
            declare: Some(wc_core::project::LibrarySource {
                name,
                kind: "folder".into(),
                location: PROJECT_LIBRARY.into(),
                reference: None,
            }),
        });
    }
    out.extend(
        folders
            .into_iter()
            .filter(|(_, r)| !same(r))
            .map(|(name, root)| ImportTarget {
                name,
                root,
                declare: None,
            }),
    );
    out
}

/// What an import did: every symbol written, every warning and every file that failed.
#[derive(Debug, Default)]
pub struct Outcome {
    /// One line per symbol written: the source name, the file written, pins and shapes.
    pub written: Vec<String>,
    pub warnings: Vec<String>,
    /// One line per file (or symbol) that could not be imported, with the reason.
    pub failed: Vec<String>,
}

impl Outcome {
    /// The result as text for a message dialog.
    pub fn report(&self, lib: &Path) -> String {
        let mut s = format!("Imported {} symbols into\n{}", self.written.len(), lib.display());
        if !self.failed.is_empty() {
            s.push_str(&format!(", {} failed", self.failed.len()));
        }
        s.push('.');
        for (head, lines) in [
            ("Written", &self.written),
            ("Failed", &self.failed),
            ("Warnings", &self.warnings),
        ] {
            if !lines.is_empty() {
                s.push_str(&format!("\n\n{head}:"));
                for l in lines {
                    s.push_str("\n  ");
                    s.push_str(l);
                }
            }
        }
        s
    }
}

/// Read a file as text, at most [`MAX_LEN`] bytes, invalid UTF-8 replaced.
fn read_text(file: &Path, max: usize) -> Result<String, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .and_then(|f| f.take(max as u64 + 1).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    if bytes.len() > max {
        return Err(format!("larger than {} MB", max >> 20));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// A file's name without its extension as an id segment: `Lamp Red.elmt` is `lamp_red`.
fn stem(file: &Path) -> String {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // `.kicad_sym` counts as one extension, in any case.
    let stem = match extension(file) {
        Some(ext) => name.get(..name.len().saturating_sub(ext.len() + 1)).unwrap_or(&name),
        None => name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem),
    };
    wc_import::id_segment(stem)
}

/// The import format of a file, by its extension (any case).
fn extension(file: &Path) -> Option<&'static str> {
    let name = file.file_name()?.to_string_lossy().to_ascii_lowercase();
    EXTENSIONS
        .iter()
        .copied()
        .find(|ext| name.ends_with(&format!(".{ext}")))
}

/// A file's symbols, each with the name it has in the file, and the file's warnings.
type FileSymbols = (Vec<(wc_core::Symbol, String)>, Vec<String>);

/// Read one file's symbols; nothing is written yet.
fn read(file: &Path) -> Result<FileSymbols, String> {
    let display_name = file
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match extension(file) {
        Some("dxf") => {
            let text = read_text(file, MAX_LEN)?;
            let id = format!("dxf.{}", stem(file));
            let (s, w) = wc_import::dxf::symbol_with_warnings(&text, &id, &display_name).map_err(|e| e.to_string())?;
            Ok((vec![(s, display_name)], w))
        }
        Some("svg") => {
            let text = read_text(file, MAX_LEN)?;
            let id = format!("svg.{}", stem(file));
            let (s, w) = wc_import::svg::symbol_with_warnings(&text, &id, &display_name).map_err(|e| e.to_string())?;
            Ok((vec![(s, display_name)], w))
        }
        // `.lib`: an old KiCad library (KiCad 5 and before); another kind of `.lib` is refused.
        Some("kicad_sym" | "lib") => {
            let text = read_text(file, MAX_LEN)?;
            let prefix = format!("kicad.{}", stem(file));
            let (symbols, w) = wc_import::kicad::symbols(&text, &prefix, 1.0).map_err(|e| e.to_string())?;
            Ok((symbols.into_iter().map(|i| (i.symbol, i.kicad_name)).collect(), w))
        }
        Some("elmt") => {
            let text = read_text(file, wc_import::qet::MAX_LEN)?;
            let id = format!("qet.{}", stem(file));
            let (i, w) =
                wc_import::qet::element(&text, &id, wc_import::qet::DEFAULT_SCALE).map_err(|e| e.to_string())?;
            Ok((vec![(i.symbol, i.qet_name)], w))
        }
        _ => Err(format!(
            "not a symbol file this import reads ({})",
            EXTENSIONS.join(", ")
        )),
    }
}

/// Import symbol files into the library folder `lib`, each by its extension. Ids are
/// `dxf.<file name>`, `svg.<file name>`, `kicad.<file name>.<symbol>` and `qet.<file name>`.
/// A file that fails is listed and the others still go in; a symbol whose id an earlier file in
/// the same import already wrote is refused instead of overwriting it. A symbol already in the
/// library is left as it is unless `replace` is set, since other projects may use it.
pub fn import(files: &[PathBuf], lib: &Path, replace: bool) -> Outcome {
    let mut out = Outcome::default();
    let mut ids: BTreeMap<String, PathBuf> = BTreeMap::new();
    for file in files {
        let shown = file.display();
        let (symbols, warnings) = match read(file) {
            Ok(r) => r,
            Err(e) => {
                out.failed.push(format!("{shown}: {e}"));
                continue;
            }
        };
        out.warnings
            .extend(warnings.into_iter().map(|w| format!("{shown}: {w}")));
        for (symbol, name) in symbols {
            if let Some(first) = ids.get(&symbol.id) {
                out.failed.push(format!(
                    "{shown}: {name} gives the same id {} as {}, skipped",
                    symbol.id,
                    first.display()
                ));
                continue;
            }
            let existing = lib.join("symbols").join(format!("{}.json", symbol.id));
            if !replace && existing.exists() {
                out.failed.push(format!(
                    "{shown}: {name}: {} is already in the library, kept (tick \"Replace symbols already in \
                     the library\" to overwrite it)",
                    symbol.id
                ));
                continue;
            }
            match wc_library::write_symbol(lib, &symbol) {
                Ok(path) => {
                    ids.insert(symbol.id.clone(), file.clone());
                    out.written.push(format!(
                        "{name} -> {} ({} pins, {} shapes)",
                        path.display(),
                        symbol.pins.len(),
                        symbol.graphics.len()
                    ));
                }
                Err(e) => out.failed.push(format!("{shown}: {name}: {e}")),
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20">
        <rect x="5" y="5" width="10" height="10"/>
        <circle id="pin:A1" cx="10" cy="0" r="1"/>
        <circle id="pin:A2" cx="10" cy="20" r="1"/>
    </svg>"#;

    const KICAD: &str = r#"(kicad_symbol_lib
        (symbol "K" (symbol "K_1_1"
            (rectangle (start -2 2) (end 2 -2))
            (pin passive line (at 0 3 270) (length 1) (name "A1") (number "A1"))
            (pin passive line (at 0 -3 90) (length 1) (name "A2") (number "A2")))))"#;

    const ELMT: &str = r#"<definition type="element" width="20" height="20" hotspot_x="10" hotspot_y="10">
        <names><name lang="en">Lamp red</name></names>
        <description>
            <circle x="-5" y="-5" diameter="10"/>
            <terminal x="0" y="-10" orientation="n"/>
            <terminal x="0" y="10" orientation="s"/>
        </description>
    </definition>"#;

    const DXF: &str = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0\n20\n0\n11\n10\n21\n0\n0\nPOINT\n8\nPIN\n10\n0\n20\n0\n0\nPOINT\n8\nPIN\n10\n10\n20\n0\n0\nENDSEC\n0\nEOF\n";

    const OLD_KICAD: &str = "EESchema-LIBRARY Version 2.4\nDEF LAMP H 0 40 Y Y 1 L N\nF0 \"H\" 0 100 50 H V C CNN\nDRAW\nX 1 1 0 0 100 R 50 50 1 1 P\nENDDRAW\nENDDEF\n";

    #[test]
    fn a_mixed_set_of_files_goes_into_the_library_and_a_bad_one_does_not_stop_the_rest() {
        let src = tempfile::tempdir().unwrap();
        let lib = tempfile::tempdir().unwrap();
        let write = |name: &str, text: &str| {
            let p = src.path().join(name);
            std::fs::write(&p, text).unwrap();
            p
        };
        let files = vec![
            write("Coil.svg", SVG),
            write("broken.svg", "<svg"),
            write("Relays.KICAD_SYM", KICAD),
            // A derived symbol whose parent is missing: a warning, the file still imports.
            write(
                "orphans.kicad_sym",
                r#"(kicad_symbol_lib (symbol "Orphan" (extends "Missing")))"#,
            ),
            write("Lamp Red.elmt", ELMT),
            // An old KiCad library (before KiCad 6), read as the new format.
            write("Old Lamps.lib", OLD_KICAD),
            write("Line.dxf", DXF),
            write("notes.txt", "hello"),
            // Same id as Coil.svg: refused, the first one stays.
            write("coil.svg", SVG),
        ];
        let out = import(&files, lib.path(), false);
        let symbols = lib.path().join("symbols");
        for id in [
            "svg.coil",
            "kicad.relays.k",
            "kicad.old_lamps.lamp",
            "qet.lamp_red",
            "dxf.line",
        ] {
            assert!(symbols.join(format!("{id}.json")).is_file(), "{id}: {out:?}");
        }
        assert_eq!(out.written.len(), 5, "{out:?}");
        assert_eq!(out.failed.len(), 3, "{out:?}");
        assert!(out.failed.iter().any(|f| f.contains("broken.svg")));
        assert!(
            out.failed
                .iter()
                .any(|f| f.contains("notes.txt") && f.contains("kicad_sym"))
        );
        assert!(out.failed.iter().any(|f| f.contains("same id svg.coil")));
        let report = out.report(lib.path());
        assert!(report.starts_with("Imported 5 symbols"), "{report}");
        // The warning names its file, in the outcome and in the report.
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("orphans.kicad_sym") && w.contains("Missing")),
            "{out:?}"
        );
        assert!(
            report.contains("Warnings:") && report.contains("orphans.kicad_sym"),
            "{report}"
        );
        assert!(report.contains("3 failed"), "{report}");
    }

    #[test]
    fn file_names_give_ids_and_formats() {
        assert_eq!(stem(Path::new("a/Std Relays.v2.kicad_sym")), "std_relays_v2");
        assert_eq!(stem(Path::new("Lamp Red.elmt")), "lamp_red");
        assert_eq!(stem(Path::new("Relays.KICAD_SYM")), "relays");
        assert_eq!(extension(Path::new("X.DXF")), Some("dxf"));
        assert_eq!(extension(Path::new("x.kicad_sym")), Some("kicad_sym"));
        assert_eq!(extension(Path::new("x.sym")), None);
    }

    #[test]
    fn a_symbol_already_in_the_library_is_kept_unless_replace_is_ticked() {
        let src = tempfile::tempdir().unwrap();
        let lib = tempfile::tempdir().unwrap();
        let file = src.path().join("Coil.svg");
        std::fs::write(&file, SVG).unwrap();
        let path = lib.path().join("symbols").join("svg.coil.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{\"mine\": true}").unwrap();
        let out = import(std::slice::from_ref(&file), lib.path(), false);
        assert!(out.written.is_empty(), "{out:?}");
        assert!(out.failed[0].contains("already in the library"), "{out:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"mine\": true}");
        let out = import(&[file], lib.path(), true);
        assert_eq!(out.written.len(), 1, "{out:?}");
        assert!(std::fs::read_to_string(&path).unwrap().contains("svg.coil"));
    }

    #[test]
    fn the_projects_own_library_is_offered_first() {
        let dir = tempfile::tempdir().unwrap();
        let starter = ("starter".to_owned(), PathBuf::from("/opt/wc/library"));
        // A new project lists only the starter library: its own folder comes first, to be added.
        let t = targets(dir.path(), vec![starter.clone()], &[]);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].root, dir.path().join("library"));
        let source = t[0].declare.as_ref().unwrap();
        assert_eq!((source.name.as_str(), source.location.as_str()), ("project", "library"));
        assert_eq!(t[1].name, "starter");
        assert!(t[1].declare.is_none());
        // Once listed (anywhere), it is first and not added again.
        let own = ("mine".to_owned(), dir.path().join("library"));
        let t = targets(dir.path(), vec![starter.clone(), own], &[]);
        assert_eq!(t[0].name, "mine");
        assert!(t[0].declare.is_none());
        assert_eq!(t.len(), 2);
        // A source already called `project` elsewhere keeps its name.
        let t = targets(dir.path(), vec![("project".to_owned(), PathBuf::from("/x"))], &[]);
        assert_eq!(t[0].name, "project_");
        // So does a git source called `project`, which is not offered as a target itself.
        let t = targets(dir.path(), vec![starter.clone()], &["starter".into(), "project".into()]);
        assert_eq!(t[0].name, "project_");
        assert_eq!(t[0].declare.as_ref().unwrap().name, "project_");
        assert_eq!(t.len(), 2);
    }
}
