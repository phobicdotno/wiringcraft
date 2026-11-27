//! WiringCraft core: the data model of an electrical project and its text-based file format.
//!
//! A project is a folder of small, deterministic text files (see [`store`]) so it can be
//! diffed, merged and recovered with ordinary tools.

pub mod autowire;
pub mod base;
pub mod box_edit;
pub mod check_rule;
pub mod connectivity;
pub mod error;
pub mod find;
pub mod font;
pub mod generate;
pub mod geom;
pub mod guard;
pub mod hash;
pub mod lock;
pub mod macros;
pub mod network;
pub mod numbering;
pub mod panel;
pub mod part;
pub mod placement;
pub mod plc_rack;
pub mod project;
pub mod report_layout;
pub mod review;
pub mod revision;
pub mod route;
pub mod store;
pub mod symbol;
pub mod tag;
pub mod template;
pub mod translate;

pub use error::{Error, Result};
pub use geom::{Point, Shape};
pub use lock::{LockEntry, Lockfile};
pub use part::{ContactKind, Contacts, Footprint, ModelSize, Part};
pub use project::{
    CableCore, Device, DevicePart, Endpoint, Location, Logo, Page, PageSize, PlacedSymbol, Project, ProjectMeta,
    Underlay, Wire, WireSpec,
};
pub use review::{Comment, Reply};
pub use revision::{Revision, RevisionCloud};
pub use symbol::{BoxPin, BoxSide, Pin, Symbol, black_box, black_box_layout, pin_position};
pub use tag::DeviceTag;
