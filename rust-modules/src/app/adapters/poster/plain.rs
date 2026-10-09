//! **Art fetched verbatim from an absolute URL** — the store's [`tex::PLAIN_URL`] keys. A Live TV
//! server (Tunarr) publishes channel logos and programme artwork as plain `http://` URLs on its own
//! origin, with no transcoder and no credential, so none of the Plex half of this module applies:
//! no `/photo/:/transcode`, no token, no server registry.
//!
//! Everything else does, which is why this is a branch of the one store rather than a second
//! loader: the same slots and LRU, the same two workers, the same disk tier (under
//! [`plx_platform::imgcache::classify_url`]) and the same render cache. The one job the transcoder
//! did that is now ours is the SCALING — the decode is scaled down to cover the box the draw asked
//! for ([`plx_gfx::img::img_decode_cover`]), so a large logo costs a tile's texture, not its own.
//!
//! **The key is the URL plus the box** (`{url}#plx={w}x{h}`): two boxes are two decodes of one
//! picture, exactly as two transcode sizes are two keys. The fragment never reaches the wire —
//! [`split_key`] takes it off before the request is built.

use plx_ui::tex;

/// The fragment that carries the box. Chosen so it cannot be mistaken for a real fragment a
/// server would use for anything (a fragment is never sent in a request anyway).
const BOX: &str = "#plx=";

/// The store key for `url` drawn in a `w`×`h` logical box, at the simulator's supersampled scale
/// (`surface::render_scale`, 1 on a television). `None` for anything that is not an absolute
/// `http(s)` URL — a relative path or another scheme is never fetched.
pub(super) fn plain_key(url: &str, w: i32, h: i32) -> Option<String> {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) || url.contains('#') || w <= 0 || h <= 0 {
        return None;
    }
    let n = plx_base::surface::render_scale();
    Some(format!("{url}{BOX}{}x{}", w * n, h * n))
}

/// A key back into its URL and pixel box.
pub(super) fn split_key(key: &str) -> Option<(&str, u32, u32)> {
    let (url, dims) = key.rsplit_once(BOX)?;
    let (w, h) = dims.split_once('x')?;
    Some((url, w.parse().ok()?, h.parse().ok()?))
}

/// `http://host:port/a/b?c` → (`http://host:port`, `/a/b?c`).
fn split_url(url: &str) -> Option<(&str, &str)> {
    let after = url.find("://")? + 3;
    match url[after..].find('/') {
        Some(i) => Some((&url[..after + i], &url[after + i..])),
        None => Some((url, "/")),
    }
}

/// The origin to dial. A URL that writes no port means the scheme's own (80, 443) — not
/// `Origin::parse`'s Plex default, which is right for a server address and wrong for a web URL.
fn origin_of(origin: &str) -> Option<plx_plex::plex::Origin> {
    let o = plx_plex::plex::Origin::parse(origin)?;
    let authority = &origin[origin.find("://")? + 3..];
    let port_written = authority.rsplit_once(':').is_some_and(|(host, port)| {
        !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) && (!host.starts_with('[') || host.ends_with(']'))
    });
    if port_written {
        return Some(o);
    }
    let port = if o.is_tls() { 443 } else { 80 };
    Some(plx_plex::plex::Origin::new(o.scheme(), o.host(), port))
}

/// What one load produced: the decoded picture, or why there is none.
pub(super) struct Loaded {
    /// `(w, h, rgba)`.
    pub art: Option<(u32, u32, Vec<u8>)>,
    pub from_disk: bool,
    /// The failure can change (no answer, a 5xx): the slot parks for a retry instead of failing.
    pub transient: bool,
}

/// **The blocking load** (a worker's): the disk tier, else the network. Only a response that
/// DECODES is written to disk, so a bad answer never replaces a good picture.
pub(super) fn load(key: &str, cache_gen: u64) -> Loaded {
    let mut out = Loaded { art: None, from_disk: false, transient: false };
    let Some((url, bw, bh)) = split_key(key) else { return out };
    let disk = if crate::dev::scenarios::imagecache_bypass_armed() {
        None
    } else {
        plx_platform::imgcache::classify_url(url, bw, bh)
    };
    if let Some(k) = &disk {
        if let Some(cached) = plx_platform::imgcache::read_at(cache_gen, k) {
            match plx_gfx::img::img_decode_cover(&cached.bytes, bw, bh) {
                Some(art) => {
                    out.art = Some(art);
                    out.from_disk = true;
                    return out;
                }
                None => plx_platform::imgcache::remove_at(cache_gen, k),
            }
        }
    }
    if cache_gen != plx_platform::imgcache::generation() {
        return out;
    }
    let Some((origin, path)) = split_url(url) else { return out };
    let Some(origin) = origin_of(origin) else { return out };
    super::FETCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    match plx_plex::http::request_bulk(&origin, path, plx_plex::http::Method::Get, &["Accept: image/*"], None) {
        Some(reply) if (200..300).contains(&reply.status) && !reply.body.is_empty() => {
            out.art = plx_gfx::img::img_decode_cover(&reply.body, bw, bh);
            if out.art.is_some() {
                if let Some(k) = &disk {
                    plx_platform::imgcache::write_at(cache_gen, k, &reply.body);
                }
            }
        }
        Some(reply) => out.transient = matches!(reply.status, 408 | 429 | 500..=599),
        None => out.transient = true,
    }
    out
}

/// Is this a [`tex::PLAIN_URL`] request?
pub(super) fn is_plain(srv: plx_plex::plex::ServerId) -> bool {
    srv.raw() == tex::PLAIN_URL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_carries_the_url_and_the_box_and_splits_back() {
        let k = plain_key("http://192.0.2.20:8000/images/logo.png", 96, 64).unwrap();
        let n = plx_base::surface::render_scale() as u32;
        assert_eq!(split_key(&k), Some(("http://192.0.2.20:8000/images/logo.png", 96 * n, 64 * n)));
        assert_ne!(k, plain_key("http://192.0.2.20:8000/images/logo.png", 128, 64).unwrap(), "a box is part of the key");
    }

    #[test]
    fn only_absolute_http_urls_make_a_key() {
        assert!(plain_key("https://example.com/a.png", 1, 1).is_some());
        assert!(plain_key("/library/metadata/1/thumb", 1, 1).is_none(), "a Plex path is not a plain URL");
        assert!(plain_key("file:///etc/passwd", 1, 1).is_none());
        assert!(plain_key("http://h/a.png#frag", 1, 1).is_none(), "a fragment would confuse the box");
        assert!(plain_key("http://h/a.png", 0, 1).is_none());
        assert_eq!(split_key("http://h/a.png"), None);
    }

    #[test]
    fn an_url_splits_into_its_origin_and_its_path() {
        assert_eq!(split_url("http://192.0.2.20:8000/api/x?y=1"), Some(("http://192.0.2.20:8000", "/api/x?y=1")));
        assert_eq!(split_url("http://192.0.2.20"), Some(("http://192.0.2.20", "/")));
        assert_eq!(split_url("no-scheme"), None);
    }

    #[test]
    fn a_url_without_a_port_dials_the_schemes_own() {
        assert_eq!(origin_of("http://192.0.2.20:8000").map(|o| o.port()), Some(8000));
        assert_eq!(origin_of("http://192.0.2.20").map(|o| o.port()), Some(80));
        assert_eq!(origin_of("https://images.example.com").map(|o| o.port()), Some(443));
    }
}
