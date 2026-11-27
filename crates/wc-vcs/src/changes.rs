//! What changed between two versions, in schematic terms.

use std::collections::BTreeMap;

use wc_core::Page;

use crate::{Git, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    PageAdded {
        page: String,
        title: String,
    },
    PageRemoved {
        page: String,
        title: String,
    },
    PageRenamed {
        page: String,
        from: String,
        to: String,
    },
    /// The page's display number changed (`page` is the new one).
    PageRenumbered {
        page: String,
        from: String,
    },
    /// A setting of the page itself changed: `what` names it (sheet size, title block, ...).
    PageSettingChanged {
        page: String,
        what: String,
    },
    SymbolAdded {
        page: String,
        item: String,
        what: String,
    },
    SymbolRemoved {
        page: String,
        item: String,
        what: String,
    },
    SymbolMoved {
        page: String,
        item: String,
        what: String,
    },
    SymbolRetagged {
        page: String,
        item: String,
        from: String,
        to: String,
    },
    /// The placed symbol kept its id but now draws another library symbol (`from` -> `to`),
    /// for example a make contact turned into a break contact.
    SymbolReplaced {
        page: String,
        item: String,
        from: String,
        to: String,
    },
    WireAdded {
        page: String,
        wire: String,
    },
    WireRemoved {
        page: String,
        wire: String,
    },
    WireChanged {
        page: String,
        wire: String,
    },
    GraphicsChanged {
        page: String,
    },
    /// Any other file: devices, library pins, project settings.
    File {
        path: String,
        status: String,
    },
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Change::PageAdded { page, title } => write!(f, "page {page} added ({title})"),
            Change::PageRemoved { page, title } => write!(f, "page {page} removed ({title})"),
            Change::PageRenamed { page, from, to } => write!(f, "page {page} renamed {from:?} -> {to:?}"),
            Change::PageRenumbered { page, from } => write!(f, "page {from} renumbered -> {page}"),
            Change::PageSettingChanged { page, what } => write!(f, "page {page}: {what} changed"),
            Change::SymbolAdded { page, item, what } => write!(f, "page {page}: {what} {item} added"),
            Change::SymbolRemoved { page, item, what } => write!(f, "page {page}: {what} {item} removed"),
            Change::SymbolMoved { page, item, what } => write!(f, "page {page}: {what} {item} moved"),
            Change::SymbolRetagged { page, item, from, to } => write!(f, "page {page}: {item} tag {from} -> {to}"),
            Change::SymbolReplaced { page, item, from, to } => {
                write!(f, "page {page}: {item} symbol {from} -> {to}")
            }
            Change::WireAdded { page, wire } => write!(f, "page {page}: wire {wire} added"),
            Change::WireRemoved { page, wire } => write!(f, "page {page}: wire {wire} removed"),
            Change::WireChanged { page, wire } => write!(f, "page {page}: wire {wire} changed"),
            Change::GraphicsChanged { page } => write!(f, "page {page}: graphics changed"),
            Change::File { path, status } => write!(f, "{path} {status}"),
        }
    }
}

/// One side of a page file: `Ok(None)` when the file is not there, `Err` (saying why) when it
/// is there but cannot be read as a page.
fn read_side(path: &str, text: Option<String>) -> wc_core::Result<Option<Page>> {
    match text {
        None => Ok(None),
        Some(t) => serde_json::from_str(&t)
            .map(Some)
            .map_err(|source| wc_core::Error::Json {
                path: path.into(),
                source,
            }),
    }
}

/// A file in the working folder: `Ok(None)` only when it is not there; any other failure to
/// read it (no permission, not UTF-8) is an error, since the file still exists.
fn read_working(path: &std::path::Path) -> wc_core::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(wc_core::Error::io(path, e)),
    }
}

fn tag_or_symbol(s: &wc_core::PlacedSymbol) -> String {
    s.tag
        .as_ref()
        .map(|t| t.to_string())
        .unwrap_or_else(|| s.symbol.clone())
}

/// Compare two pages of the same id.
pub fn page_changes(old: Option<&Page>, new: Option<&Page>) -> Vec<Change> {
    let mut out = Vec::new();
    match (old, new) {
        (None, Some(n)) => out.push(Change::PageAdded {
            page: n.number.clone(),
            title: n.title.clone(),
        }),
        (Some(o), None) => out.push(Change::PageRemoved {
            page: o.number.clone(),
            title: o.title.clone(),
        }),
        (Some(o), Some(n)) => {
            let page = n.number.clone();
            if o.number != n.number {
                out.push(Change::PageRenumbered {
                    page: page.clone(),
                    from: o.number.clone(),
                });
            }
            if o.title != n.title {
                out.push(Change::PageRenamed {
                    page: page.clone(),
                    from: o.title.clone(),
                    to: n.title.clone(),
                });
            }
            let os: BTreeMap<&str, _> = o.symbols.iter().map(|s| (s.id.as_str(), s)).collect();
            let ns: BTreeMap<&str, _> = n.symbols.iter().map(|s| (s.id.as_str(), s)).collect();
            for (id, s) in &ns {
                match os.get(id) {
                    None => out.push(Change::SymbolAdded {
                        page: page.clone(),
                        item: tag_or_symbol(s),
                        what: s.symbol.clone(),
                    }),
                    Some(prev) => {
                        if prev.tag != s.tag {
                            out.push(Change::SymbolRetagged {
                                page: page.clone(),
                                item: s.id.clone(),
                                from: prev.tag.as_ref().map(|t| t.to_string()).unwrap_or_default(),
                                to: s.tag.as_ref().map(|t| t.to_string()).unwrap_or_default(),
                            });
                        }
                        if prev.symbol != s.symbol {
                            out.push(Change::SymbolReplaced {
                                page: page.clone(),
                                item: tag_or_symbol(s),
                                from: prev.symbol.clone(),
                                to: s.symbol.clone(),
                            });
                        }
                        if prev.at != s.at || prev.rotation != s.rotation || prev.mirror != s.mirror {
                            out.push(Change::SymbolMoved {
                                page: page.clone(),
                                item: tag_or_symbol(s),
                                what: s.symbol.clone(),
                            });
                        }
                    }
                }
            }
            for (id, s) in &os {
                if !ns.contains_key(id) {
                    out.push(Change::SymbolRemoved {
                        page: page.clone(),
                        item: tag_or_symbol(s),
                        what: s.symbol.clone(),
                    });
                }
            }
            let ow: BTreeMap<&str, _> = o.wires.iter().map(|w| (w.id.as_str(), w)).collect();
            let nw: BTreeMap<&str, _> = n.wires.iter().map(|w| (w.id.as_str(), w)).collect();
            for (id, w) in &nw {
                match ow.get(id) {
                    None => out.push(Change::WireAdded {
                        page: page.clone(),
                        wire: (*id).into(),
                    }),
                    Some(prev) if prev != w => out.push(Change::WireChanged {
                        page: page.clone(),
                        wire: (*id).into(),
                    }),
                    Some(_) => {}
                }
            }
            for id in ow.keys() {
                if !nw.contains_key(id) {
                    out.push(Change::WireRemoved {
                        page: page.clone(),
                        wire: (*id).into(),
                    });
                }
            }
            if o.graphics != n.graphics {
                out.push(Change::GraphicsChanged { page: page.clone() });
            }
            let settings = [
                ("sheet size", o.size != n.size),
                ("title block", o.title_block != n.title_block),
                ("revision clouds", o.clouds != n.clouds),
                ("revision table", o.revision_table != n.revision_table),
                ("underlay", o.underlay != n.underlay),
                ("overview setting", o.overview != n.overview),
                ("cover setting", o.cover != n.cover),
            ];
            for (what, changed) in settings {
                if changed {
                    out.push(Change::PageSettingChanged {
                        page: page.clone(),
                        what: what.into(),
                    });
                }
            }
            // Anything else that differs (a part of the page added later) is still a change.
            if out.is_empty() && o != n {
                out.push(Change::PageSettingChanged {
                    page,
                    what: "page".into(),
                });
            }
        }
        (None, None) => {}
    }
    out
}

/// Changes from `from` to `to` (a revision, or `None` for the working folder).
pub fn changes(git: &Git, from: &str, to: Option<&str>) -> Result<Vec<Change>> {
    // `-z`: paths as they are (no quoting of accents, tabs or quotes), every field NUL-ended.
    // `--end-of-options`: a version that starts with `-` is refused as an unknown version, not
    // read as an option of git.
    let mut args = vec!["diff", "--name-status", "--no-renames", "-z", "--end-of-options", from];
    if let Some(t) = to {
        args.push(t);
    }
    // Paths read as bytes: a name that is not UTF-8 stays apart from every other name.
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut fields = git.run_paths(&args)?.into_iter();
    while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
        entries.push((status, path));
    }
    if to.is_none() {
        // New files the user has not recorded yet are changes too.
        for path in git.run_paths(&["ls-files", "--others", "--exclude-standard", "-z"])? {
            entries.push(("A".to_owned(), path));
        }
    }
    let mut out = Vec::new();
    for (status, path) in &entries {
        let (status, path) = (status.as_str(), path.as_str());
        let pages = if path.starts_with("pages/") && path.ends_with(".page.json") {
            let old = read_side(path, git.show(from, path));
            let new = match to {
                Some(t) => read_side(path, git.show(t, path)),
                None => read_working(&git.dir.join(path)).and_then(|text| read_side(path, text)),
            };
            // A side whose file is there but is no page (conflict markers, a hand edit, a
            // newer format, bytes that are not text) cannot be compared: the file itself is
            // listed instead.
            match (old, new) {
                (Ok(old), Ok(new)) => Some((old, new)),
                _ => None,
            }
        } else {
            None
        };
        if let Some((old, new)) = pages {
            out.extend(page_changes(old.as_ref(), new.as_ref()));
        } else {
            let status = match status {
                "A" => "added",
                "D" => "removed",
                _ => "changed",
            };
            out.push(Change::File {
                path: path.into(),
                status: status.into(),
            });
        }
    }
    Ok(out)
}

/// One recorded version of a page: who changed it, when, why, and what changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageVersion {
    pub commit: String,
    pub author: String,
    /// Unix seconds.
    pub time: i64,
    pub subject: String,
    pub changes: Vec<Change>,
}

/// The history of one page, newest first: every recorded version that changed its file, with
/// the changes against the version before it. A page that was removed and added again shows
/// both.
pub fn page_history(git: &Git, page: &str, limit: usize) -> Result<Vec<PageVersion>> {
    wc_core::store::check_id(page)?;
    let path = format!("pages/{page}.page.json");
    let n = format!("-n{}", limit.max(1));
    let text = git.run(&["log", &n, "--format=%H%x00%an%x00%at%x00%s%x00", "--", &path])?;
    let mut out = Vec::new();
    // NUL-ended fields, so a subject holding any other character keeps its record whole.
    for [commit, author, time, subject] in crate::log_records(&text) {
        let old_text = git.show(&format!("{commit}^"), &path);
        let new_text = git.show(commit, &path);
        let (had, has) = (old_text.is_some(), new_text.is_some());
        // As in [`changes`]: a side whose file is there but is no page cannot be compared, so
        // the file itself is listed instead of a page that seems removed or added.
        let changes = match (read_side(&path, old_text), read_side(&path, new_text)) {
            (Ok(old), Ok(new)) => page_changes(old.as_ref(), new.as_ref()),
            _ => vec![Change::File {
                path: path.clone(),
                status: match (had, has) {
                    (false, _) => "added",
                    (_, false) => "removed",
                    _ => "changed",
                }
                .into(),
            }],
        };
        out.push(PageVersion {
            commit: commit.to_owned(),
            author: author.to_owned(),
            time: time.trim().parse().unwrap_or(0),
            subject: subject.to_owned(),
            changes,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wc_core::geom::{Point, Shape};
    use wc_core::project::{Endpoint, Wire};
    use wc_core::{DeviceTag, PlacedSymbol, store};

    fn symbol(id: &str, lib: &str, tag: Option<&str>) -> wc_core::Result<PlacedSymbol> {
        Ok(PlacedSymbol {
            id: id.into(),
            symbol: lib.into(),
            at: Point::new(10.0, 10.0),
            rotation: Default::default(),
            mirror: false,
            tag: tag.map(DeviceTag::parse).transpose()?,
        })
    }

    fn wire(id: &str, x: f64) -> Wire {
        Wire {
            id: id.into(),
            from: Endpoint::Point {
                at: Point::new(0.0, 0.0),
            },
            to: Endpoint::Point { at: Point::new(x, 0.0) },
            via: Vec::new(),
            number: None,
            potential: None,
            cable: None,
            route: Default::default(),
            spec: Default::default(),
        }
    }

    fn shown(old: Option<&Page>, new: Option<&Page>) -> Vec<String> {
        page_changes(old, new).iter().map(ToString::to_string).collect()
    }

    #[test]
    fn a_page_added_removed_or_renamed_is_named() {
        let a = Page::new("p", "3", "Motor");
        assert!(page_changes(None, None).is_empty());
        assert_eq!(shown(None, Some(&a)), ["page 3 added (Motor)"]);
        assert_eq!(shown(Some(&a), None), ["page 3 removed (Motor)"]);
        let mut b = a.clone();
        b.title = "Pump".into();
        b.number = "4".into();
        // Renumbered first, then renamed, both under the new number.
        assert_eq!(
            page_changes(Some(&a), Some(&b)),
            [
                Change::PageRenumbered {
                    page: "4".into(),
                    from: "3".into(),
                },
                Change::PageRenamed {
                    page: "4".into(),
                    from: "Motor".into(),
                    to: "Pump".into(),
                },
            ]
        );
        assert_eq!(shown(Some(&a), Some(&b))[1], r#"page 4 renamed "Motor" -> "Pump""#);
    }

    #[test]
    fn wires_added_removed_and_changed_are_listed_by_id() {
        let mut a = Page::new("p", "3", "X");
        a.wires = vec![wire("w1", 10.0), wire("w2", 20.0), wire("w3", 30.0)];
        let mut b = a.clone();
        b.wires.retain(|w| w.id != "w2");
        b.wires[0].number = Some("101".into());
        b.wires.push(wire("w4", 40.0));
        // w3 is untouched and not listed; additions and changes come before removals.
        assert_eq!(
            shown(Some(&a), Some(&b)),
            [
                "page 3: wire w1 changed",
                "page 3: wire w4 added",
                "page 3: wire w2 removed",
            ]
        );
        // Only the wire order changed: matched by id, no wire is named, but the file differs.
        let mut c = a.clone();
        c.wires.reverse();
        assert_eq!(shown(Some(&a), Some(&c)), ["page 3: page changed"]);
    }

    #[test]
    fn a_symbol_without_a_tag_is_named_by_its_library_symbol() -> wc_core::Result<()> {
        let mut a = Page::new("p", "3", "X");
        a.symbols = vec![symbol("s1", "terminal", None)?, symbol("s2", "coil", Some("-K1"))?];
        let mut b = a.clone();
        b.symbols.retain(|s| s.id != "s1");
        assert_eq!(shown(Some(&a), Some(&b)), ["page 3: terminal terminal removed"]);
        assert_eq!(
            page_changes(Some(&b), Some(&a)),
            [Change::SymbolAdded {
                page: "3".into(),
                item: "terminal".into(),
                what: "terminal".into(),
            }]
        );
        // Tagged for the first time: the old tag is empty, the item is the symbol's id.
        let mut c = a.clone();
        c.symbols[0].tag = Some(DeviceTag::parse("-X1")?);
        assert_eq!(
            page_changes(Some(&a), Some(&c)),
            [Change::SymbolRetagged {
                page: "3".into(),
                item: "s1".into(),
                from: String::new(),
                to: "-X1".into(),
            }]
        );
        // Turned or mirrored in place is a move too.
        let mut d = a.clone();
        d.symbols[1].mirror = true;
        assert_eq!(shown(Some(&a), Some(&d)), ["page 3: coil -K1 moved"]);
        Ok(())
    }

    #[test]
    fn graphics_changes_are_reported_once() {
        let a = Page::new("p", "3", "X");
        let mut b = a.clone();
        b.graphics.push(Shape::Line {
            a: Point::new(0.0, 0.0),
            b: Point::new(10.0, 0.0),
        });
        b.graphics.push(Shape::Line {
            a: Point::new(0.0, 5.0),
            b: Point::new(10.0, 5.0),
        });
        assert_eq!(shown(Some(&a), Some(&b)), ["page 3: graphics changed"]);
    }

    #[test]
    fn other_files_are_listed_with_their_status() -> Result<()> {
        let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
        let dir = tmp.path();
        store::create(dir, "Demo")?;
        let write = |name: &str, text: &str| {
            let path = dir.join(name);
            std::fs::write(&path, text).map_err(|e| wc_core::Error::io(&path, e))
        };
        write("keep.txt", "one\n")?;
        write("drop.txt", "one\n")?;
        let git = Git::new(dir);
        git.init()?;
        let base = git.head().ok_or_else(|| wc_core::Error::Invalid("no head".into()))?;

        write("keep.txt", "two\n")?;
        std::fs::remove_file(dir.join("drop.txt")).map_err(|e| wc_core::Error::io("drop.txt", e))?;
        // Not recorded yet, and in the pages folder without being a page file.
        write("pages/notes.txt", "new\n")?;
        let file = |path: &str, status: &str| Change::File {
            path: path.into(),
            status: status.into(),
        };
        let mut working = changes(&git, &base, None)?;
        working.sort_by_key(ToString::to_string);
        assert_eq!(
            working,
            [
                file("drop.txt", "removed"),
                file("keep.txt", "changed"),
                file("pages/notes.txt", "added"),
            ]
        );

        git.commit("Edit")?;
        let mut recorded = changes(&git, &base, Some("HEAD"))?;
        recorded.sort_by_key(ToString::to_string);
        assert_eq!(recorded, working, "the same once recorded");
        // The other way round: what was removed is added back, what was added is removed.
        let mut back = changes(&git, "HEAD", Some(&base))?;
        back.sort_by_key(ToString::to_string);
        assert_eq!(
            back,
            [
                file("drop.txt", "added"),
                file("keep.txt", "changed"),
                file("pages/notes.txt", "removed"),
            ]
        );
        Ok(())
    }

    #[test]
    fn page_history_keeps_to_the_limit() -> Result<()> {
        let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
        let dir = tmp.path();
        let mut p = store::create(dir, "Demo")?;
        let git = Git::new(dir);
        git.init()?;
        for title in ["One", "Two"] {
            if let Some(page) = p.page_mut("p0001") {
                page.title = title.into();
            }
            store::save(dir, &p)?;
            git.commit(&format!("Title {title}"))?;
        }
        assert_eq!(page_history(&git, "p0001", 10)?.len(), 3);
        let newest = page_history(&git, "p0001", 1)?;
        let [only] = newest.as_slice() else {
            unreachable!("one version: {newest:?}")
        };
        assert_eq!(only.subject, "Title Two");
        assert_eq!(only.commit, git.head().unwrap_or_default());
        assert!(only.time > 0);
        assert!(
            matches!(only.changes.as_slice(), [Change::PageRenamed { from, to, .. }] if from == "One" && to == "Two"),
            "{:?}",
            only.changes
        );
        // A limit of 0 still shows the newest version.
        assert_eq!(page_history(&git, "p0001", 0)?, newest);
        // A page that was never recorded has no history.
        assert!(page_history(&git, "p9999", 10)?.is_empty());
        Ok(())
    }
}
