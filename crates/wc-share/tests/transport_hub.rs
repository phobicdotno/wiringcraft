//! How the host's [`Hub`] drops connections: which ends are reported by `take_closed` and
//! which are not, and what a failed send does.

use wc_share::transport::{CloseReason, GuestLink, Hub, pair};
use wc_share::{Error, GuestMsg, Host, HostMsg, MemoryJournal, PROTOCOL_VERSION, PeerId};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn host() -> Host {
    Host::new(
        wc_core::Project::new("Demo"),
        "Hanna",
        "hanna@example.com",
        Box::new(MemoryJournal::default()),
    )
}

fn join(name: &str) -> GuestMsg {
    GuestMsg::Join {
        name: name.into(),
        email: format!("{}@example.com", name.to_lowercase()),
        protocol: PROTOCOL_VERSION,
    }
}

/// A new connection that has asked to join and waits for approval.
fn waiting(
    hub: &mut Hub,
    host: &mut Host,
    name: &str,
) -> std::result::Result<(PeerId, GuestLink), Box<dyn std::error::Error>> {
    let (guest, host_end) = pair();
    let id = hub.add(host, host_end);
    guest.send(join(name))?;
    hub.pump(host);
    assert!(matches!(guest.drain().as_slice(), [HostMsg::Waiting]));
    Ok((id, guest))
}

#[test]
fn a_guest_who_leaves_is_dropped_but_not_reported_as_closed() -> TestResult {
    let mut host = host();
    let mut hub = Hub::new();
    let (_, guest) = waiting(&mut hub, &mut host, "Ola")?;
    assert_eq!(host.pending().len(), 1);
    guest.send(GuestMsg::Leave)?;
    hub.pump(&mut host);
    assert!(hub.is_empty());
    assert!(hub.take_closed().is_empty(), "leaving is not a lost connection");
    assert!(host.pending().is_empty());
    assert_eq!(host.peers().len(), 1, "only the host is left");
    // The hub dropped its end, so the guest finds the link closed in order.
    assert!(matches!(guest.try_recv(), Err(Error::Closed)));
    assert_eq!(guest.close_reason(), Some(CloseReason::Ended));
    Ok(())
}

#[test]
fn a_declined_guest_gets_the_answer_before_the_connection_is_dropped() -> TestResult {
    let mut host = host();
    let mut hub = Hub::new();
    let (ola, ola_link) = waiting(&mut hub, &mut host, "Ola")?;
    let (_, kari_link) = waiting(&mut hub, &mut host, "Kari")?;
    assert!(host.decline(ola));
    hub.flush(&mut host);
    assert_eq!(hub.len(), 1, "only the declined connection is dropped");
    assert!(hub.take_closed().is_empty(), "a decline is not a lost connection");
    assert!(matches!(
        ola_link.try_recv(),
        Ok(Some(HostMsg::Declined { version: None }))
    ));
    assert!(matches!(ola_link.try_recv(), Err(Error::Closed)));
    assert!(matches!(kari_link.try_recv(), Ok(None)), "the other guest still waits");
    Ok(())
}

#[test]
fn closing_the_session_drops_every_connection_without_reporting_them() -> TestResult {
    let mut host = host();
    let mut hub = Hub::new();
    let (_, a) = waiting(&mut hub, &mut host, "Ola")?;
    let (_, b) = waiting(&mut hub, &mut host, "Kari")?;
    host.close();
    hub.pump(&mut host);
    assert!(hub.is_empty());
    assert!(hub.take_closed().is_empty());
    for link in [&a, &b] {
        assert!(matches!(link.try_recv(), Ok(Some(HostMsg::Closed))));
        assert!(matches!(link.try_recv(), Err(Error::Closed)));
    }
    Ok(())
}

#[test]
fn a_send_that_fails_reports_the_connection_and_forgets_the_guest() -> TestResult {
    let mut host = host();
    let mut hub = Hub::new();
    let (id, guest) = waiting(&mut hub, &mut host, "Ola")?;
    drop(guest);
    // The host approves before a pump noticed the guest went: the welcome cannot be sent.
    assert!(host.approve(id));
    hub.flush(&mut host);
    assert!(hub.is_empty());
    assert_eq!(hub.take_closed(), [(id, CloseReason::Ended)]);
    assert_eq!(host.peers().len(), 1, "the guest is disconnected");
    assert!(!host.approve(id), "and cannot be approved again");
    Ok(())
}

#[test]
fn messages_for_a_connection_the_hub_does_not_hold_are_dropped() -> TestResult {
    let mut host = host();
    let mut hub = Hub::new();
    let (_, kept) = waiting(&mut hub, &mut host, "Kari")?;
    // A participant the host knows but whose connection is not in this hub.
    let stray = host.connect();
    assert!(host.receive(stray, join("Ola")).is_ok());
    hub.flush(&mut host);
    assert!(host.take_outbox().is_empty(), "the stray answer was taken and dropped");
    assert_eq!(hub.len(), 1);
    assert!(hub.take_closed().is_empty());
    assert_eq!(host.pending().len(), 2, "the stray guest still waits");
    assert!(matches!(kept.try_recv(), Ok(None)));
    Ok(())
}
