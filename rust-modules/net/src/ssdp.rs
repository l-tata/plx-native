//! **SSDP search** — the UPnP multicast discovery an HDHomeRun-style tuner answers on the LAN.
//!
//! One blocking call, [`search`]: send an `M-SEARCH` for a search target to
//! `239.255.255.250:1900`, collect every unicast reply that arrives before the deadline, and
//! return the replies' `LOCATION` (the device description URL), `USN` and `SERVER` headers. What
//! a reply MEANS — which device it is, where its API lives — is the caller's business: this module
//! knows the wire format and nothing about tuners.
//!
//! Run it on a worker. It sends the search twice (UDP is lossy and the packets are tiny, which is
//! what the UPnP spec itself recommends) and waits out the whole window, because SSDP has no "that
//! was everyone" signal: a device answers after a random delay of up to `MX` seconds.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

/// The SSDP multicast group and port (UPnP Device Architecture 1.1 §1.1.2).
const GROUP: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), 1900);

/// One device's answer to a search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    /// The device-description URL (`LOCATION`), e.g. `http://192.0.2.20:8000/device.xml`.
    pub location: String,
    /// The unique service name (`USN`), carrying the device's UDN.
    pub usn: String,
    /// The `SERVER` product tokens, empty when absent.
    pub server: String,
}

/// The `M-SEARCH` datagram for `target`, asking for replies within `mx` seconds.
pub fn request(target: &str, mx: u8) -> String {
    format!(
        "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: {}\r\nST: {}\r\n\r\n",
        mx.clamp(1, 5),
        target
    )
}

/// Parse one SSDP reply datagram. `None` for anything that is not a `200` search response with a
/// `LOCATION` — a `NOTIFY` another device multicast, a stray packet, a reply with no URL to follow.
/// Header names are case-insensitive (RFC 9110 §5.1), which real devices exercise.
pub fn parse_reply(datagram: &[u8]) -> Option<Reply> {
    let text = std::str::from_utf8(datagram).ok()?;
    let mut lines = text.split("\r\n").flat_map(|l| l.split('\n'));
    let status = lines.next()?.trim();
    let mut parts = status.split_whitespace();
    if !parts.next()?.starts_with("HTTP/") || parts.next()? != "200" {
        return None;
    }
    let (mut location, mut usn, mut server) = (None, String::new(), String::new());
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "location" => location = Some(value.to_owned()),
            "usn" => usn = value.to_owned(),
            "server" => server = value.to_owned(),
            _ => {}
        }
    }
    let location = location.filter(|l| !l.is_empty())?;
    Some(Reply { location, usn, server })
}

/// Search the LAN for `target` for `window`, returning each distinct `LOCATION` once, in arrival
/// order. An `Err` is a socket that could not be opened or a search that could not be sent at all
/// (no IPv4 route); an empty `Ok` is a search nobody answered.
pub fn search(target: &str, window: Duration) -> std::io::Result<Vec<Reply>> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    // TTL 2 is the UPnP recommendation: one router hop at most, never the internet.
    let _ = socket.set_multicast_ttl_v4(2);
    let mx = window.as_secs().clamp(1, 5) as u8;
    let datagram = request(target, mx);
    socket.send_to(datagram.as_bytes(), GROUP)?;
    let deadline = Instant::now() + window;
    let mut resent = false;
    let mut found: Vec<Reply> = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        if !resent && deadline - now < window / 2 {
            let _ = socket.send_to(datagram.as_bytes(), GROUP);
            resent = true;
        }
        let wait = (deadline - now).min(Duration::from_millis(250)).max(Duration::from_millis(1));
        socket.set_read_timeout(Some(wait))?;
        match socket.recv_from(&mut buf) {
            Ok((n, _)) => {
                if let Some(reply) = parse_reply(&buf[..n]) {
                    if !found.iter().any(|r| r.location == reply.location) {
                        found.push(reply);
                    }
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return if found.is_empty() { Err(e) } else { Ok(found) },
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_names_the_target_and_clamps_mx() {
        let r = request("upnp:rootdevice", 9);
        assert!(r.starts_with("M-SEARCH * HTTP/1.1\r\n"));
        assert!(r.contains("\r\nST: upnp:rootdevice\r\n"));
        assert!(r.contains("\r\nMX: 5\r\n"));
        assert!(r.contains("MAN: \"ssdp:discover\""));
        assert!(r.ends_with("\r\n\r\n"));
    }

    #[test]
    fn a_search_response_yields_its_location_usn_and_server() {
        let reply = b"HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\nlocation: http://192.0.2.20:8000/device.xml\r\nServer: Tunarr/0.1 UPnP/1.0\r\nST: urn:schemas-upnp-org:device:MediaServer:1\r\nUSN: uuid:d936e232-6671-4cd7-a8ab-34b5956ff4d6::urn:schemas-upnp-org:device:MediaServer:1\r\n\r\n";
        let r = parse_reply(reply).unwrap();
        assert_eq!(r.location, "http://192.0.2.20:8000/device.xml");
        assert!(r.usn.starts_with("uuid:d936e232-6671-4cd7-a8ab-34b5956ff4d6"));
        assert_eq!(r.server, "Tunarr/0.1 UPnP/1.0");
    }

    #[test]
    fn notifies_errors_and_replies_without_a_location_are_not_replies() {
        assert_eq!(parse_reply(b"NOTIFY * HTTP/1.1\r\nLOCATION: http://x/\r\n\r\n"), None);
        assert_eq!(parse_reply(b"HTTP/1.1 404 Not Found\r\nLOCATION: http://x/\r\n\r\n"), None);
        assert_eq!(parse_reply(b"HTTP/1.1 200 OK\r\nUSN: uuid:x\r\n\r\n"), None);
        assert_eq!(parse_reply(b"HTTP/1.1 200 OK\r\nLOCATION:\r\n\r\n"), None);
        assert_eq!(parse_reply(&[0xff, 0xfe]), None);
    }
}
