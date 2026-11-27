//! Version control for WiringCraft projects, built on git.
//!
//! A project folder is a git repository. Because every page is its own deterministic text
//! file, git's history, branches and merges work at page granularity, and two people editing
//! different pages never conflict. On top of plain git this crate adds:
//!
//! - [`history`]: the commit graph laid out as a subway map (one coloured line per branch);
//! - [`subway_svg`] / [`subway_text`]: that map drawn as SVG or as terminal text;
//! - [`changes()`]: what changed between two versions in schematic terms (pages, symbols, wires),
//!   not in lines of JSON;
//! - [`page_history`]: who changed one page, when, and what they changed.

pub mod base;
pub mod changes;
mod git;
pub mod subway;

pub use changes::{Change, PageVersion, changes, page_history};
pub use git::{Git, LOCK_FILE, MergeOutcome, PARENT_PROJECT_ENV};
pub use subway::{Commit, Edge, Row, history, subway_svg, subway_text};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot run git: {0}")]
    Spawn(std::io::Error),
    #[error("git {args}: {message}")]
    Git { args: String, message: String },
    #[error(transparent)]
    Core(#[from] wc_core::Error),
    /// Stopped on request (see [`base::create_from_base_until`]).
    #[error("cancelled")]
    Cancelled,
    /// A file next to git's own that could not be read or written.
    #[error("{path}: {source}")]
    Io { path: String, source: std::io::Error },
}

pub type Result<T> = std::result::Result<T, Error>;

/// The records of `git log --format=...` output in which every one of the `N` fields ends in
/// NUL (`%x00`). Git never writes NUL inside a field (a commit message cannot hold one), so
/// text such as a subject cannot break the framing, whatever other characters it holds. The
/// newline git puts after each record is left off the first field of the next.
pub(crate) fn log_records<const N: usize>(text: &str) -> Vec<[&str; N]> {
    let mut parts = text.split('\0');
    let mut out = Vec::new();
    loop {
        let mut record = [""; N];
        for slot in &mut record {
            let Some(field) = parts.next() else {
                return out;
            };
            *slot = field;
        }
        if let Some(first) = record.first_mut() {
            *first = first.trim_start_matches('\n');
        }
        out.push(record);
    }
}

#[cfg(test)]
mod tests;
