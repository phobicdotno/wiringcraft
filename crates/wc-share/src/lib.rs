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
//!
//! Picture files (the logo, page underlays) are not project data but travel too: the host sends
//! each one after the welcome, and a participant that adds one sends it as a
//! [`ProjectEdit::AddPicture`] before the change that names it.

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
pub use journal::{Change, GitJournal, Journal, MemoryJournal, write_picture};
pub use protocol::{
    GuestMsg, HostMsg, MAX_ADDED_PICTURE_BYTES, MAX_ADDED_PICTURES, MAX_PICTURE, PROTOCOL_VERSION, Peer, PeerId,
    Picture, ProjectEdit, Refusal, Snapshot, VersionMismatch, apply_edit, same_but_number,
};

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
    /// [`write_picture`] found another file under the picture's name, which it never writes
    /// over (other storage failures stay [`Error::Io`]).
    #[error("{0}")]
    PictureNameTaken(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The items `lock` pins that `files` (the hash of each file the session has, by kind and id)
/// has no file of in that version: kind and id. What the host and each guest count as missing.
fn files_missing(
    lock: &wc_core::Lockfile,
    files: &std::collections::BTreeMap<(wc_core::lock::ItemKind, String), String>,
) -> Vec<(wc_core::lock::ItemKind, String)> {
    use wc_core::lock::ItemKind;
    [ItemKind::Symbol, ItemKind::Part]
        .into_iter()
        .flat_map(|kind| {
            lock.map(kind)
                .iter()
                .filter(move |(id, e)| files.get(&(kind, (*id).clone())) != Some(&e.hash))
                .map(move |(id, _)| (kind, id.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests;
