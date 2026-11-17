//! Moving session messages between a host and its guests.
//!
//! [`pair`] makes an in-process connection (two instances in one program, and tests). A network
//! transport sends the same messages, one JSON object per line ([`encode`] / [`decode`]), and
//! plugs into [`Hub`] the same way.

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{Error, GuestMsg, Host, HostMsg, PeerId, Result};

/// One end of a connection: sends `S`, receives `R`.
#[derive(Debug)]
pub struct Link<S, R> {
    tx: Sender<S>,
    rx: Receiver<R>,
}

impl<S, R> Link<S, R> {
    pub fn send(&self, msg: S) -> Result<()> {
        self.tx.send(msg).map_err(|_| Error::Closed)
    }

    /// The next message if one has arrived, `Ok(None)` if not, `Err(Closed)` once the other end
    /// is gone and everything it sent was read.
    pub fn try_recv(&self) -> Result<Option<R>> {
        match self.rx.try_recv() {
            Ok(m) => Ok(Some(m)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(Error::Closed),
        }
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
        Link {
            tx: to_host,
            rx: from_host,
        },
        Link {
            tx: to_guest,
            rx: from_guest,
        },
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
                self.links.remove(&to);
                if failed {
                    host.disconnect(to);
                }
            }
        }
    }
}
