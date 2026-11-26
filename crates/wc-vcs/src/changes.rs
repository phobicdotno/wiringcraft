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
            Change::WireAdded { page, wire } => write!(f, "page {page}: wire {wire} added"),
            Change::WireRemoved { page, wire } => write!(f, "page {page}: wire {wire} removed"),
            Change::WireChanged { page, wire } => write!(f, "page {page}: wire {wire} changed"),
            Change::GraphicsChanged { page } => write!(f, "page {page}: graphics changed"),
            Change::File { path, status } => write!(f, "{path} {status}"),
        }
    }
}

fn parse_page(text: Option<String>) -> Option<Page> {
    text.and_then(|t| serde_json::from_str(&t).ok())
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
    // `--end-of-options`: a version that starts with `-` is refused as an unknown version, not
    // read as an option of git.
    let mut args = vec!["diff", "--name-status", "--no-renames", "--end-of-options", from];
    if let Some(t) = to {
        args.push(t);
    }
    let mut text = git.run(&args)?;
    if to.is_none() {
        // New files the user has not recorded yet are changes too.
        for path in git.run(&["ls-files", "--others", "--exclude-standard"])?.lines() {
            text.push_str(&format!("A\t{path}\n"));
        }
    }
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = line.splitn(2, '\t');
        let (Some(status), Some(path)) = (parts.next(), parts.next()) else {
            continue;
        };
        if path.starts_with("pages/") && path.ends_with(".page.json") {
            let old = parse_page(git.show(from, path));
            let new = match to {
                Some(t) => parse_page(git.show(t, path)),
                None => parse_page(std::fs::read_to_string(git.dir.join(path)).ok()),
            };
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
        let old = parse_page(git.show(&format!("{commit}^"), &path));
        let new = parse_page(git.show(commit, &path));
        out.push(PageVersion {
            commit: (*commit).to_owned(),
            author: (*author).to_owned(),
            time: time.parse().unwrap_or(0),
            subject: (*subject).to_owned(),
            changes: page_changes(old.as_ref(), new.as_ref()),
        });
    }
    Ok(out)
}
