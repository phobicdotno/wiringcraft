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
    let text = git.run(&args)?;
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut fields = text.split('\0');
    while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
        if status.is_empty() || path.is_empty() {
            continue;
        }
        entries.push((status.to_owned(), path.to_owned()));
    }
    if to.is_none() {
        // New files the user has not recorded yet are changes too.
        let others = git.run(&["ls-files", "--others", "--exclude-standard", "-z"])?;
        for path in others.split('\0').filter(|p| !p.is_empty()) {
            entries.push(("A".to_owned(), path.to_owned()));
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
    let text = git.run(&["log", &n, "--format=%H%x1f%an%x1f%at%x1f%s%x1e", "--", &path])?;
    let mut out = Vec::new();
    for record in text.split('\u{1e}') {
        let fields: Vec<&str> = record.trim().split('\u{1f}').collect();
        let [commit, author, time, subject] = fields.as_slice() else {
            continue;
        };
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
            commit: (*commit).to_owned(),
            author: (*author).to_owned(),
            time: time.parse().unwrap_or(0),
            subject: (*subject).to_owned(),
            changes,
        });
    }
    Ok(out)
}
