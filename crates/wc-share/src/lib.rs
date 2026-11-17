//! Shared work sessions: several people edit one open project live over the office network.
//!
//! One instance is the **host**: it holds the authoritative project and decides everything.
//! Others are **guests**: they keep their own copy of the project on their own disk and only
//! exchange changes with the host (opening a project straight from a file share is about 20
//! times slower, so the project itself never travels over the network after joining).
//!
//! The rules, all enforced by [`Host`]:
//!
//! - **Approval.** A guest sends [`GuestMsg::Join`]; the host approves or declines each request.
//!   Everything a guest sends before approval is refused.
//! - **Page locks.** The page a person has open is locked to them. Others see it read-only.
//!   Opening another page, or leaving, releases the lock.
//! - **Project-level data** (devices, panels, comments, adding, removing and renumbering pages)
//!   goes through the host one change at a time as a [`ProjectEdit`]. A page's display number is
//!   project-level too: a page edit never changes it.
//! - **One commit per change.** Every applied change is handed to a [`Journal`]; [`GitJournal`]
//!   saves the project and commits it with the participant as the author, so any change can be
//!   reverted from the history.
//! - **Undo per change.** The host keeps what each change touched ([`Parts`]). Undo takes back
//!   the person's own last change only, and any change can be taken back from the list, as long
//!   as nobody changed the same parts since; each is a commit of its own.
//!
//! The host and guests are plain state machines with no I/O; [`transport`] connects them
//! in-process and [`net`] over TCP; [`discovery`] finds sessions on the local network.
//! Tests run whole sessions both ways.

pub mod discovery;
mod guest;
mod history;
mod host;
mod journal;
pub mod net;
mod protocol;
pub mod transport;

pub use guest::{Guest, GuestStatus};
pub use history::{ChangeInfo, ChangeKind, MAX_HISTORY, Parts, note, redo_target, undo_target};
pub use host::{HOST, Host};
pub use journal::{Change, GitJournal, Journal, MemoryJournal};
pub use protocol::{GuestMsg, HostMsg, Peer, PeerId, ProjectEdit, Refusal, Snapshot, apply_edit, same_but_number};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Core(#[from] wc_core::Error),
    #[error(transparent)]
    Vcs(#[from] wc_vcs::Error),
    #[error("bad session message: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("session network: {0}")]
    Io(#[from] std::io::Error),
    #[error("the session connection is closed")]
    Closed,
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
