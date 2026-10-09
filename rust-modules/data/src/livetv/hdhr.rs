//! **The HDHomeRun HTTP surface** — `discover.json`, `lineup.json` and the UPnP `device.xml` —
//! as Tunarr serves it (`server/src/api/hdhrApi.ts`, `services/HDHRService.ts`).
//!
//! Pure parsing: the fetches are `source.rs`'s. Fields are read softly (a number where a string
//! was expected, a missing optional) because an HDHomeRun look-alike is any server that answers
//! these paths, and only the three lineup fields are load-bearing.

use serde_json::Value;

/// `discover.json`: what the device says it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Device {
    pub friendly_name: String,
    pub device_id: String,
    /// Advertised, not enforced by Tunarr (`hdhrSettings().tunerCount`, default 2).
    pub tuner_count: u32,
    /// The origin the device names for itself, e.g. `http://192.0.2.20:8000`.
    pub base_url: String,
    pub lineup_url: String,
}

/// One `lineup.json` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineupEntry {
    /// `GuideNumber` — the channel number as the device spells it (`"12"`, `"5.1"`).
    pub number: String,
    pub name: String,
    /// The channel's continuous MPEG-TS stream.
    pub url: String,
}

/// Tunarr's fixed UPnP device UDN (`HDHRService.ts`). Every Tunarr install answers SSDP with it,
/// which is how a search tells a Tunarr from any other UPnP media server on the LAN.
pub const TUNARR_UDN: &str = "uuid:d936e232-6671-4cd7-a8ab-34b5956ff4d6";

/// The SSDP search target Tunarr advertises (and every real HDHomeRun answers too).
pub const SSDP_TARGET: &str = "urn:schemas-upnp-org:device:MediaServer:1";

fn text(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.trim().to_owned(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// Parse `discover.json`. `None` when the body is not a JSON object.
pub fn parse_device(body: &[u8]) -> Option<Device> {
    let v: Value = serde_json::from_slice(body).ok()?;
    if !v.is_object() {
        return None;
    }
    let tuner_count = v.get("TunerCount").and_then(|n| n.as_u64().or_else(|| n.as_str()?.trim().parse().ok())).unwrap_or(0);
    Some(Device {
        friendly_name: text(&v, "FriendlyName"),
        device_id: text(&v, "DeviceID"),
        tuner_count: u32::try_from(tuner_count).unwrap_or(u32::MAX),
        base_url: text(&v, "BaseURL").trim_end_matches('/').to_owned(),
        lineup_url: text(&v, "LineupURL"),
    })
}

/// Parse `lineup.json`. `None` when the body is not a JSON array; rows missing a number or a URL
/// are dropped, and so is Tunarr's empty-install placeholder (one row whose URL is `{origin}/setup`
/// — `hdhrApi.ts` writes it when no channel exists, and it is a web page, not a stream).
pub fn parse_lineup(body: &[u8]) -> Option<Vec<LineupEntry>> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let rows = v.as_array()?;
    Some(
        rows.iter()
            .filter_map(|row| {
                let number = text(row, "GuideNumber");
                let url = text(row, "URL");
                if number.is_empty() || url.is_empty() || is_setup_placeholder(&url) {
                    return None;
                }
                let name = text(row, "GuideName");
                Some(LineupEntry { name: if name.is_empty() { number.clone() } else { name }, number, url })
            })
            .collect(),
    )
}

fn is_setup_placeholder(url: &str) -> bool {
    let path = url.split_once("://").map_or(url, |(_, rest)| rest.split_once('/').map_or("", |(_, p)| p));
    let path = path.split(['?', '#']).next().unwrap_or("");
    path.trim_end_matches('/') == "setup"
}

/// `<URLBase>` out of a UPnP device description, with its trailing slash trimmed. Tunarr's
/// `device.xml` carries the origin its HDHR routes live on.
pub fn url_base(device_xml: &str) -> Option<String> {
    let start = device_xml.find("<URLBase>")? + "<URLBase>".len();
    let end = device_xml[start..].find("</URLBase>")? + start;
    let base = device_xml[start..end].trim().trim_end_matches('/');
    (!base.is_empty()).then(|| base.to_owned())
}

/// The origin a device-description URL lives on — the fallback when a `device.xml` names no
/// `URLBase`: `http://192.0.2.20:8000/device.xml` → `http://192.0.2.20:8000`.
pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    (!authority.is_empty() && !scheme.is_empty()).then(|| format!("{scheme}://{authority}"))
}

/// Normalise what a person typed as a Tunarr address into an origin: a bare `host` or `host:port`
/// gets `http://` and Tunarr's default port 8000; a pasted URL keeps its scheme and port and loses
/// any path. `None` for something with no host at all.
pub fn normalise_address(typed: &str) -> Option<String> {
    let typed = typed.trim();
    if typed.is_empty() || typed.chars().any(char::is_whitespace) {
        return None;
    }
    let (scheme, rest) = match typed.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => ("http".to_owned(), typed),
    };
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return None;
    }
    // A port is present when the text after the last ':' is digits and the host is not a bare v6
    // literal (which needs brackets to carry one).
    let has_port = match authority.rsplit_once(':') {
        Some((host, port)) => !host.is_empty() && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
            && (!host.contains(':') || host.ends_with(']')),
        None => false,
    };
    if authority.ends_with(':') {
        return None;
    }
    Some(if has_port { format!("{scheme}://{authority}") } else { format!("{scheme}://{authority}:8000") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunarrs_discover_json_parses() {
        let body = br#"{"FriendlyName":"Tunarr","Manufacturer":"Tunarr - Silicondust","ManufacturerURL":"https://github.com/chrisbenincasa/tunarr","ModelNumber":"HDTC-2US","FirmwareName":"hdhomeruntc_atsc","TunerCount":2,"FirmwareVersion":"20170930","DeviceID":"Tunarr","DeviceAuth":"","BaseURL":"http://192.0.2.20:8000","LineupURL":"http://192.0.2.20:8000/lineup.json"}"#;
        let d = parse_device(body).unwrap();
        assert_eq!(d.friendly_name, "Tunarr");
        assert_eq!(d.device_id, "Tunarr");
        assert_eq!(d.tuner_count, 2);
        assert_eq!(d.base_url, "http://192.0.2.20:8000");
        assert_eq!(d.lineup_url, "http://192.0.2.20:8000/lineup.json");
        assert_eq!(parse_device(b"[]"), None);
        assert_eq!(parse_device(b"<html>"), None);
        assert_eq!(parse_device(br#"{"TunerCount":"4"}"#).unwrap().tuner_count, 4);
    }

    #[test]
    fn the_lineup_keeps_streams_and_drops_the_setup_placeholder() {
        let body = br#"[{"GuideNumber":"1","GuideName":"Cartoons","URL":"http://192.0.2.20:8000/stream/channels/aaaa.ts"},
                        {"GuideNumber":12,"GuideName":"","URL":"http://192.0.2.20:8000/stream/channels/bbbb.ts"},
                        {"GuideNumber":"3","GuideName":"No url"}]"#;
        let l = parse_lineup(body).unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0], LineupEntry { number: "1".into(), name: "Cartoons".into(), url: "http://192.0.2.20:8000/stream/channels/aaaa.ts".into() });
        assert_eq!(l[1].number, "12", "a numeric GuideNumber reads as its text");
        assert_eq!(l[1].name, "12", "a nameless channel is named by its number");
        let empty = br#"[{"GuideNumber":"1","GuideName":"Tunarr","URL":"http://192.0.2.20:8000/setup"}]"#;
        assert_eq!(parse_lineup(empty).unwrap(), vec![], "an empty Tunarr has no channels, not a channel called Tunarr");
        assert_eq!(parse_lineup(b"{}"), None);
    }

    #[test]
    fn the_device_description_names_its_base() {
        let xml = "<root xmlns=\"urn:schemas-upnp-org:device-1-0\">\n  <URLBase>http://192.0.2.20:8000</URLBase>\n</root>";
        assert_eq!(url_base(xml).as_deref(), Some("http://192.0.2.20:8000"));
        assert_eq!(url_base("<root><URLBase> </URLBase></root>"), None);
        assert_eq!(url_base("<root/>"), None);
        assert_eq!(origin_of("http://192.0.2.20:8000/device.xml").as_deref(), Some("http://192.0.2.20:8000"));
        assert_eq!(origin_of("device.xml"), None);
    }

    #[test]
    fn a_typed_address_becomes_an_origin() {
        assert_eq!(normalise_address("192.0.2.20").as_deref(), Some("http://192.0.2.20:8000"));
        assert_eq!(normalise_address(" 192.0.2.20:8080 ").as_deref(), Some("http://192.0.2.20:8080"));
        assert_eq!(normalise_address("tunarr.local").as_deref(), Some("http://tunarr.local:8000"));
        assert_eq!(normalise_address("http://192.0.2.20:8000/web/guide").as_deref(), Some("http://192.0.2.20:8000"));
        assert_eq!(normalise_address("HTTPS://tv.example.com").as_deref(), Some("https://tv.example.com:8000"));
        assert_eq!(normalise_address("[2001:db8::1]:8000").as_deref(), Some("http://[2001:db8::1]:8000"));
        assert_eq!(normalise_address("[2001:db8::1]").as_deref(), Some("http://[2001:db8::1]:8000"));
        assert_eq!(normalise_address(""), None);
        assert_eq!(normalise_address("ftp://x"), None);
        assert_eq!(normalise_address("http://"), None);
        assert_eq!(normalise_address("192.0.2.20:"), None);
        assert_eq!(normalise_address("a b"), None);
    }
}
