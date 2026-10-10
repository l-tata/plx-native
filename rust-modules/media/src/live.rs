//! **Live TV playback**: what a channel's stream IS, and which channel the player is on.
//!
//! A Tunarr channel is one continuous MPEG-TS stream (`/stream/channels/{uuid}.ts`) with no
//! Plex decision behind it, so nobody has told the app its codecs or frame rate. The Starfish Load
//! payload must declare both before the first byte is fed (`player/CLAUDE.md`: a 24p H.264 stream
//! declared at 60 presents at 13 fps), so a tune first PROBES the stream: [`probe`] reads its first
//! seconds, [`analyse`] finds the PAT and PMT (the codecs) and measures the video PES timestamps
//! (the frame rate). The probe's connection is closed before playback opens its own; Tunarr keeps
//! the channel's transcode alive for several seconds after the last viewer leaves
//! (`stream/Session.ts`'s cleanup delay), so the real open reattaches to a warm session.
//!
//! [`LiveSession`] rides on the `PlaybackSession` while a channel plays: the joined lineup the
//! viewer is zapping through, the channel index and the one before it. `route` keeps it; the
//! player's live banner and the channel keys read it.

use plx_data::livetv::guide::{Channel, Lineup};
use std::ffi::CString;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The channel the player is on.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveSession {
    pub lineup: Arc<Lineup>,
    pub index: usize,
    /// The channel watched before this one, for "last channel".
    pub previous: Option<usize>,
    /// What the probe found, once it has (`None` while tuning).
    pub facts: Option<StreamFacts>,
    /// The tune failed (the probe could not reach the stream, or found nothing playable in it):
    /// the banner says so and OK retries.
    pub failed: bool,
    /// On a virtual channel (`plx_data::vchannel`): the wall-clock instant the programme playing
    /// began on the channel's timeline — position 0 of the item — so how far the viewer has
    /// fallen behind live (a pause) is [`LiveSession::behind_ms`]. `None` on a Tunarr channel.
    pub airing_start_ms: Option<i64>,
}

/// How far behind live a virtual channel must be before OK jumps back rather than only raising
/// the banner, and the banner says so.
pub const JUMP_TO_LIVE_MS: i64 = 10_000;

impl LiveSession {
    /// A channel about to be tuned.
    pub fn tuning(lineup: Arc<Lineup>, index: usize, previous: Option<usize>) -> Self {
        Self { lineup, index, previous, facts: None, failed: false, airing_start_ms: None }
    }
}

impl LiveSession {
    pub fn channel(&self) -> Option<&Channel> {
        self.lineup.channels.get(self.index)
    }

    /// Is this one of the app's own virtual channels (its programmes are library items the app
    /// plays itself) rather than a Tunarr stream?
    pub fn is_virtual(&self) -> bool {
        self.channel().is_some_and(|c| plx_data::livetv::virtual_playlist(&c.url).is_some())
    }

    /// Has a virtual channel fallen far enough behind live (a pause) that OK jumps back?
    pub fn is_behind(&self, now_ms: i64, playpos_ns: i64) -> bool {
        self.behind_ms(now_ms, playpos_ns) >= JUMP_TO_LIVE_MS
    }

    /// How far behind live a virtual channel is at `now_ms` with the item at `playpos_ns`: 0 when
    /// on time (or ahead, or not a virtual channel).
    pub fn behind_ms(&self, now_ms: i64, playpos_ns: i64) -> i64 {
        match self.airing_start_ms {
            Some(start) => ((now_ms - start) - playpos_ns / 1_000_000).max(0),
            None => 0,
        }
    }
}

/// The declaration a channel's stream needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamFacts {
    /// `"h264"` or `"hevc"` — the route's codec spellings.
    pub vcodec: &'static str,
    /// `"aac"`, `"ac3"` or `"eac3"`; `""` when the stream carries no audio this pipeline decodes.
    pub acodec: &'static str,
    /// Frames per second, 0 when the timestamps did not say.
    pub fps: f64,
    /// The coded picture size, `(0, 0)` when the probe did not see it (read from an H.264
    /// sequence parameter set; an HEVC channel is declared 4K whatever its size). It decides the
    /// Load's sink envelope: a 4K H.264 channel declared as 1080p is given a 1080p decoder.
    pub raster: (u16, u16),
}

impl StreamFacts {
    /// What Tunarr sends unless its transcode config says otherwise (`TranscodeConfig.ts`): H.264,
    /// AAC, and a rate the probe could not measure.
    pub const TUNARR_DEFAULT: StreamFacts = StreamFacts { vcodec: "h264", acodec: "aac", fps: 0.0, raster: (0, 0) };
}

/// Why a probe could not say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeError {
    BadUrl,
    /// The connection failed, or the server answered with this status (0 = no answer at all).
    Http(i32),
    /// Bytes arrived but no PMT with a video stream this pipeline plays was found in them.
    NotPlayable(&'static str),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::BadUrl => f.write_str("bad stream URL"),
            ProbeError::Http(0) => f.write_str("no answer"),
            ProbeError::Http(s) => write!(f, "HTTP {s}"),
            ProbeError::NotPlayable(why) => f.write_str(why),
        }
    }
}

const TS_PACKET: usize = 188;
/// How much stream a probe reads at most, and how long it waits for it. A probe stops as soon as it
/// has the answer (the PMT, a sequence parameter set and [`PTS_SAMPLES`] frames), so this only
/// bounds a high-bitrate channel: a 4K UHD stream at 60 Mbit/s spends 6 MB in under a second, the
/// time [`PTS_SAMPLES`] frames take to arrive. (It was 1.5 MB, a quarter of a second of 4K.)
const PROBE_BYTES: usize = 6 * 1024 * 1024;
const PROBE_WINDOW: Duration = Duration::from_secs(12);
/// How long a probe that already has the rate waits on for an H.264 sequence parameter set.
const RASTER_WAIT: Duration = Duration::from_secs(2);
/// Video timestamps a rate measurement wants: under a second of 30p. The median of their gaps,
/// snapped to the broadcast/film rate within 1 %, needs a handful of steady frames, not seconds of
/// them — and a probe reads the stream in real time, so every sample here is tune latency.
const PTS_SAMPLES: usize = 24;

/// Read the head of a channel's stream and [`analyse`] it. Blocks for up to [`PROBE_WINDOW`]
/// (Tunarr starts an FFmpeg for a cold channel before its first byte); run it on a worker.
pub fn probe(url: &str) -> Result<StreamFacts, ProbeError> {
    let origin = plx_plex::plex::Origin::parse(url).ok_or(ProbeError::BadUrl)?;
    if origin.scheme() != plx_plex::plex::Scheme::Http {
        // The plaintext socket is the only transport a probe has; an HTTPS Tunarr plays with the
        // default declaration.
        return Ok(StreamFacts::TUNARR_DEFAULT);
    }
    let path = url
        .split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..]))
        .unwrap_or("/");
    let host = CString::new(origin.host()).map_err(|_| ProbeError::BadUrl)?;
    let path_c = CString::new(path).map_err(|_| ProbeError::BadUrl)?;
    let extra = CString::new("").map_err(|_| ProbeError::BadUrl)?;
    let mut hs = plx_net::stream::http_stream_boxed();
    let hs_ptr: *mut plx_net::stream::HttpStream = &mut *hs;
    let rc = plx_net::stream::http_open(hs_ptr, host.as_ptr(), origin.port(), path_c.as_ptr(), extra.as_ptr(), "GET");
    let status = plx_net::stream::hs_status(hs_ptr);
    if rc != 0 || !(200..300).contains(&status) {
        plx_net::stream::http_close(hs_ptr);
        return Err(ProbeError::Http(if rc != 0 { 0 } else { status }));
    }
    let started = Instant::now();
    let mut buf = vec![0u8; PROBE_BYTES];
    let mut got = 0usize;
    let mut result = Err(ProbeError::NotPlayable("no programme table in the stream"));
    while got < buf.len() && started.elapsed() < PROBE_WINDOW {
        let want = (buf.len() - got).min(64 * 1024) as i32;
        let n = plx_net::stream::http_read(hs_ptr, buf[got..].as_mut_ptr(), want);
        if n <= 0 {
            break;
        }
        got += n as usize;
        // Stop as soon as the answer is complete, not when the buffer is full.
        if let Ok(facts) = analyse(&buf[..got]) {
            // An H.264 channel's size comes with its next keyframe's SPS: worth waiting for (a 4K
            // channel declared 1080p gets a 1080p decoder), but only so long — past
            // [`RASTER_WAIT`] the default declaration is used and the first frame tells the rest.
            let sized = facts.vcodec != "h264" || facts.raster != (0, 0) || started.elapsed() >= RASTER_WAIT;
            let done = facts.fps > 0.0 && sized;
            result = Ok(facts);
            if done {
                break;
            }
        }
    }
    plx_net::stream::http_close(hs_ptr);
    if result.is_err() {
        result = analyse(&buf[..got]);
    }
    result
}

/// Find the PAT and PMT in a run of transport packets, and measure the video frame rate from its
/// PES timestamps.
pub fn analyse(ts: &[u8]) -> Result<StreamFacts, ProbeError> {
    let start = sync_offset(ts).ok_or(ProbeError::NotPlayable("no MPEG-TS sync"))?;
    let packets = ts[start..].chunks_exact(TS_PACKET);
    let mut pmt_pid: Option<u16> = None;
    let mut pmt_seen = false;
    let mut video: Option<(u16, &'static str)> = None;
    let mut audio: Option<&'static str> = None;
    let mut pts: Vec<i64> = Vec::new();
    let mut raster = (0u16, 0u16);
    for pkt in packets {
        if pkt[0] != 0x47 {
            continue;
        }
        let pusi = pkt[1] & 0x40 != 0;
        let pid = (u16::from(pkt[1] & 0x1f) << 8) | u16::from(pkt[2]);
        let Some(payload) = payload_of(pkt) else { continue };
        if pid == 0 && pusi && pmt_pid.is_none() {
            pmt_pid = pat_first_pmt(payload);
        } else if Some(pid) == pmt_pid && pusi && video.is_none() {
            if let Some((v, a)) = pmt_streams(payload) {
                pmt_seen = true;
                video = v;
                audio = a;
            }
        } else if video.is_some_and(|(vpid, _)| vpid == pid) && pusi {
            if pts.len() < PTS_SAMPLES {
                if let Some(t) = pes_pts(payload) {
                    pts.push(t);
                }
            }
            if raster == (0, 0) && video.is_some_and(|(_, c)| c == "h264") {
                raster = pes_es(payload).and_then(h264_sps_raster).unwrap_or((0, 0));
            }
        }
    }
    let (_, vcodec) = video.ok_or(if pmt_seen {
        ProbeError::NotPlayable("no H.264 or HEVC video in the stream")
    } else {
        ProbeError::NotPlayable("no programme table in the stream")
    })?;
    Ok(StreamFacts { vcodec, acodec: audio.unwrap_or(""), fps: rate_of(&pts), raster })
}

/// The elementary-stream bytes of a PES that begins in this payload (past its header).
fn pes_es(p: &[u8]) -> Option<&[u8]> {
    if p.len() < 9 || p[0..3] != [0, 0, 1] {
        return None;
    }
    p.get(9 + usize::from(p[8])..)
}

/// The coded size an H.264 sequence parameter set in `es` declares (macroblocks × 16, frame
/// height doubled for field coding; cropping ignored — a 1080p stream reads 1920×1088, which the
/// sink envelope treats as 1080p). `None` without a complete SPS in these bytes.
fn h264_sps_raster(es: &[u8]) -> Option<(u16, u16)> {
    let start = es.windows(4).position(|w| w[0..3] == [0, 0, 1] && w[3] & 0x1f == 7)? + 4;
    // RBSP: drop the emulation-prevention byte of every 00 00 03.
    let mut rbsp = Vec::with_capacity(64);
    let mut zeros = 0;
    for &b in es[start..].iter().take(256) {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        rbsp.push(b);
    }
    let mut r = Bits { data: &rbsp, at: 0 };
    let profile = r.bits(8)?;
    r.bits(16)?; // constraint flags, level
    r.ue()?; // seq_parameter_set_id
    if matches!(profile, 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135) {
        let chroma = r.ue()?;
        if chroma == 3 {
            r.bits(1)?;
        }
        r.ue()?; // bit_depth_luma_minus8
        r.ue()?; // bit_depth_chroma_minus8
        r.bits(1)?; // qpprime_y_zero_transform_bypass
        if r.bits(1)? == 1 {
            for i in 0..if chroma == 3 { 12 } else { 8 } {
                if r.bits(1)? == 1 {
                    let size = if i < 6 { 16 } else { 64 };
                    let (mut last, mut next) = (8i64, 8i64);
                    for _ in 0..size {
                        if next != 0 {
                            next = (last + r.se()? + 256) % 256;
                        }
                        if next != 0 {
                            last = next;
                        }
                    }
                }
            }
        }
    }
    r.ue()?; // log2_max_frame_num_minus4
    match r.ue()? {
        0 => {
            r.ue()?;
        }
        1 => {
            r.bits(1)?;
            r.se()?;
            r.se()?;
            for _ in 0..r.ue()?.min(255) {
                r.se()?;
            }
        }
        _ => {}
    }
    r.ue()?; // max_num_ref_frames
    r.bits(1)?; // gaps_in_frame_num_value_allowed
    let w_mbs = r.ue()? + 1;
    let h_units = r.ue()? + 1;
    let frame_mbs_only = r.bits(1)?;
    let w = w_mbs * 16;
    let h = h_units * 16 * (2 - frame_mbs_only);
    (w <= 8192 && h <= 8192).then(|| (w as u16, h as u16))
}

/// A big-endian bit reader over an RBSP, with the Exp-Golomb codes an SPS is written in.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.at / 8)?;
        let b = (byte >> (7 - self.at % 8)) & 1;
        self.at += 1;
        Some(u32::from(b))
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        (0..n).try_fold(0u32, |v, _| Some(v << 1 | self.bit()?))
    }

    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1u32 << zeros) - 1 + self.bits(zeros)?)
    }

    fn se(&mut self) -> Option<i64> {
        let k = i64::from(self.ue()?);
        Some(if k % 2 == 1 { (k + 1) / 2 } else { -k / 2 })
    }
}

/// The offset of the first packet boundary three consecutive sync bytes agree on.
fn sync_offset(ts: &[u8]) -> Option<usize> {
    (0..TS_PACKET.min(ts.len())).find(|&i| {
        ts.len() >= i + 3 * TS_PACKET && ts[i] == 0x47 && ts[i + TS_PACKET] == 0x47 && ts[i + 2 * TS_PACKET] == 0x47
    })
}

/// A packet's payload, past any adaptation field. `None` for a packet that carries none.
fn payload_of(pkt: &[u8]) -> Option<&[u8]> {
    let afc = (pkt[3] >> 4) & 0x3;
    match afc {
        1 => Some(&pkt[4..]),
        3 => {
            let skip = 5 + usize::from(pkt[4]);
            (skip < pkt.len()).then(|| &pkt[skip..])
        }
        _ => None,
    }
}

/// A PSI section at the head of a PUSI payload (past the pointer field): (table id, body after
/// the 8-byte long header, up to the CRC).
fn section(payload: &[u8]) -> Option<(u8, &[u8])> {
    let pointer = usize::from(*payload.first()?);
    let s = payload.get(1 + pointer..)?;
    let table = *s.first()?;
    let len = (usize::from(*s.get(1)? & 0x0f) << 8) | usize::from(*s.get(2)?);
    // The section length counts from after its own field to the end of the CRC.
    let end = (3 + len).min(s.len());
    let body = s.get(8..end.checked_sub(4)?)?;
    Some((table, body))
}

fn pat_first_pmt(payload: &[u8]) -> Option<u16> {
    let (table, body) = section(payload)?;
    if table != 0x00 {
        return None;
    }
    body.chunks_exact(4).find_map(|e| {
        let program = (u16::from(e[0]) << 8) | u16::from(e[1]);
        (program != 0).then(|| (u16::from(e[2] & 0x1f) << 8) | u16::from(e[3]))
    })
}

type PmtStreams = (Option<(u16, &'static str)>, Option<&'static str>);

fn pmt_streams(payload: &[u8]) -> Option<PmtStreams> {
    let (table, body) = section(payload)?;
    if table != 0x02 || body.len() < 4 {
        return None;
    }
    let info_len = (usize::from(body[2] & 0x0f) << 8) | usize::from(body[3]);
    let mut es = body.get(4 + info_len..)?;
    let (mut video, mut audio) = (None, None);
    while es.len() >= 5 {
        let stream_type = es[0];
        let pid = (u16::from(es[1] & 0x1f) << 8) | u16::from(es[2]);
        let es_info_len = (usize::from(es[3] & 0x0f) << 8) | usize::from(es[4]);
        let descriptors = es.get(5..5 + es_info_len).unwrap_or(&[]);
        match stream_type {
            0x1b if video.is_none() => video = Some((pid, "h264")),
            0x24 if video.is_none() => video = Some((pid, "hevc")),
            0x0f if audio.is_none() => audio = Some("aac"),
            0x81 if audio.is_none() => audio = Some("ac3"),
            0x87 if audio.is_none() => audio = Some("eac3"),
            // DVB carries AC-3 / E-AC-3 as private data with a descriptor naming it.
            0x06 if audio.is_none() => {
                if has_descriptor(descriptors, 0x7a) {
                    audio = Some("eac3");
                } else if has_descriptor(descriptors, 0x6a) {
                    audio = Some("ac3");
                }
            }
            _ => {}
        }
        es = es.get(5 + es_info_len..).unwrap_or(&[]);
    }
    Some((video, audio))
}

fn has_descriptor(mut d: &[u8], tag: u8) -> bool {
    while d.len() >= 2 {
        if d[0] == tag {
            return true;
        }
        d = d.get(2 + usize::from(d[1])..).unwrap_or(&[]);
    }
    false
}

/// The PTS of a PES packet that begins in this payload, in 90 kHz ticks.
fn pes_pts(p: &[u8]) -> Option<i64> {
    if p.len() < 14 || p[0..3] != [0, 0, 1] || p[7] & 0x80 == 0 {
        return None;
    }
    let b = &p[9..14];
    Some(
        (i64::from(b[0] & 0x0e) << 29)
            | (i64::from(b[1]) << 22)
            | (i64::from(b[2] & 0xfe) << 14)
            | (i64::from(b[3]) << 7)
            | (i64::from(b[4]) >> 1),
    )
}

/// The frame rate the timestamps describe: the median gap between consecutive DISTINCT sorted
/// PTS values (sorting undoes B-frame reordering), snapped to the broadcast/film rate it is within
/// 1 % of. 0 when there are too few to say.
fn rate_of(pts: &[i64]) -> f64 {
    let mut sorted: Vec<i64> = pts.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut gaps: Vec<i64> = sorted.windows(2).map(|w| w[1] - w[0]).filter(|&g| g > 0 && g < 90_000).collect();
    if gaps.len() < 4 {
        return 0.0;
    }
    gaps.sort_unstable();
    let median = gaps[gaps.len() / 2] as f64;
    let fps = 90_000.0 / median;
    const RATES: [f64; 8] = [24_000.0 / 1001.0, 24.0, 25.0, 30_000.0 / 1001.0, 30.0, 50.0, 60_000.0 / 1001.0, 60.0];
    RATES
        .iter()
        .copied()
        .min_by(|a, b| (a - fps).abs().total_cmp(&(b - fps).abs()))
        .filter(|r| (r - fps).abs() / r < 0.01)
        .unwrap_or(fps)
}

#[cfg(test)]
mod tests {

    fn session(url: &str, airing_start_ms: Option<i64>) -> LiveSession {
        let ch = plx_data::livetv::guide::Channel { number: "900".into(), url: url.into(), ..Default::default() };
        let lineup = Arc::new(Lineup { channels: vec![ch], ..Default::default() });
        LiveSession { airing_start_ms, ..LiveSession::tuning(lineup, 0, None) }
    }

    #[test]
    fn a_virtual_channel_knows_how_far_behind_live_a_pause_left_it() {
        let s = session("plxvc:42", Some(1_000_000));
        assert!(s.is_virtual());
        // Ten minutes into the programme on the wall clock, the item at ten minutes: on time.
        assert_eq!(s.behind_ms(1_000_000 + 600_000, 600_000 * 1_000_000), 0);
        assert!(!s.is_behind(1_000_000 + 600_000, 600_000 * 1_000_000));
        // Paused for a minute: a minute behind, and OK jumps back.
        assert_eq!(s.behind_ms(1_000_000 + 660_000, 600_000 * 1_000_000), 60_000);
        assert!(s.is_behind(1_000_000 + 660_000, 600_000 * 1_000_000));
        let tunarr = session("http://192.0.2.20:8000/stream/1", None);
        assert!(!tunarr.is_virtual() && tunarr.behind_ms(i64::MAX / 2, 0) == 0, "a Tunarr stream is always live");
    }

    use super::*;

    /// A transport packet carrying `payload` on `pid`, padded with an adaptation field so the
    /// packet is exactly 188 bytes (the way a muxer stuffs a short PSI or PES start).
    fn packet(pid: u16, pusi: bool, payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() <= 183);
        let mut p = vec![0x47, (if pusi { 0x40 } else { 0 }) | ((pid >> 8) as u8 & 0x1f), pid as u8];
        let stuffing = 184 - payload.len();
        if stuffing == 0 {
            p.push(0x10);
        } else {
            p.push(0x30);
            p.push((stuffing - 1) as u8);
            if stuffing > 1 {
                p.push(0x00);
                p.extend(std::iter::repeat(0xff).take(stuffing - 2));
            }
        }
        p.extend_from_slice(payload);
        assert_eq!(p.len(), 188);
        p
    }

    fn psi(table: u8, body: &[u8]) -> Vec<u8> {
        // pointer, table id, length, then the 5 bytes of the long header, the body and a dummy CRC.
        let len = 5 + body.len() + 4;
        let mut s = vec![0x00, table, 0xb0 | ((len >> 8) as u8), len as u8, 0x00, 0x01, 0xc1, 0x00, 0x00];
        s.extend_from_slice(body);
        s.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        s
    }

    fn pat(pmt: u16) -> Vec<u8> {
        packet(0, true, &psi(0x00, &[0x00, 0x01, 0xe0 | (pmt >> 8) as u8, pmt as u8]))
    }

    fn pmt(streams: &[(u8, u16, &[u8])]) -> Vec<u8> {
        let mut body = vec![0xe1, 0x00, 0xf0, 0x00]; // PCR PID 0x100, no programme info
        for (ty, pid, desc) in streams {
            body.extend_from_slice(&[*ty, 0xe0 | (pid >> 8) as u8, *pid as u8, 0xf0, desc.len() as u8]);
            body.extend_from_slice(desc);
        }
        packet(0x1000, true, &psi(0x02, &body))
    }

    fn pes(pid: u16, pts: i64) -> Vec<u8> {
        let b = [
            0x21 | (((pts >> 30) & 0x07) as u8) << 1,
            (pts >> 22) as u8,
            0x01 | ((pts >> 15) as u8) << 1,
            (pts >> 7) as u8,
            0x01 | ((pts as u8) << 1),
        ];
        let mut p = vec![0, 0, 1, 0xe0, 0, 0, 0x80, 0x80, 5];
        p.extend_from_slice(&b);
        p.extend_from_slice(&[0, 0, 0, 1, 0x09, 0xf0]);
        packet(pid, true, &p)
    }

    fn stream(streams: &[(u8, u16, &[u8])], frame_ticks: i64, frames: usize) -> Vec<u8> {
        let mut ts = vec![0xffu8; 7]; // a torn packet tail before the first sync
        ts.extend(pat(0x1000));
        ts.extend(pmt(streams));
        // Decode order with B-frames: I P B B P B B … — PTS out of order.
        let order = [0i64, 3, 1, 2, 6, 4, 5, 9, 7, 8];
        for i in 0..frames {
            let k = order[i % order.len()] + (i / order.len()) as i64 * 10;
            ts.extend(pes(0x100, 900_000 + k * frame_ticks));
        }
        ts
    }

    #[test]
    fn tunarrs_default_h264_aac_at_29_97_is_recognised() {
        let ts = stream(&[(0x1b, 0x100, &[]), (0x0f, 0x101, &[])], 3003, 40);
        let f = analyse(&ts).unwrap();
        assert_eq!((f.vcodec, f.acodec), ("h264", "aac"));
        assert!((f.fps - 30_000.0 / 1001.0).abs() < 1e-9, "{}", f.fps);
    }

    #[test]
    fn hevc_with_dvb_eac3_at_24p() {
        let ts = stream(&[(0x24, 0x100, &[]), (0x06, 0x101, &[0x7a, 0x01, 0x00])], 3750, 40);
        let f = analyse(&ts).unwrap();
        assert_eq!((f.vcodec, f.acodec), ("hevc", "eac3"));
        assert_eq!(f.fps, 24.0);
    }

    #[test]
    fn ac3_stream_type_and_too_few_frames_to_measure() {
        let ts = stream(&[(0x1b, 0x100, &[]), (0x81, 0x101, &[])], 1800, 3);
        let f = analyse(&ts).unwrap();
        assert_eq!(f.acodec, "ac3");
        assert_eq!(f.fps, 0.0, "three frames say nothing about a rate");
    }

    #[test]
    fn a_stream_without_playable_video_is_refused() {
        let ts = stream(&[(0x02, 0x100, &[]), (0x0f, 0x101, &[])], 3003, 10);
        assert_eq!(analyse(&ts), Err(ProbeError::NotPlayable("no H.264 or HEVC video in the stream")));
        assert_eq!(analyse(&[0u8; 1000]), Err(ProbeError::NotPlayable("no MPEG-TS sync")));
        let mut no_pmt = pat(0x1000);
        no_pmt.extend(pat(0x1000));
        no_pmt.extend(pat(0x1000));
        assert_eq!(analyse(&no_pmt), Err(ProbeError::NotPlayable("no programme table in the stream")));
    }

    /// An H.264 SPS writer for the tests: Exp-Golomb codes into bytes.
    struct Writer(Vec<bool>);
    impl Writer {
        fn bits(&mut self, v: u32, n: u32) -> &mut Self {
            for i in (0..n).rev() {
                self.0.push(v >> i & 1 == 1);
            }
            self
        }
        fn ue(&mut self, v: u32) -> &mut Self {
            let n = 32 - (v + 1).leading_zeros();
            self.bits(0, n - 1).bits(v + 1, n)
        }
        fn bytes(&self) -> Vec<u8> {
            self.0.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |b, (i, &bit)| b | (u8::from(bit) << (7 - i)))).collect()
        }
    }

    /// A High-profile SPS for `w_mbs` × `h_mbs` macroblocks, progressive, after a start code.
    fn sps(w_mbs: u32, h_mbs: u32) -> Vec<u8> {
        let mut w = Writer(Vec::new());
        w.bits(100, 8).bits(0, 8).bits(51, 8).ue(0) // High, level 5.1, sps 0
            .ue(1).ue(0).ue(0).bits(0, 1).bits(0, 1) // 4:2:0, 8-bit, no scaling matrices
            .ue(0).ue(0).ue(4) // log2_max_frame_num, poc type 0, its lsb
            .ue(4).bits(0, 1) // ref frames, gaps
            .ue(w_mbs - 1).ue(h_mbs - 1).bits(1, 1).bits(1, 1).bits(0, 1).bits(1, 1); // frame only, 8x8, no crop, vui...
        let mut out = vec![0, 0, 0, 1, 0x67];
        out.extend(w.bytes());
        out
    }

    /// **A 4K channel is declared as 4K.** The probe reads the H.264 stream's own size from its
    /// SPS; a 1080p one reads 1920×1088 (coded), which the sink envelope keeps at 1080p.
    #[test]
    fn the_probe_reads_an_h264_channels_size_from_its_sps() {
        assert_eq!(h264_sps_raster(&sps(240, 135)), Some((3840, 2160)));
        assert_eq!(h264_sps_raster(&sps(120, 68)), Some((1920, 1088)));
        assert_eq!(h264_sps_raster(&[0, 0, 1, 0x65, 1, 2, 3]), None, "no SPS: no size");
        // In a PES on the video PID, the way Tunarr's MPEG-TS carries it.
        let mut payload = vec![0, 0, 1, 0xe0, 0, 0, 0x80, 0x80, 5, 0x21, 0, 1, 0, 1];
        payload.extend_from_slice(&[0, 0, 0, 1, 0x09, 0xf0]);
        payload.extend(sps(240, 135));
        let mut ts = pat(0x1000);
        ts.extend(pmt(&[(0x1b, 0x100, &[]), (0x0f, 0x101, &[])]));
        ts.extend(packet(0x100, true, &payload));
        assert_eq!(analyse(&ts).unwrap().raster, (3840, 2160));
    }

    #[test]
    fn an_unusual_rate_is_kept_as_measured() {
        assert!((rate_of(&[0, 6000, 12000, 18000, 24000, 30000]) - 15.0).abs() < 1e-9);
        assert_eq!(rate_of(&[0, 1500, 3000, 4500, 6000, 7500]), 60.0);
    }
}
