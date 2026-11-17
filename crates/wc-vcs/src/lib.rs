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

pub mod changes;
mod git;
pub mod subway;

pub use changes::{Change, PageVersion, changes, page_history};
pub use git::{Git, MergeOutcome};
pub use subway::{Commit, Edge, Row, history, subway_svg, subway_text};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot run git: {0}")]
    Spawn(std::io::Error),
    #[error("git {args}: {message}")]
    Git { args: String, message: String },
    #[error(transparent)]
    Core(#[from] wc_core::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
