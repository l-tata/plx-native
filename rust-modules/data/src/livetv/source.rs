//! **Talking to the Tunarr server**: finding it on the LAN, and the three reads a guide is built
//! from (`discover.json`, `lineup.json`, `/api/xmltv.xml`).
//!
//! Every function here BLOCKS (an HTTP round trip, or a multi-second SSDP window) and is run by
//! the Live TV store's worker, never on the frame thread — `plx_plex::http`'s own guard aborts a
//! developer build that tries. No request carries a credential: Tunarr's HDHR, stream and XMLTV
//! routes are `authRequired: false`, so these reads never reach the app's token policy.

use super::guide::Lineup;
use super::hdhr::{self, Device, LineupEntry};
use super::xmltv::{self, Guide};
use plx_plex::http::{self, Method};
use plx_plex::plex::Origin;
use std::time::Duration;

/// Why a read failed, in the words the setup page reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// The origin could not be parsed (a corrupt stored address).
    BadAddress,
    /// Nothing answered: refused, timed out, unroutable.
    Unreachable,
    /// The server answered with this HTTP status.
    Status(i32),
    /// It answered 200, but not with what an HDHomeRun / XMLTV endpoint returns.
    NotTunarr,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::BadAddress => f.write_str("bad address"),
            FetchError::Unreachable => f.write_str("unreachable"),
            FetchError::Status(s) => write!(f, "HTTP {s}"),
            FetchError::NotTunarr => f.write_str("not an HDHomeRun/XMLTV endpoint"),
        }
    }
}

/// A Tunarr (or any HDHomeRun-compatible server) found on the LAN.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The origin its HDHR routes live on.
    pub origin: String,
    /// `discover.json`'s `FriendlyName`.
    pub name: String,
}

/// How long a LAN search listens. SSDP replies arrive after a random delay of up to `MX`
/// seconds; three is the UPnP spec's usual figure and keeps the setup page's spinner short.
pub const SEARCH_WINDOW: Duration = Duration::from_secs(3);

/// How much guide is kept: from an hour ago (the airing on now may have started long before) to
/// a day ahead (Tunarr's default horizon is 12 h; a longer one is kept up to this).
pub const GUIDE_BEHIND_MS: i64 = 60 * 60 * 1000;
pub const GUIDE_AHEAD_MS: i64 = 24 * 60 * 60 * 1000;

fn get(origin: &Origin, path: &str, accept: &str) -> Result<Vec<u8>, FetchError> {
    let header = format!("Accept: {accept}");
    let reply = http::request_bulk(origin, path, Method::Get, &[header.as_str()], None).ok_or(FetchError::Unreachable)?;
    if !(200..300).contains(&reply.status) {
        return Err(FetchError::Status(reply.status));
    }
    Ok(reply.body)
}

fn parse_origin(origin: &str) -> Result<Origin, FetchError> {
    Origin::parse(origin).ok_or(FetchError::BadAddress)
}

/// `discover.json` at `origin`.
pub fn fetch_device(origin: &str) -> Result<Device, FetchError> {
    let o = parse_origin(origin)?;
    let body = get(&o, "/discover.json", "application/json")?;
    hdhr::parse_device(&body).ok_or(FetchError::NotTunarr)
}

/// `lineup.json` at `origin` — always the origin's own path, never the `LineupURL` a device names:
/// a server behind a proxy or NAT routinely advertises an address the television cannot reach,
/// and the origin the person chose is the one that demonstrably answers.
pub fn fetch_lineup(origin: &str) -> Result<Vec<LineupEntry>, FetchError> {
    let o = parse_origin(origin)?;
    let body = get(&o, "/lineup.json", "application/json")?;
    hdhr::parse_lineup(&body).ok_or(FetchError::NotTunarr)
}

/// Tunarr's XMLTV guide, keeping `[now - GUIDE_BEHIND_MS, now + GUIDE_AHEAD_MS)`.
pub fn fetch_guide(origin: &str, now_ms: i64) -> Result<Guide, FetchError> {
    let o = parse_origin(origin)?;
    let body = get(&o, "/api/xmltv.xml", "application/xml")?;
    xmltv::parse(&body, now_ms - GUIDE_BEHIND_MS, now_ms + GUIDE_AHEAD_MS).map_err(|e| {
        plx_base::eventlog::log(&format!("livetv: guide rejected: {e}"));
        FetchError::NotTunarr
    })
}

/// Everything a guide is built from, in one blocking pass: the device, its channels and — when
/// the guide answers — their airings. A failed GUIDE is not a failed load (the channels still
/// tune); a failed device or lineup is.
pub fn load(origin: &str, now_ms: i64) -> Result<(Device, Lineup, Option<FetchError>), FetchError> {
    let device = fetch_device(origin)?;
    let entries = fetch_lineup(origin)?;
    let (guide, guide_error) = match fetch_guide(origin, now_ms) {
        Ok(g) => (Some(g), None),
        Err(e) => {
            plx_base::eventlog::log(&format!("livetv: guide unavailable ({e}); channels listed without airings"));
            (None, Some(e))
        }
    };
    Ok((device, Lineup::join(entries, guide.as_ref()), guide_error))
}

/// Search the LAN for Tunarr: an SSDP search for the media-server target Tunarr advertises, then
/// `discover.json` at each responder's base. A responder that does not answer `discover.json` is
/// some other UPnP device (a NAS, a renderer) and is skipped. Tunarr's own replies (its fixed UDN)
/// are listed first.
pub fn discover() -> Vec<Found> {
    let replies = match plx_net::ssdp::search(hdhr::SSDP_TARGET, SEARCH_WINDOW) {
        Ok(r) => r,
        Err(e) => {
            plx_base::eventlog::log(&format!("livetv: LAN search could not be sent ({e})"));
            return Vec::new();
        }
    };
    let mut ordered: Vec<&plx_net::ssdp::Reply> = replies.iter().collect();
    ordered.sort_by_key(|r| !r.usn.contains(hdhr::TUNARR_UDN));
    let mut found: Vec<Found> = Vec::new();
    for reply in ordered {
        let Some(base) = base_of(reply) else { continue };
        if found.iter().any(|f| f.origin == base) {
            continue;
        }
        if let Ok(device) = fetch_device(&base) {
            let name = if device.friendly_name.is_empty() { "Tunarr".to_owned() } else { device.friendly_name };
            found.push(Found { origin: base, name });
        }
    }
    plx_base::eventlog::log(&format!("livetv: LAN search replies={} servers={}", replies.len(), found.len()));
    found
}

/// The HDHR origin behind an SSDP reply: the `URLBase` its `device.xml` names, else the origin of
/// the `LOCATION` itself.
fn base_of(reply: &plx_net::ssdp::Reply) -> Option<String> {
    let location_origin = hdhr::origin_of(&reply.location)?;
    let o = Origin::parse(&location_origin)?;
    let path = reply.location.splitn(4, '/').nth(3).map(|p| format!("/{p}")).unwrap_or_else(|| "/".to_owned());
    let named = get(&o, &path, "text/xml")
        .ok()
        .and_then(|body| String::from_utf8(body).ok())
        .and_then(|xml| hdhr::url_base(&xml));
    Some(named.unwrap_or(location_origin))
}
