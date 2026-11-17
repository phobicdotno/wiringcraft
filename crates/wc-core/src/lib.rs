//! WiringCraft core: the data model of an electrical project and its text-based file format.
//!
//! A project is a folder of small, deterministic text files (see [`store`]) so it can be
//! diffed, merged and recovered with ordinary tools.

pub mod autowire;
pub mod connectivity;
pub mod error;
pub mod find;
pub mod geom;
pub mod hash;
pub mod lock;
pub mod macros;
pub mod numbering;
pub mod panel;
pub mod part;
pub mod project;
pub mod review;
pub mod route;
pub mod store;
pub mod symbol;
pub mod tag;

pub use error::{Error, Result};
pub use geom::{Point, Shape};
pub use lock::{LockEntry, Lockfile};
pub use part::Part;
pub use project::{
    CableCore, Device, DevicePart, Endpoint, Location, Page, PageSize, PlacedSymbol, Project, ProjectMeta, Underlay,
    Wire, WireSpec,
};
pub use review::{Comment, Reply};
pub use symbol::{Pin, Symbol};
pub use tag::DeviceTag;
