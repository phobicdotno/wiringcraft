//! The TCP transport seen from outside the crate: binding, accepting, connecting, and a guest
//! link talking to a plain socket. Everything stays on this computer (127.0.0.1).

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{SocketAddr, TcpListener};
use std::time::{Duration, Instant};

use wc_core::Project;
use wc_share::net::{Listener, connect, connect_with_max_line};
use wc_share::transport::{CloseReason, GuestLink, Hub, decode, encode};
use wc_share::{Error, GuestMsg, Host, HostMsg, MemoryJournal, PROTOCOL_VERSION};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

const PATIENCE: Duration = Duration::from_secs(10);

fn host() -> Host {
    Host::new(
        Project::new("Demo"),
        "Hanna Host",
        "hanna@example.com",
        Box::new(MemoryJournal::default()),
    )
}

/// Wait (without sleeping) until the link reports closed, then return why.
fn reason_after_close(link: &GuestLink) -> Option<CloseReason> {
    let start = Instant::now();
    while start.elapsed() < PATIENCE {
        match link.try_recv() {
            Err(Error::Closed) => return link.close_reason(),
            _ => std::thread::yield_now(),
        }
    }
    None
}

/// Wait (without sleeping) for the next message on the link. A link that closes first, or
/// a wait that runs out, is an error that says which, with the close reason.
fn next_msg(link: &GuestLink) -> Result<HostMsg, String> {
    let start = Instant::now();
    while start.elapsed() < PATIENCE {
        match link.try_recv() {
            Ok(Some(msg)) => return Ok(msg),
            Ok(None) => std::thread::yield_now(),
            Err(e) => {
                return Err(format!(
                    "the link failed before a message came: {e} (close reason: {:?})",
                    link.close_reason()
                ));
            }
        }
    }
    Err(format!("no message within {PATIENCE:?}"))
}

#[test]
fn a_poll_with_nobody_waiting_takes_nothing() -> TestResult {
    let mut l = Listener::bind("127.0.0.1:0")?;
    let addr = l.local_addr()?;
    assert!(addr.ip().is_loopback());
    assert_ne!(addr.port(), 0, "port 0 is replaced by the port picked");

    let (mut hub, mut host) = (Hub::new(), host());
    assert!(l.accept(&mut hub, &mut host).is_empty());
    assert!(l.accept(&mut hub, &mut host).is_empty(), "polling again never blocks");
    assert!(hub.is_empty());
    assert!(l.take_errors().is_empty());
    Ok(())
}

#[test]
fn every_waiting_connection_gets_its_own_id() -> TestResult {
    let mut l = Listener::bind("127.0.0.1:0")?;
    let addr = l.local_addr()?;
    let links = [connect(addr)?, connect(addr)?, connect(addr)?];

    let (mut hub, mut host) = (Hub::new(), host());
    let mut ids = Vec::new();
    let start = Instant::now();
    while ids.len() < links.len() && start.elapsed() < PATIENCE {
        ids.extend(l.accept(&mut hub, &mut host));
        std::thread::yield_now();
    }
    assert_eq!(ids.len(), 3);
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 3, "ids are distinct: {ids:?}");
    assert_eq!(hub.len(), 3);
    assert!(l.take_errors().is_empty());
    Ok(())
}

#[test]
fn binding_a_port_in_use_fails() -> TestResult {
    let first = Listener::bind("127.0.0.1:0")?;
    let addr = first.local_addr()?;
    assert!(matches!(Listener::bind(addr), Err(Error::Io(_))));
    Ok(())
}

#[test]
fn connecting_where_nobody_listens_fails() -> TestResult {
    // A port that was free a moment ago and is closed now.
    let addr = TcpListener::bind("127.0.0.1:0")?.local_addr()?;
    assert!(matches!(connect(addr), Err(Error::Io(_))));
    Ok(())
}

#[test]
fn connecting_to_no_address_says_so() {
    let none: &[SocketAddr] = &[];
    assert!(matches!(connect(none), Err(Error::Io(e)) if e.kind() == ErrorKind::NotFound));
}

#[test]
fn a_guest_link_speaks_one_json_line_per_message() -> TestResult {
    let server = TcpListener::bind("127.0.0.1:0")?;
    let link = connect(server.local_addr()?)?;
    let (raw, _) = server.accept()?;
    raw.set_read_timeout(Some(PATIENCE))?;

    let join = GuestMsg::Join {
        name: "Ola".into(),
        email: "ola@example.com".into(),
        protocol: PROTOCOL_VERSION,
    };
    link.send(join.clone())?;
    let mut reader = BufReader::new(raw.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    assert!(line.ends_with('\n'));
    assert_eq!(line.matches('\n').count(), 1, "one line: {line:?}");
    assert_eq!(decode::<GuestMsg>(&line)?, join);

    // Blank lines between messages are skipped, not taken as bad messages.
    let mut writer = raw;
    writer.write_all(b"\n  \r\n")?;
    writer.write_all(encode(&HostMsg::Waiting)?.as_bytes())?;
    assert_eq!(next_msg(&link)?, HostMsg::Waiting);
    assert_eq!(link.close_reason(), None, "still open");

    // The host closing in order reads as closed, and as ended.
    writer.shutdown(std::net::Shutdown::Both)?;
    drop(writer);
    drop(reader);
    let reason = reason_after_close(&link);
    assert_eq!(reason, Some(CloseReason::Ended));
    Ok(())
}

#[test]
fn a_guest_caps_lines_from_the_host() -> TestResult {
    let server = TcpListener::bind("127.0.0.1:0")?;
    let link = connect_with_max_line(server.local_addr()?, 64)?;
    let (mut raw, _) = server.accept()?;

    // A short message fits; the next one is too long and closes the link.
    raw.write_all(encode(&HostMsg::Waiting)?.as_bytes())?;
    let waiting = encode(&HostMsg::Waiting)?;
    assert!(waiting.len() <= 64, "{waiting:?}");
    raw.write_all(&[b'x'; 256])?;
    assert_eq!(next_msg(&link)?, HostMsg::Waiting);
    let reason = reason_after_close(&link);
    assert_eq!(reason, Some(CloseReason::LineTooLong { max: 64 }));
    assert!(!reason.is_some_and(|r| r.is_orderly()));
    Ok(())
}
