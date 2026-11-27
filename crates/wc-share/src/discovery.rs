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

    /// Make every listed session look heard from `by` earlier (tests: expire sessions without
    /// waiting on the clock).
    #[cfg(test)]
    pub(crate) fn age(&mut self, by: Duration) {
        for (_, at) in self.heard.values_mut() {
            *at = at.checked_sub(by).unwrap_or(*at);
        }
    }

    /// Drop the sessions not heard from within the timeout.
    fn forget_silent(&mut self, now: Instant) {
        let keep = self.forget_after;
        self.heard.retain(|_, (_, at)| now.duration_since(*at) <= keep);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

    fn beacon(project: &str, host: &str, port: u16) -> Beacon {
        Beacon {
            project: project.into(),
            host: host.into(),
            port,
            protocol: crate::PROTOCOL_VERSION,
        }
    }

    fn browser() -> Result<Browser> {
        Browser::bind(
            "127.0.0.1:0".parse().map_err(io::Error::other)?,
            Duration::from_secs(60),
        )
    }

    /// Hand `datagrams` to the browser, each from its sender, then report "would block".
    fn feed(b: &mut Browser, datagrams: Vec<(Vec<u8>, SocketAddr)>) -> Result<usize> {
        let mut it = datagrams.into_iter();
        b.receive(|buf| {
            let Some((bytes, from)) = it.next() else {
                return Err(io::ErrorKind::WouldBlock.into());
            };
            let n = bytes.len().min(buf.len());
            buf[..n].copy_from_slice(&bytes[..n]);
            Ok((bytes.len(), from))
        })
    }

    #[test]
    fn a_beacon_survives_encode_and_decode() {
        let b = Beacon {
            protocol: 7,
            ..beacon("Pump \"station\" \u{e6}\u{f8}\u{e5}\n", "Ola\t", 65535)
        };
        assert_eq!(Beacon::decode(&b.encode()), Some(b));
    }

    #[test]
    fn a_beacon_without_magic_with_port_0_or_cut_short_is_not_read() {
        let good = beacon("P", "Ola", 7001).encode();
        assert!(Beacon::decode(&good).is_some());
        assert_eq!(
            Beacon::decode(&beacon("P", "Ola", 0).encode()),
            None,
            "port 0 cannot be joined"
        );
        assert_eq!(
            Beacon::decode(br#"{"project":"P","host":"Ola","port":7001}"#),
            None,
            "no magic"
        );
        assert_eq!(
            Beacon::decode(br#"{"magic":"WiringCraft-Session","project":"P","host":"Ola","port":7001}"#),
            None,
            "magic is compared exactly"
        );
        assert_eq!(Beacon::decode(&good[..good.len() - 1]), None, "truncated JSON");
        assert_eq!(Beacon::decode(b""), None);
        assert_eq!(
            Beacon::decode(br#"{"magic":"wiringcraft-session","project":"P","host":"Ola","port":70000}"#),
            None,
            "a port out of range"
        );
        assert_eq!(
            Beacon::decode(br#"{"magic":"wiringcraft-session","project":"P","port":7001}"#),
            None,
            "a missing host"
        );
    }

    #[test]
    fn a_beacon_from_a_newer_build_with_more_fields_is_still_read() {
        let wire =
            br#"{"magic":"wiringcraft-session","project":"P","host":"Ola","port":7001,"protocol":3,"colour":"blue"}"#;
        assert_eq!(
            Beacon::decode(wire),
            Some(Beacon {
                protocol: 3,
                ..beacon("P", "Ola", 7001)
            })
        );
    }

    #[test]
    fn a_datagram_longer_than_a_beacon_is_cut_and_ignored() -> TestResult {
        let mut b = browser()?;
        let from: SocketAddr = "192.168.1.50:5000".parse()?;
        // Valid JSON as a whole, but only its first MAX_BEACON bytes are read.
        let long = beacon(&"P".repeat(MAX_BEACON), "Ola", 7001).encode();
        assert!(long.len() > MAX_BEACON);
        assert_eq!(feed(&mut b, vec![(long, from)])?, 1);
        assert!(b.poll()?.is_empty());
        Ok(())
    }

    #[test]
    fn one_sender_on_two_ports_is_two_sessions_and_a_repeat_replaces_one() -> TestResult {
        let mut b = browser()?;
        let from: SocketAddr = "192.168.1.50:5000".parse()?;
        let other: SocketAddr = "192.168.1.51:6000".parse()?;
        feed(
            &mut b,
            vec![
                (beacon("Mill", "Kari", 7001).encode(), from),
                (beacon("Mill", "Kari", 7002).encode(), from),
                (beacon("Mill", "Kari", 7001).encode(), other),
                // The same address again, renamed: it replaces the first, not a new entry.
                (beacon("Mill 2", "Kari", 7001).encode(), from),
            ],
        )?;
        let list = b.poll()?;
        let seen: Vec<_> = list.iter().map(|s| (s.project.as_str(), s.addr.to_string())).collect();
        assert_eq!(
            seen,
            [
                ("Mill", "192.168.1.50:7002".to_string()),
                ("Mill", "192.168.1.51:7001".to_string()),
                ("Mill 2", "192.168.1.50:7001".to_string()),
            ]
        );
        Ok(())
    }

    #[test]
    fn sessions_are_sorted_by_project_then_host() -> TestResult {
        let mut b = browser()?;
        let from = |last: u8| SocketAddr::from(([10, 0, 0, last], 5000));
        feed(
            &mut b,
            vec![
                (beacon("B", "Zed", 7001).encode(), from(1)),
                (beacon("A", "Ola", 7001).encode(), from(2)),
                (beacon("B", "Ada", 7001).encode(), from(3)),
            ],
        )?;
        let order: Vec<_> = b.poll()?.into_iter().map(|s| (s.project, s.host)).collect();
        assert_eq!(
            order,
            [
                ("A".into(), "Ola".into()),
                ("B".into(), "Ada".into()),
                ("B".into(), "Zed".into())
            ] as [(String, String); 3]
        );
        Ok(())
    }

    #[test]
    fn a_session_is_kept_until_silent_for_longer_than_the_timeout() -> TestResult {
        let forget = Duration::from_secs(60);
        let mut b = Browser::bind("127.0.0.1:0".parse()?, forget)?;
        feed(
            &mut b,
            vec![(beacon("P", "Ola", 7001).encode(), "10.0.0.1:5000".parse()?)],
        )?;
        b.age(forget / 2);
        assert_eq!(b.poll()?.len(), 1, "half the timeout: still listed");
        b.age(forget);
        assert!(b.poll()?.is_empty(), "past the timeout: forgotten");
        Ok(())
    }

    #[test]
    fn interrupted_and_reset_receives_are_skipped_and_other_errors_returned() -> TestResult {
        let mut b = browser()?;
        let from: SocketAddr = "10.0.0.1:5000".parse()?;
        let wire = beacon("P", "Ola", 7001).encode();
        let mut step = 0;
        let read = b.receive(|buf| {
            step += 1;
            match step {
                1 => Err(io::ErrorKind::Interrupted.into()),
                2 => Err(io::ErrorKind::ConnectionReset.into()),
                3 => {
                    buf[..wire.len()].copy_from_slice(&wire);
                    Ok((wire.len(), from))
                }
                _ => Err(io::ErrorKind::WouldBlock.into()),
            }
        })?;
        assert_eq!(read, 1, "only the datagram counts");
        assert_eq!(b.poll()?.len(), 1, "the beacon after the errors is read");

        let err = b.receive(|_| Err(io::ErrorKind::PermissionDenied.into()));
        assert!(
            matches!(err, Err(crate::Error::Io(ref e)) if e.kind() == io::ErrorKind::PermissionDenied),
            "{err:?}"
        );
        Ok(())
    }
}
