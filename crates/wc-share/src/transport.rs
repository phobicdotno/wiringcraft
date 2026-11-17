//! Moving session messages between a host and its guests.
//!
//! [`pair`] makes an in-process connection (two instances in one program, and tests). The network
//! transport ([`crate::net`]) sends the same messages over TCP, one JSON object per line
//! ([`encode`] / [`decode`]), and plugs into [`Hub`] the same way.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{Error, GuestMsg, Host, HostMsg, PeerId, Result};

/// Why a connection ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    /// The other side ended it in order: it closed the socket or dropped its end.
    Ended,
    /// The other side sent a line longer than the limit (bytes).
    LineTooLong { max: usize },
    /// The other side sent a line that is not a session message.
    BadMessage(String),
    /// Reading from the connection failed.
    ReadFailed(String),
    /// A write did not finish in time: the other side stopped reading.
    WriteTimedOut,
    /// Writing to the connection failed.
    WriteFailed(String),
}

impl CloseReason {
    /// Whether the connection ended in order rather than failing.
    pub fn is_orderly(&self) -> bool {
        matches!(self, Self::Ended)
    }
}

impl fmt::Display for CloseReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ended => write!(f, "the other side closed the connection"),
            Self::LineTooLong { max } => write!(f, "the other side sent a line longer than {max} bytes"),
            Self::BadMessage(why) => write!(f, "the other side sent something that is not a session message ({why})"),
            Self::ReadFailed(why) => write!(f, "reading from the network failed ({why})"),
            Self::WriteTimedOut => write!(f, "the other side stopped taking messages (write timed out)"),
            Self::WriteFailed(why) => write!(f, "writing to the network failed ({why})"),
        }
    }
}

/// Where a link's worker threads leave why the connection ended. The first reason stays: a
/// failed write closes the socket, and the reader's end of stream that follows is not the cause.
#[derive(Debug, Clone, Default)]
pub(crate) struct CloseSlot(Arc<Mutex<Option<CloseReason>>>);

impl CloseSlot {
    /// Keep `reason` unless one is already kept.
    pub(crate) fn set(&self, reason: CloseReason) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(reason);
    }

    fn get(&self) -> Option<CloseReason> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// One end of a connection: sends `S`, receives `R`.
#[derive(Debug)]
pub struct Link<S, R> {
    tx: Sender<S>,
    rx: Receiver<R>,
    reason: CloseSlot,
    /// Set once a receive or send found the connection closed.
    closed: AtomicBool,
}

impl<S, R> Link<S, R> {
    fn new(tx: Sender<S>, rx: Receiver<R>, reason: CloseSlot) -> Self {
        Self {
            tx,
            rx,
            reason,
            closed: AtomicBool::new(false),
        }
    }

    /// A link over existing channels; the network transport feeds them from a socket and
    /// leaves the reason it closed in `reason`.
    pub(crate) fn from_channels(tx: Sender<S>, rx: Receiver<R>, reason: CloseSlot) -> Self {
        Self::new(tx, rx, reason)
    }

    pub fn send(&self, msg: S) -> Result<()> {
        self.tx.send(msg).map_err(|_| {
            self.closed.store(true, Ordering::Relaxed);
            Error::Closed
        })
    }

    /// The next message if one has arrived, `Ok(None)` if not, `Err(Closed)` once the other end
    /// is gone and everything it sent was read. [`Self::close_reason`] then says why.
    pub fn try_recv(&self) -> Result<Option<R>> {
        match self.rx.try_recv() {
            Ok(m) => Ok(Some(m)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.closed.store(true, Ordering::Relaxed);
                Err(Error::Closed)
            }
        }
    }

    /// Why the connection ended: `None` while it is open (or before a receive or send found it
    /// closed), else whether it ended in order or why it failed.
    pub fn close_reason(&self) -> Option<CloseReason> {
        self.reason
            .get()
            .or_else(|| self.closed.load(Ordering::Relaxed).then_some(CloseReason::Ended))
    }

    /// Every message that has arrived so far.
    pub fn drain(&self) -> Vec<R> {
        self.rx.try_iter().collect()
    }
}

/// The guest's end of a connection.
pub type GuestLink = Link<GuestMsg, HostMsg>;
/// The host's end of a connection.
pub type HostLink = Link<HostMsg, GuestMsg>;

/// An in-process connection: `(guest end, host end)`.
pub fn pair() -> (GuestLink, HostLink) {
    let (to_host, from_guest) = channel();
    let (to_guest, from_host) = channel();
    (
        Link::new(to_host, from_host, CloseSlot::default()),
        Link::new(to_guest, from_guest, CloseSlot::default()),
    )
}

/// One message as a line of JSON, ending in `\n`.
pub fn encode<T: Serialize>(msg: &T) -> Result<String> {
    let mut line = serde_json::to_string(msg)?;
    line.push('\n');
    Ok(line)
}

/// One line of JSON back into a message.
pub fn decode<T: DeserializeOwned>(line: &str) -> Result<T> {
    Ok(serde_json::from_str(line.trim_end())?)
}

/// The host's connections. Call [`Hub::pump`] once per frame: it hands every arrived message
/// to the host, notices closed connections, and sends what the host has to say.
#[derive(Debug, Default)]
pub struct Hub {
    links: BTreeMap<PeerId, HostLink>,
    /// Connections that ended without the guest leaving, with why, for [`Hub::take_closed`].
    closed: Vec<(PeerId, CloseReason)>,
}

impl Hub {
    pub fn new() -> Self {
        Self::default()
    }

    /// A new connection; returns the participant id the host gave it.
    pub fn add(&mut self, host: &mut Host, link: HostLink) -> PeerId {
        let id = host.connect();
        self.links.insert(id, link);
        id
    }

    pub fn len(&self) -> usize {
        self.links.len()
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    pub fn pump(&mut self, host: &mut Host) {
        let mut gone = Vec::new();
        for (&id, link) in &self.links {
            loop {
                match link.try_recv() {
                    Ok(Some(msg)) => {
                        let leaving = matches!(msg, GuestMsg::Leave);
                        // A refusal is already queued for the guest by the host.
                        let _ = host.receive(id, msg);
                        if leaving {
                            gone.push(id);
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => {
                        self.closed
                            .push((id, link.close_reason().unwrap_or(CloseReason::Ended)));
                        gone.push(id);
                        break;
                    }
                }
            }
        }
        for id in gone {
            host.disconnect(id);
            self.links.remove(&id);
        }
        self.flush(host);
    }

    /// Send what the host has queued. Connections that were declined or closed are dropped.
    pub fn flush(&mut self, host: &mut Host) {
        for (to, msg) in host.take_outbox() {
            let last = matches!(msg, HostMsg::Declined | HostMsg::Closed);
            let failed = self.links.get(&to).is_some_and(|link| link.send(msg).is_err());
            if last || failed {
                if let Some(link) = self.links.remove(&to)
                    && failed
                {
                    self.closed
                        .push((to, link.close_reason().unwrap_or(CloseReason::Ended)));
                }
                if failed {
                    host.disconnect(to);
                }
            }
        }
    }

    /// Connections that closed since the last call without the guest saying it leaves, with
    /// why, oldest first: for the log or the session window.
    pub fn take_closed(&mut self) -> Vec<(PeerId, CloseReason)> {
        std::mem::take(&mut self.closed)
    }
}
