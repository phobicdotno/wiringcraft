//! WiringCraft core: the data model of an electrical project and its text-based file format.
//!
//! A project is a folder of small, deterministic text files (see [`store`]) so it can be
//! diffed, merged and recovered with ordinary tools.

pub mod autowire;
pub mod base;
pub mod connectivity;
pub mod error;
pub mod find;
pub mod font;
pub mod geom;
pub mod hash;
pub mod lock;
pub mod macros;
pub mod numbering;
pub mod panel;
pub mod part;
pub mod project;
pub mod report_layout;
pub mod review;
pub mod revision;
pub mod route;
pub mod store;
pub mod symbol;
pub mod tag;
pub mod template;

pub use error::{Error, Result};
pub use geom::{Point, Shape};
pub use lock::{LockEntry, Lockfile};
pub use part::{ContactKind, Contacts, Part};
pub use project::{
    CableCore, Device, DevicePart, Endpoint, Location, Logo, Page, PageSize, PlacedSymbol, Project, ProjectMeta,
    Underlay, Wire, WireSpec,
};
pub use review::{Comment, Reply};
pub use revision::{Revision, RevisionCloud};
pub use symbol::{BoxPin, BoxSide, Pin, Symbol, black_box};
pub use tag::DeviceTag;
