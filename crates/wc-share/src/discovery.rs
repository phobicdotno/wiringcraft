//! Finding shared sessions on the local network.
//!
//! A host that shares a session runs an [`Announcer`]: a thread that sends a small UDP beacon
//! (the project's name, the host's name and the session's TCP port, as one JSON object) every
//! [`ANNOUNCE_EVERY`] to the broadcast address on [`DISCOVERY_PORT`]. Nothing is sent unless
//! an announcer was started, and it stops when [`Announcer::stop`] is called or it is dropped
//! (so closing the app stops it too).
//!
//! A guest runs a [`Browser`]: it listens on the discovery port and lists the sessions it has
//! heard from, dropping one that has been silent for longer than its timeout. Networks that do
//! not pass broadcasts (most VPNs) find nothing; the guest then types the host's address and
//! joins with [`crate::net::connect`] as usual.

use std::collections::BTreeMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::Result;

/// The UDP port beacons are sent to and browsers listen on.
pub const DISCOVERY_PORT: u16 = 47_811;

/// How often an announcer sends its beacon.
pub const ANNOUNCE_EVERY: Duration = Duration::from_secs(2);

/// How long a browser keeps a session it has stopped hearing from.
pub const FORGET_AFTER: Duration = Duration::from_secs(7);

/// Marks a datagram as a WiringCraft beacon, so other traffic on the port is ignored.
const MAGIC: &str = "wiringcraft-session";

/// The largest beacon, in bytes. An announcer refuses a longer one; a browser reads no more.
/// 2048 bytes of names and port as before the protocol version was announced, plus room for
/// that field, so every project and host name that could be announced then still can.
pub const MAX_BEACON: usize = 2048 + 64;

/// The most datagrams one [`Browser::poll`] reads, so a flood cannot hold up the window.
/// The rest wait for the next poll.
pub(crate) const MAX_PER_POLL: usize = 256;

/// The most sessions a [`Browser`] lists at once. A beacon from a session not already listed
/// is ignored while the list is full, until older sessions are forgotten, so a device sending
/// beacons for many ports or from many addresses cannot grow the list without bound. Sessions
/// already listed keep being refreshed.
pub(crate) const MAX_SESSIONS: usize = 256;

/// What a host announces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Beacon {
    /// The project's name.
    pub project: String,
    /// The host's name, as people know the computer or the person sharing.
    pub host: String,
    /// The TCP port the session listens on.
    pub port: u16,
    /// The host's session protocol version ([`crate::PROTOCOL_VERSION`]); 0 from a build that
    /// did not announce one.
    #[serde(default)]
    pub protocol: u32,
}

#[derive(Serialize, Deserialize)]
struct Wire {
    magic: String,
    #[serde(flatten)]
    beacon: Beacon,
}

impl Beacon {
    pub(crate) fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(&Wire {
            magic: MAGIC.into(),
            beacon: self.clone(),
        })
        .unwrap_or_default()
    }

    fn decode(bytes: &[u8]) -> Option<Beacon> {
        let w: Wire = serde_json::from_slice(bytes).ok()?;
        (w.magic == MAGIC && w.beacon.port != 0).then_some(w.beacon)
    }
}

/// A session a browser has heard of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub project: String,
    pub host: String,
    /// Where to connect to join: the sender's IP address with the announced port.
    pub addr: SocketAddr,
    /// The host's session protocol version, as announced: a session on another version than
    /// [`crate::PROTOCOL_VERSION`] cannot be joined.
    pub protocol: u32,
}

/// A running announcement. Stops on [`Announcer::stop`] or when dropped.
#[derive(Debug)]
pub struct Announcer {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Why the last send failed; cleared by the next one that works.
    last_error: Arc<Mutex<Option<String>>>,
}

impl Announcer {
    /// Announce `beacon` to the local network's broadcast address on [`DISCOVERY_PORT`].
    pub fn start(beacon: Beacon) -> Result<Self> {
        let to = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::BROADCAST, DISCOVERY_PORT));
        Self::start_to(beacon, to, ANNOUNCE_EVERY)
    }

    /// Announce `beacon` to `to` every `every` (tests send to a browser on localhost).
    pub fn start_to(beacon: Beacon, to: SocketAddr, every: Duration) -> Result<Self> {
        let bytes = beacon.encode();
        if bytes.len() > MAX_BEACON {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "the session announcement is {} bytes, more than the {MAX_BEACON} a browser reads; use a shorter project or host name",
                    bytes.len()
                ),
            )
            .into());
        }
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        socket.set_broadcast(true)?;
        let last_error = Arc::new(Mutex::new(None));
        let error = Arc::clone(&last_error);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = thread::Builder::new().name("session-announce".into()).spawn(move || {
            let tick = Duration::from_millis(50).min(every);
            let mut next = Instant::now();
            while !flag.load(Ordering::Relaxed) {
                if Instant::now() >= next {
                    // A failed send (no network yet, cable out) is tried again next time;
                    // its reason is kept for `last_error`.
                    let result = socket.send_to(&bytes, to).err().map(|e| e.to_string());
                    if let Ok(mut slot) = error.lock() {
                        *slot = result;
                    }
                    next = Instant::now() + every;
                }
                thread::sleep(tick);
            }
        })?;
        Ok(Self {
            stop,
            thread: Some(thread),
            last_error,
        })
    }

    /// Why the last announcement could not be sent, while it keeps failing (for example no
    /// network). `None` once a send works again.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|e| e.clone())
    }

    /// Stop announcing. Browsers forget the session after [`FORGET_AFTER`].
    pub fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Announcer {
    fn drop(&mut self) {
        self.halt();
    }
}

/// Listens for beacons and lists the sessions heard recently. Poll it from the window; it
/// never blocks.
#[derive(Debug)]
pub struct Browser {
    socket: UdpSocket,
    forget_after: Duration,
    heard: BTreeMap<SocketAddr, (Session, Instant)>,
}

impl Browser {
    /// Listen on [`DISCOVERY_PORT`] on every interface.
    pub fn open() -> Result<Self> {
        Self::bind(
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)),
            FORGET_AFTER,
        )
    }

    /// Listen on `addr` (tests use `127.0.0.1:0`), forgetting sessions silent for `forget_after`.
    pub fn bind(addr: SocketAddr, forget_after: Duration) -> Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            forget_after,
            heard: BTreeMap::new(),
        })
    }

    /// The address it listens on.
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.socket.local_addr()?)
    }

    /// Take datagrams from `recv` until it would block, at most [`MAX_PER_POLL`] of them, so
    /// a sender that keeps the queue full cannot hold up a poll. Returns how many were read.
    pub(crate) fn receive(
        &mut self,
        mut recv: impl FnMut(&mut [u8]) -> io::Result<(usize, SocketAddr)>,
    ) -> Result<usize> {
        let mut buf = [0u8; MAX_BEACON];
        let mut read = 0;
        for _ in 0..MAX_PER_POLL {
            match recv(&mut buf) {
                Ok((n, from)) => {
                    read += 1;
                    if let Some(b) = Beacon::decode(&buf[..n.min(buf.len())]) {
                        let addr = SocketAddr::new(from.ip(), b.port);
                        let s = Session {
                            project: b.project,
                            host: b.host,
                            addr,
                            protocol: b.protocol,
                        };
                        let now = Instant::now();
                        if !self.heard.contains_key(&addr) && self.heard.len() >= MAX_SESSIONS {
                            self.forget_silent(now);
                            if self.heard.len() >= MAX_SESSIONS {
                                continue;
                            }
                        }
                        self.heard.insert(addr, (s, now));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                // Windows reports an earlier send's ICMP error on the next receive; it says
                // nothing about this socket.
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(read)
    }

    /// Read the beacons that have arrived (at most a few hundred per call) and return the
    /// sessions heard within the timeout, sorted by project name and host.
    pub fn poll(&mut self) -> Result<Vec<Session>> {
        let socket = self.socket.try_clone()?;
        self.receive(|buf| socket.recv_from(buf))?;
        self.forget_silent(Instant::now());
        let mut list: Vec<Session> = self.heard.values().map(|(s, _)| s.clone()).collect();
        list.sort_by(|a, b| (&a.project, &a.host, a.addr).cmp(&(&b.project, &b.host, b.addr)));
        Ok(list)
    }

    /// Drop the sessions not heard from within the timeout.
    fn forget_silent(&mut self, now: Instant) {
        let keep = self.forget_after;
        self.heard.retain(|_, (_, at)| now.duration_since(*at) <= keep);
    }
}
