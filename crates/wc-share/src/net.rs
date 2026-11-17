//! The network transport: session messages over TCP, one JSON object per line.
//!
//! The host binds a [`Listener`] and calls [`Listener::accept`] once per frame; every new socket
//! becomes a [`HostLink`](crate::transport::HostLink) in the [`Hub`]. A guest calls [`connect`] and gets a [`GuestLink`].
//! Both are the same channel links as the in-process transport: reading and writing run on two
//! threads per socket, and the window polls the link each frame without blocking.
//!
//! A line longer than the limit, or one that is not a valid message, closes that connection
//! only. A closed connection reads as [`Error::Closed`], so the hub releases the guest's locks.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::transport::{GuestLink, Hub, Link, decode, encode};
use crate::{Error, Host, PeerId, Result};

/// The longest line read from a peer by default: 64 MiB, room for the whole project sent on
/// joining, while a bad peer cannot fill the memory.
pub const MAX_LINE: usize = 64 * 1024 * 1024;

/// How long [`connect`] tries each address of the host.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The host's listening socket.
#[derive(Debug)]
pub struct Listener {
    inner: TcpListener,
    max_line: usize,
}

impl Listener {
    /// Listen on `addr`, for example `0.0.0.0:7878`, or `127.0.0.1:0` for any free port.
    pub fn bind(addr: impl ToSocketAddrs) -> Result<Self> {
        let inner = TcpListener::bind(addr)?;
        inner.set_nonblocking(true)?;
        Ok(Self {
            inner,
            max_line: MAX_LINE,
        })
    }

    /// The address and port it listens on.
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.inner.local_addr()?)
    }

    /// The longest line accepted from a guest; longer ones close that connection.
    pub fn set_max_line(&mut self, max: usize) {
        self.max_line = max;
    }

    /// Take every connection that has arrived since the last call and add it to the hub.
    /// Returns the new participant ids; each may ask to join next. Never blocks.
    pub fn accept(&mut self, hub: &mut Hub, host: &mut Host) -> Vec<PeerId> {
        let mut new = Vec::new();
        loop {
            match self.inner.accept() {
                Ok((stream, _)) => {
                    if let Ok(link) = spawn_link(stream, self.max_line) {
                        new.push(hub.add(host, link));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                // WouldBlock: nothing more waiting. Anything else: try again next frame.
                Err(_) => break,
            }
        }
        new
    }
}

/// Connect to a host, for example `192.168.1.20:7878`.
pub fn connect(addr: impl ToSocketAddrs) -> Result<GuestLink> {
    connect_with_max_line(addr, MAX_LINE)
}

/// [`connect`], with the longest line accepted from the host.
pub fn connect_with_max_line(addr: impl ToSocketAddrs, max_line: usize) -> Result<GuestLink> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no address to connect to");
    for a in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
            Ok(stream) => return spawn_link(stream, max_line),
            Err(e) => last = e,
        }
    }
    Err(Error::Io(last))
}

/// Run a socket as a link: a thread reads lines into the link, another writes what is sent.
/// Dropping the link ends the writer, which closes the socket and so ends the reader.
fn spawn_link<S, R>(stream: TcpStream, max_line: usize) -> Result<Link<S, R>>
where
    S: Serialize + Send + 'static,
    R: DeserializeOwned + Send + 'static,
{
    // An accepted socket can inherit non-blocking mode from the listener on some systems.
    stream.set_nonblocking(false)?;
    // Small messages (a page lock) should not wait for more data.
    stream.set_nodelay(true)?;
    let read_half = stream.try_clone()?;
    let (to_link, from_net) = channel::<R>();
    let (to_net, from_link) = channel::<S>();

    thread::Builder::new().name("wc-share-read".into()).spawn(move || {
        let mut reader = BufReader::new(read_half);
        let mut line = Vec::new();
        // Ends at the end of the stream, on a read error or on an over-long line.
        while let Ok(true) = read_line(&mut reader, &mut line, max_line) {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let msg = match std::str::from_utf8(&line).map(decode::<R>) {
                Ok(Ok(msg)) => msg,
                _ => break,
            };
            if to_link.send(msg).is_err() {
                break;
            }
        }
        // Dropping `to_link` tells the link the connection is closed; the shutdown stops the
        // writer's next write too.
        let _ = reader.get_ref().shutdown(Shutdown::Both);
    })?;

    let mut write_half = stream;
    thread::Builder::new().name("wc-share-write".into()).spawn(move || {
        for msg in from_link.iter() {
            let ok = encode(&msg)
                .ok()
                .is_some_and(|line| write_half.write_all(line.as_bytes()).is_ok());
            if !ok || write_half.flush().is_err() {
                break;
            }
        }
        let _ = write_half.shutdown(Shutdown::Both);
    })?;

    Ok(Link::from_channels(to_net, from_net))
}

/// Read one line, without its `\n`, into `buf`. `Ok(false)` at the end of the stream (a partial
/// last line is dropped). A line longer than `max` bytes is an error, read no further than that.
fn read_line(r: &mut impl BufRead, buf: &mut Vec<u8>, max: usize) -> io::Result<bool> {
    buf.clear();
    loop {
        let avail = match r.fill_buf() {
            Ok(a) => a,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if avail.is_empty() {
            return Ok(false);
        }
        let (take, done) = match avail.iter().position(|&b| b == b'\n') {
            Some(i) => (i, true),
            None => (avail.len(), false),
        };
        if buf.len().saturating_add(take) > max {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "session line too long"));
        }
        buf.extend_from_slice(&avail[..take]);
        r.consume(if done { take + 1 } else { take });
        if done {
            return Ok(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_split_and_capped() -> io::Result<()> {
        let mut r = io::Cursor::new(b"ab\n\ncdef\ntail".to_vec());
        let mut buf = Vec::new();
        assert!(read_line(&mut r, &mut buf, 4)?);
        assert_eq!(buf, b"ab");
        assert!(read_line(&mut r, &mut buf, 4)?);
        assert!(buf.is_empty());
        assert!(read_line(&mut r, &mut buf, 4)?);
        assert_eq!(buf, b"cdef");
        assert!(!read_line(&mut r, &mut buf, 4)?, "a partial last line is dropped");

        let mut r = io::Cursor::new(b"abcde\n".to_vec());
        assert!(read_line(&mut r, &mut buf, 4).is_err());
        Ok(())
    }
}
